use crate::circuit_breaker::CircuitBreaker;
use crate::config::{Config, ModelSelection, ProviderConfig};
use log::{debug, error, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ── Global in-flight tracking ───────────────────────────────────────────────
// Shared across all LlmChain instances so concurrent requests from different
// agents/handlers see each other's in-flight counts for the same provider.

/// Global per-provider metrics shared across all LlmChain instances.
struct GlobalProviderMetrics {
    in_flight: AtomicU32,
    /// Latency EMA stored as microseconds (avoids float atomics).
    latency_ema_us: std::sync::atomic::AtomicU64,
    total_requests: std::sync::atomic::AtomicU64,
}

static PROVIDER_METRICS: OnceLock<std::sync::Mutex<HashMap<String, Arc<GlobalProviderMetrics>>>> =
    OnceLock::new();

fn provider_metrics(name: &str) -> Arc<GlobalProviderMetrics> {
    let map = PROVIDER_METRICS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap();
    guard
        .entry(name.to_string())
        .or_insert_with(|| {
            Arc::new(GlobalProviderMetrics {
                in_flight: AtomicU32::new(0),
                latency_ema_us: std::sync::atomic::AtomicU64::new(0),
                total_requests: std::sync::atomic::AtomicU64::new(0),
            })
        })
        .clone()
}

impl GlobalProviderMetrics {
    fn record_latency(&self, latency_ms: u64) {
        let latency_us = latency_ms * 1000;
        self.total_requests
            .fetch_add(1, Ordering::Relaxed);
        let prev = self.latency_ema_us.load(Ordering::Relaxed);
        let ema = if prev == 0 {
            latency_us
        } else {
            // EMA with alpha=0.2
            (prev * 4 + latency_us) / 5
        };
        self.latency_ema_us.store(ema, Ordering::Relaxed);
    }

    fn avg_latency_ms(&self) -> f64 {
        self.latency_ema_us.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

/// Snapshot of a provider's current state for introspection.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderSnapshot {
    pub name: String,
    pub model_id: String,
    pub in_flight: u32,
    pub avg_latency_ms: f64,
    pub total_requests: u64,
    pub circuit_state: String,
}

// ── LLM Messages ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self { role: "system".to_string(), content: content.to_string(), tool_calls: None, tool_call_id: None }
    }
    pub fn user(content: &str) -> Self {
        Self { role: "user".to_string(), content: content.to_string(), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".to_string(), content: content.to_string(), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant_with_tools(content: &str, tool_calls: Vec<serde_json::Value>) -> Self {
        Self { role: "assistant".to_string(), content: content.to_string(), tool_calls: Some(tool_calls), tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self { role: "tool".to_string(), content: content.to_string(), tool_calls: None, tool_call_id: Some(tool_call_id.to_string()) }
    }
}

/// Result of an LLM call — either text or tool calls
#[derive(Debug)]
pub enum LlmResult {
    Text(String),
    ToolCalls { content: String, tool_calls: Vec<serde_json::Value> },
}

#[derive(Deserialize, Debug)]
struct LlmResponse {
    choices: Option<Vec<LlmChoice>>,
}

#[derive(Deserialize, Debug)]
struct LlmChoice {
    message: Option<LlmMessage>,
}

#[derive(Deserialize, Debug)]
struct LlmMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<serde_json::Value>>,
}

// ── Provider Chain ──────────────────────────────────────────────────────────

/// Pick a sensible default HTTP timeout for a provider based on its name.
///
/// - `claude-shim` / `claude-cli`: 600s — these spawn local `claude -p` which
///   can take 60-300s for tool-using turns (Claude Code does its own tool
///   execution internally before returning final text). 120s was too short
///   and caused fall-throughs to fallback providers mid-conversation.
/// - `lmstudio` / `local`: 60s — fast local inference servers, anything
///   slower than this means they're stuck.
/// - everything else: 120s — remote OpenAI-compatible providers (OpenRouter
///   Nemotron, etc.).
fn provider_default_timeout(prov_name: &str) -> Duration {
    if prov_name.contains("claude-shim") || prov_name.contains("claude-cli") {
        Duration::from_secs(600)
    } else if prov_name.contains("lmstudio") || prov_name.contains("local") {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(120)
    }
}

pub struct LlmProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub max_tokens: u64,
    pub circuit: Mutex<CircuitBreaker>,
}

pub struct LlmChain {
    providers: Vec<LlmProvider>,
    client: Client,
}

impl LlmChain {
    /// Build a chain that prefers the agent's `fast` model for cheap/quick
    /// phases (research planning, report synthesis). Falls back to the
    /// primary model if no fast model is configured.
    pub fn from_config_fast(config: &Config, agent_id: &str, client: Client) -> Self {
        let model_sel = config.agent_model(agent_id);
        if let Some(fast) = model_sel.fast.as_deref() {
            // Build a chain with fast as primary, original primary as fallback,
            // then the original fallbacks. Keeps resilience while preferring fast.
            let mut alt = ModelSelection::default();
            alt.primary = fast.to_string();
            alt.fallbacks = std::iter::once(model_sel.primary.clone())
                .chain(model_sel.fallbacks.iter().cloned())
                .collect();
            return Self::from_model_selection(config, &alt, agent_id, client);
        }
        Self::from_config(config, agent_id, client)
    }

    /// Build from config's model selection (primary + fallbacks)
    pub fn from_config(config: &Config, agent_id: &str, client: Client) -> Self {
        let model_sel = config.agent_model(agent_id);
        Self::from_model_selection(config, model_sel, agent_id, client)
    }

    /// Build a chain from any ModelSelection (used by from_config and from_config_fast).
    fn from_model_selection(
        config: &Config,
        model_sel: &ModelSelection,
        agent_id: &str,
        client: Client,
    ) -> Self {
        let mut providers = Vec::new();

        // Primary
        if let Some((prov_name, model_id)) = config.resolve_model(&model_sel.primary) {
            if let Some(prov_config) = config.models.providers.get(&prov_name) {
                let timeout = provider_default_timeout(&prov_name);

                providers.push(LlmProvider {
                    name: prov_name.clone(),
                    base_url: prov_config.base_url.clone(),
                    api_key: prov_config.api_key.clone(),
                    model_id,
                    max_tokens: prov_config.models.first().map(|m| m.max_tokens).unwrap_or(4096),
                    circuit: Mutex::new(CircuitBreaker::new(&prov_name, timeout)),
                });
            }
        }

        // Fallbacks
        for fallback in &model_sel.fallbacks {
            if let Some((prov_name, model_id)) = config.resolve_model(fallback) {
                if let Some(prov_config) = config.models.providers.get(&prov_name) {
                    let timeout = provider_default_timeout(&prov_name);
                    providers.push(LlmProvider {
                        name: prov_name.clone(),
                        base_url: prov_config.base_url.clone(),
                        api_key: prov_config.api_key.clone(),
                        model_id,
                        max_tokens: prov_config.models.first().map(|m| m.max_tokens).unwrap_or(4096),
                        circuit: Mutex::new(CircuitBreaker::new(&prov_name, timeout)),
                    });
                }
            }
        }

        if providers.is_empty() {
            warn!("No LLM providers configured for agent {}", agent_id);
        } else {
            info!("LLM chain for {}: {}", agent_id,
                providers.iter().map(|p| format!("{}:{}", p.name, p.model_id)).collect::<Vec<_>>().join(" → "));
        }

        Self { providers, client }
    }

    /// Generate an embedding for the given text via the configured embedding
    /// model. Uses the FIRST provider in the chain whose `models` list contains
    /// an entry tagged as an embedding model (heuristic: id contains "embed").
    /// Returns the raw f32 vector. Falls back to an error if no embedding
    /// provider is wired.
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>, String> {
        // Try each provider in order; first success wins
        for prov in &self.providers {
            // Heuristic: skip providers whose model_id doesn't look like an embedder
            if !prov.model_id.contains("embed") && !prov.model_id.contains("Embed") {
                continue;
            }
            let url = format!("{}/embeddings", prov.base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": prov.model_id,
                "input": text,
            });
            let req = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", prov.api_key))
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(60))
                .json(&body);
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("[llm:{}] embed: {}", prov.name, e);
                    continue;
                }
            };
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                log::warn!("[llm:{}] embed HTTP error: {}", prov.name, body);
                continue;
            }
            #[derive(serde::Deserialize)]
            struct EmbedItem { embedding: Vec<f32> }
            #[derive(serde::Deserialize)]
            struct EmbedResp { data: Vec<EmbedItem> }
            let parsed: EmbedResp = match resp.json().await {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("[llm:{}] embed parse: {}", prov.name, e);
                    continue;
                }
            };
            if let Some(item) = parsed.data.into_iter().next() {
                return Ok(item.embedding);
            }
        }
        Err("no embedding provider succeeded".to_string())
    }

    /// True if any provider in the chain looks like an embedding model.
    pub fn has_embedder(&self) -> bool {
        self.providers
            .iter()
            .any(|p| p.model_id.contains("embed") || p.model_id.contains("Embed"))
    }

    /// Score all providers and return indices in best-first order.
    /// Uses circuit breaker state, average latency, and in-flight count
    /// to pick the best provider — not just the first in config order.
    async fn ranked_order(&self) -> Vec<usize> {
        let mut scored: Vec<(usize, f64)> = Vec::with_capacity(self.providers.len());

        for (i, provider) in self.providers.iter().enumerate() {
            let circuit = provider.circuit.lock().await;
            let score;

            if !circuit.is_available() {
                score = 100_000.0;
            } else {
                let metrics = provider_metrics(&provider.name);
                let global_avg = metrics.avg_latency_ms();
                let circuit_avg = circuit.avg_latency_ms();
                // Use whichever has data; prefer global (cross-chain)
                let avg_lat = if global_avg > 0.0 { global_avg } else { circuit_avg };
                let base = if avg_lat > 0.0 { avg_lat } else { 500.0 };

                let active = metrics.in_flight.load(Ordering::Relaxed) as f64;
                let penalty = active * base.max(500.0) * 0.5;

                score = base + penalty;
            }

            scored.push((i, score));
        }

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(i, _)| i).collect()
    }

    /// Get stats for all providers in this chain (for diagnostics).
    pub async fn provider_stats(&self) -> Vec<ProviderSnapshot> {
        let mut stats = Vec::new();
        for provider in &self.providers {
            let circuit = provider.circuit.lock().await;
            let metrics = provider_metrics(&provider.name);
            stats.push(ProviderSnapshot {
                name: provider.name.clone(),
                model_id: provider.model_id.clone(),
                in_flight: metrics.in_flight.load(Ordering::Relaxed),
                avg_latency_ms: metrics.avg_latency_ms(),
                total_requests: metrics.total_requests.load(Ordering::Relaxed),
                circuit_state: format!("{:?}", circuit.state()),
            });
        }
        stats
    }

    /// Call LLM — returns text only
    pub async fn call(&self, messages: &[ChatMessage]) -> Result<String, String> {
        match self.call_raw(messages, None).await? {
            LlmResult::Text(t) => Ok(t),
            LlmResult::ToolCalls { content, .. } => Ok(if content.is_empty() { "(tool call requested)".to_string() } else { content }),
        }
    }

    /// Call LLM with tools — returns structured result.
    /// Providers are tried in load-aware ranked order (best score first)
    /// rather than simple config order, with in-flight tracking across chains.
    pub async fn call_raw(&self, messages: &[ChatMessage], tools: Option<&Vec<serde_json::Value>>) -> Result<LlmResult, String> {
        let order = self.ranked_order().await;
        let total = order.len();
        let mut last_error: Option<String> = None;

        for (attempt, &idx) in order.iter().enumerate() {
            let provider = &self.providers[idx];

            // Check circuit breaker (may transition Open→HalfOpen)
            {
                let mut circuit = provider.circuit.lock().await;
                if !circuit.can_execute() {
                    debug!("[llm:{}] Circuit OPEN, skipping", provider.name);
                    continue;
                }
            }

            let timeout = {
                let circuit = provider.circuit.lock().await;
                circuit.timeout()
            };

            let metrics = provider_metrics(&provider.name);
            metrics.in_flight.fetch_add(1, Ordering::Relaxed);

            let start = Instant::now();
            info!("[llm:{}] Calling model={} timeout={}s in_flight={}", provider.name, provider.model_id, timeout.as_secs(), metrics.in_flight.load(Ordering::Relaxed));
            let result = call_provider(&self.client, provider, messages, timeout, tools).await;

            metrics.in_flight.fetch_sub(1, Ordering::Relaxed);

            match result {
                Ok(result) => {
                    let latency = start.elapsed().as_millis() as u64;
                    metrics.record_latency(latency);
                    let desc = match &result {
                        LlmResult::Text(t) => format!("text {} chars", t.len()),
                        LlmResult::ToolCalls { tool_calls, .. } => format!("{} tool calls", tool_calls.len()),
                    };
                    info!("[llm:{}] Success in {}ms ({})", provider.name, latency, desc);
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_success(latency);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let was_timeout = e.contains("timeout") || e.contains("timed out");
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_failure(was_timeout);
                    }

                    if attempt < total - 1 {
                        warn!("[llm:{}] Failed after {}ms ({}), trying next provider", provider.name, latency, e);
                    } else {
                        error!("[llm] All {} providers failed after {}ms. Last error: {}", self.providers.len(), latency, e);
                        last_error = Some(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "All LLM providers failed".to_string()))
    }

    /// Same as `call_raw` but with an explicit `max_tokens` cap that
    /// overrides the provider's configured ceiling. Intended for the voice
    /// pipeline where responses must be 1 short sentence so TTS playback
    /// is brief and the echo window stays small. Any other caller should
    /// keep using `call_raw`.
    pub async fn call_raw_capped(
        &self,
        messages: &[ChatMessage],
        tools: Option<&Vec<serde_json::Value>>,
        max_tokens_override: u32,
    ) -> Result<LlmResult, String> {
        let order = self.ranked_order().await;
        let total = order.len();
        let mut last_error: Option<String> = None;

        for (attempt, &idx) in order.iter().enumerate() {
            let provider = &self.providers[idx];
            {
                let mut circuit = provider.circuit.lock().await;
                if !circuit.can_execute() {
                    debug!("[llm:{}] Circuit OPEN, skipping", provider.name);
                    continue;
                }
            }
            let timeout = {
                let circuit = provider.circuit.lock().await;
                circuit.timeout()
            };

            let metrics = provider_metrics(&provider.name);
            metrics.in_flight.fetch_add(1, Ordering::Relaxed);

            let start = Instant::now();
            info!(
                "[llm:{}] Calling model={} timeout={}s max_tokens={} in_flight={} (capped)",
                provider.name, provider.model_id, timeout.as_secs(), max_tokens_override, metrics.in_flight.load(Ordering::Relaxed)
            );
            let result = call_provider_capped(
                &self.client,
                provider,
                messages,
                timeout,
                tools,
                max_tokens_override,
            )
            .await;

            metrics.in_flight.fetch_sub(1, Ordering::Relaxed);

            match result {
                Ok(result) => {
                    let latency = start.elapsed().as_millis() as u64;
                    metrics.record_latency(latency);
                    let desc = match &result {
                        LlmResult::Text(t) => format!("text {} chars", t.len()),
                        LlmResult::ToolCalls { tool_calls, .. } => {
                            format!("{} tool calls", tool_calls.len())
                        }
                    };
                    info!("[llm:{}] Success in {}ms ({})", provider.name, latency, desc);
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_success(latency);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let was_timeout = e.contains("timeout") || e.contains("timed out");
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_failure(was_timeout);
                    }
                    if attempt < total - 1 {
                        warn!(
                            "[llm:{}] Failed after {}ms ({}), trying next provider",
                            provider.name, latency, e
                        );
                    } else {
                        error!(
                            "[llm] All {} providers failed after {}ms. Last error: {}",
                            self.providers.len(),
                            latency,
                            e
                        );
                        last_error = Some(e);
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "All LLM providers failed".to_string()))
    }
}

/// Return a dashboard/status link for a provider, if known.
fn provider_dashboard_link(name: &str, base_url: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if lower.contains("openrouter") || base_url.contains("openrouter.ai") {
        Some("https://openrouter.ai/credits")
    } else if lower.contains("openai") || base_url.contains("api.openai.com") {
        Some("https://platform.openai.com/usage")
    } else if lower.contains("anthropic") || base_url.contains("api.anthropic.com") {
        Some("https://console.anthropic.com/settings/billing")
    } else if lower.contains("google") || base_url.contains("generativelanguage.googleapis.com") {
        Some("https://console.cloud.google.com/billing")
    } else {
        None
    }
}

async fn call_provider(
    client: &Client,
    provider: &LlmProvider,
    messages: &[ChatMessage],
    timeout: Duration,
    tools: Option<&Vec<serde_json::Value>>,
) -> Result<LlmResult, String> {
    let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));

    let mut payload = serde_json::json!({
        "model": provider.model_id,
        "messages": messages,
        "max_tokens": provider.max_tokens.min(8192).max(2000),
        "temperature": 0.7,
    });

    // Add tool definitions if provided
    if let Some(tools) = tools {
        payload["tools"] = serde_json::json!(tools);
    }

    debug!("[llm:{}] POST {} (model={}, messages={}, max_tokens={})",
        provider.name, url, provider.model_id, messages.len(), provider.max_tokens.min(8192).max(2000));

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", provider.api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| {
            let msg = if e.is_timeout() {
                format!("{} timed out after {}s — the model may be overloaded or the server may be down", provider.name, timeout.as_secs())
            } else if e.is_connect() {
                format!("Can't reach {} — check that the server is running and the URL is correct", provider.name)
            } else {
                format!("{} request failed: {}", provider.name, e)
            };
            error!("[llm:{}] HTTP error: {}", provider.name, msg);
            msg
        })?;

    let status = resp.status();
    debug!("[llm:{}] HTTP {}", provider.name, status);

    // Handle rate limit and server errors
    let dashboard = provider_dashboard_link(&provider.name, &provider.base_url);
    let link_hint = dashboard.map(|url| format!("\n\nCheck status and billing:\n{}", url)).unwrap_or_default();

    if status.as_u16() == 429 {
        return Err(format!("{} is rate limited (HTTP 429) — too many requests, will retry later.{}", provider.name, link_hint));
    }
    if status.as_u16() == 402 {
        return Err(format!("{} billing error (HTTP 402) — check API credits or payment method.{}", provider.name, link_hint));
    }
    if status.is_server_error() {
        return Err(format!("{} returned server error ({}) — this is usually temporary, will retry.{}", provider.name, status, link_hint));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{} returned HTTP {} — {}{}", provider.name, status, body.chars().take(200).collect::<String>(), link_hint));
    }

    let raw_body = resp.text().await
        .map_err(|e| format!("{} response could not be read: {}", provider.name, e))?;

    debug!("[llm:{}] Raw response: {}...", provider.name, &raw_body[..raw_body.len().min(500)]);

    let body: LlmResponse = serde_json::from_str(&raw_body)
        .map_err(|e| {
            error!("[llm:{}] Response parse error: {} — raw: {}...", provider.name, e, &raw_body[..raw_body.len().min(200)]);
            format!("{} returned an unexpected response format — the model endpoint may have changed or be misconfigured.{}", provider.name, link_hint)
        })?;

    let message = body.choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message);

    let content = match &message {
        Some(m) => {
            let c = m.content.clone()
                .or_else(|| m.reasoning_content.clone())
                .or_else(|| m.reasoning.clone())
                .unwrap_or_default();
            if c.is_empty() {
                warn!("[llm:{}] All content fields empty. content={:?}, reasoning_content={:?}, reasoning={:?}",
                    provider.name, m.content.is_some(), m.reasoning_content.is_some(), m.reasoning.is_some());
            }
            c
        }
        None => {
            error!("[llm:{}] No message in response", provider.name);
            String::new()
        }
    };

    // Check for tool calls in the response
    let raw_tool_calls: Option<Vec<serde_json::Value>> = {
        let raw_val: serde_json::Value = serde_json::from_str(&raw_body).unwrap_or_default();
        raw_val.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array())
            .cloned()
    };

    if let Some(ref tc) = raw_tool_calls {
        if !tc.is_empty() {
            info!("[llm:{}] {} tool call(s) returned", provider.name, tc.len());
            return Ok(LlmResult::ToolCalls {
                content: content.clone(),
                tool_calls: tc.clone(),
            });
        }
    }

    if content.is_empty() {
        return Err(format!("{} returned an empty response — the model may be overloaded or misconfigured", provider.name));
    }

    // Strip <think> blocks
    let think_re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let cleaned = think_re.replace_all(&content, "").trim().to_string();

    Ok(LlmResult::Text(if cleaned.is_empty() { content } else { cleaned }))
}

/// Same body as `call_provider` but overrides `max_tokens` at the request
/// level rather than inheriting the provider's configured ceiling. Voice
/// responses must stay under ~1 sentence so TTS audio is brief.
async fn call_provider_capped(
    client: &Client,
    provider: &LlmProvider,
    messages: &[ChatMessage],
    timeout: Duration,
    tools: Option<&Vec<serde_json::Value>>,
    max_tokens: u32,
) -> Result<LlmResult, String> {
    let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));

    let mut payload = serde_json::json!({
        "model": provider.model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.3,
    });
    if let Some(tools) = tools {
        payload["tools"] = serde_json::json!(tools);
    }

    debug!(
        "[llm:{}] POST {} (capped, max_tokens={}, messages={})",
        provider.name, url, max_tokens, messages.len()
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", provider.api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| {
            let msg = if e.is_timeout() {
                format!("{} timed out after {}s — the model may be overloaded or the server may be down", provider.name, timeout.as_secs())
            } else if e.is_connect() {
                format!("Can't reach {} — check that the server is running and the URL is correct", provider.name)
            } else {
                format!("{} request failed: {}", provider.name, e)
            };
            error!("[llm:{}] HTTP error: {}", provider.name, msg);
            msg
        })?;

    let status = resp.status();
    let dashboard = provider_dashboard_link(&provider.name, &provider.base_url);
    let link_hint = dashboard.map(|url| format!("\n\nCheck status and billing:\n{}", url)).unwrap_or_default();

    if status.as_u16() == 429 {
        return Err(format!("{} is rate limited (HTTP 429) — too many requests, will retry later.{}", provider.name, link_hint));
    }
    if status.as_u16() == 402 {
        return Err(format!("{} billing error (HTTP 402) — check API credits or payment method.{}", provider.name, link_hint));
    }
    if status.is_server_error() {
        return Err(format!("{} returned server error ({}) — this is usually temporary, will retry.{}", provider.name, status, link_hint));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{} returned HTTP {} — {}{}", provider.name, status, body.chars().take(200).collect::<String>(), link_hint));
    }

    let raw_body = resp
        .text()
        .await
        .map_err(|e| format!("{} response could not be read: {}", provider.name, e))?;

    let body: LlmResponse = serde_json::from_str(&raw_body)
        .map_err(|e| format!("{} returned an unexpected response format — the model endpoint may have changed.{}", provider.name, link_hint))?;

    let message = body
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message);

    let content = match &message {
        Some(m) => m
            .content
            .clone()
            .or_else(|| m.reasoning_content.clone())
            .or_else(|| m.reasoning.clone())
            .unwrap_or_default(),
        None => String::new(),
    };

    // Parse tool calls if present
    let raw_tool_calls: Option<Vec<serde_json::Value>> = {
        let raw_val: serde_json::Value = serde_json::from_str(&raw_body).unwrap_or_default();
        raw_val
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array())
            .cloned()
    };
    if let Some(ref tc) = raw_tool_calls {
        if !tc.is_empty() {
            return Ok(LlmResult::ToolCalls {
                content: content.clone(),
                tool_calls: tc.clone(),
            });
        }
    }

    if content.is_empty() {
        return Err("empty response from LLM".to_string());
    }

    let think_re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let cleaned = think_re.replace_all(&content, "").trim().to_string();

    Ok(LlmResult::Text(if cleaned.is_empty() {
        content
    } else {
        cleaned
    }))
}
