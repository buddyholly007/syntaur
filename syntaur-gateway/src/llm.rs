use crate::circuit_breaker::CircuitBreaker;
use crate::config::{Config, ModelSelection, ProviderConfig};
use futures_util::StreamExt;
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

/// Content part for multimodal LLM requests (text + images). Only emitted
/// on the wire when `ChatMessage.content_parts` is set; otherwise the plain
/// `content` string is serialized.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlDetail },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageUrlDetail {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip)]
    pub content_parts: Option<Vec<ContentPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self { role: String::new(), content: String::new(), content_parts: None, tool_calls: None, tool_call_id: None }
    }
}

impl serde::Serialize for ChatMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;
        if let Some(ref parts) = self.content_parts {
            map.serialize_entry("content", parts)?;
        } else {
            map.serialize_entry("content", &self.content)?;
        }
        if let Some(ref tc) = self.tool_calls {
            map.serialize_entry("tool_calls", tc)?;
        }
        if let Some(ref id) = self.tool_call_id {
            map.serialize_entry("tool_call_id", id)?;
        }
        map.end()
    }
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self { role: "system".to_string(), content: content.to_string(), content_parts: None, tool_calls: None, tool_call_id: None }
    }
    pub fn user(content: &str) -> Self {
        Self { role: "user".to_string(), content: content.to_string(), content_parts: None, tool_calls: None, tool_call_id: None }
    }
    pub fn user_with_images(text: &str, image_urls: &[String]) -> Self {
        let mut parts = vec![ContentPart::Text { text: text.to_string() }];
        for url in image_urls {
            parts.push(ContentPart::ImageUrl {
                image_url: ImageUrlDetail { url: url.clone(), detail: Some("auto".to_string()) },
            });
        }
        Self { role: "user".to_string(), content: text.to_string(), content_parts: Some(parts), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".to_string(), content: content.to_string(), content_parts: None, tool_calls: None, tool_call_id: None }
    }
    pub fn assistant_with_tools(content: &str, tool_calls: Vec<serde_json::Value>) -> Self {
        Self { role: "assistant".to_string(), content: content.to_string(), content_parts: None, tool_calls: Some(tool_calls), tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self { role: "tool".to_string(), content: content.to_string(), content_parts: None, tool_calls: None, tool_call_id: Some(tool_call_id.to_string()) }
    }
}

/// Result of an LLM call — either text or tool calls
#[derive(Debug)]
pub enum LlmResult {
    Text(String),
    ToolCalls { content: String, tool_calls: Vec<serde_json::Value> },
}

// ── Provider Chain ──────────────────────────────────────────────────────────

/// Detect placeholder/empty API keys so we can skip the provider instead of
/// wasting a request cycle on a guaranteed 401. Matches empty strings,
/// common "REPLACE_ME"/"YOUR_KEY" patterns, and literal "None"/"null".
fn is_placeholder_api_key(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return true;
    }
    let upper = trimmed.to_uppercase();
    upper.contains("REPLACE_ME")
        || upper.contains("YOUR_KEY")
        || upper.contains("YOUR-KEY")
        || upper.contains("PLACEHOLDER")
        || upper == "NONE"
        || upper == "NULL"
}

/// Pick a sensible default HTTP timeout for a provider based on its name.
///
/// - `claude-shim` / `claude-cli`: 600s — these spawn local `claude -p` which
///   can take 60-300s for tool-using turns (Claude Code does its own tool
///   execution internally before returning final text). 120s was too short
///   and caused fall-throughs to fallback providers mid-conversation.
/// - `lmstudio` / `local`: 60s — fast local inference servers, anything
///   slower than this means they're stuck.
/// - `openrouter` / `groq` / `cerebras`: 30s — free-tier remote providers
///   consistently return tool-calling rounds in 1-5s when healthy. A stall
///   past 30s almost always means a broken stream; fail over fast instead
///   of eating a full 120s per round.
/// - everything else: 90s — remote OpenAI-compatible providers, paid tiers.
fn provider_default_timeout(prov_name: &str) -> Duration {
    if prov_name.contains("claude-shim") || prov_name.contains("claude-cli") {
        Duration::from_secs(600)
    } else if prov_name.contains("lmstudio") || prov_name.contains("local") {
        Duration::from_secs(60)
    } else if prov_name.contains("openrouter")
        || prov_name.contains("groq")
        || prov_name.contains("cerebras")
    {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(90)
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
                if !is_placeholder_api_key(&prov_config.api_key) {
                    let timeout = provider_default_timeout(&prov_name);

                    providers.push(LlmProvider {
                        name: prov_name.clone(),
                        base_url: prov_config.base_url.clone(),
                        api_key: prov_config.api_key.clone(),
                        model_id,
                        max_tokens: prov_config.models.first().map(|m| m.max_tokens).unwrap_or(4096),
                        circuit: Mutex::new(CircuitBreaker::new(&prov_name, timeout)),
                    });
                } else {
                    warn!("[llm] Skipping primary provider '{}' — API key is a placeholder, fill it in and restart", prov_name);
                }
            }
        }

        // Fallbacks
        for fallback in &model_sel.fallbacks {
            if let Some((prov_name, model_id)) = config.resolve_model(fallback) {
                if let Some(prov_config) = config.models.providers.get(&prov_name) {
                    if is_placeholder_api_key(&prov_config.api_key) {
                        debug!("[llm] Skipping fallback '{}' — API key is a placeholder", prov_name);
                        continue;
                    }
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
    ///
    /// Config order is the PRIMARY signal — the user put their primary model
    /// first for quality/cost reasons, and a local "faster" fallback should
    /// never silently replace it while the primary is healthy. Latency and
    /// in-flight load only influence ordering when two providers are at the
    /// same config position (which never happens in practice) or when the
    /// configured primary is seriously degraded.
    ///
    /// Score layout (smaller = higher priority):
    /// - Available provider at position `i`:  `i * 1_000_000 + avg_lat_ms + in_flight_penalty`
    /// - Unavailable provider at position `i`: `1e11 + i * 1_000_000` (always loses to any
    ///   available provider, but still prefers earlier unavailables over later ones so we
    ///   retry in the right order once circuits recover).
    ///
    /// In realistic ranges (avg_lat ≤ 60_000, in_flight ≤ 4) the position term
    /// dominates by 2+ orders of magnitude, so a 14.5s remote primary still
    /// beats a 500ms local fallback — matching the user's configured intent.
    async fn ranked_order(&self) -> Vec<usize> {
        const POSITION_MULT: f64 = 1_000_000.0;
        const UNAVAILABLE_BIAS: f64 = 1.0e11;

        let mut scored: Vec<(usize, f64)> = Vec::with_capacity(self.providers.len());

        for (i, provider) in self.providers.iter().enumerate() {
            let circuit = provider.circuit.lock().await;
            let position_score = (i as f64) * POSITION_MULT;
            let score;

            if !circuit.is_available() {
                score = UNAVAILABLE_BIAS + position_score;
            } else {
                let metrics = provider_metrics(&provider.name);
                let global_avg = metrics.avg_latency_ms();
                let circuit_avg = circuit.avg_latency_ms();
                let avg_lat = if global_avg > 0.0 { global_avg } else { circuit_avg };
                let base = if avg_lat > 0.0 { avg_lat } else { 500.0 };

                let active = metrics.in_flight.load(Ordering::Relaxed) as f64;
                let penalty = active * base.max(500.0) * 0.5;

                score = position_score + base + penalty;
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

/// Read an OpenAI-compatible SSE chat-completion stream and assemble the
/// final result. Fails fast on a stalled stream (no chunk in 10s) instead
/// of waiting for the full request timeout — this is the core fix for
/// OpenRouter free-tier mid-stream stalls that used to burn 120s per round.
async fn read_sse_response(
    resp: reqwest::Response,
    provider_name: &str,
    link_hint: &str,
) -> Result<LlmResult, String> {
    #[derive(Default)]
    struct ToolAcc {
        id: String,
        ty: String,
        name: String,
        arguments: String,
    }

    // Track `content` and `reasoning_content`/`reasoning` separately — some
    // reasoning models (Nemotron, DeepSeek R1) stream their chain-of-thought
    // in `reasoning` and the actual answer in `content`. We only fall back
    // to reasoning if content is empty at end-of-stream, so the user never
    // sees the thinking trace when a real answer was produced.
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_accs: HashMap<u64, ToolAcc> = HashMap::new();
    let mut line_buf = String::new();
    let mut saw_done = false;
    let mut byte_stream = resp.bytes_stream();
    let chunk_timeout = Duration::from_secs(10);

    loop {
        match tokio::time::timeout(chunk_timeout, byte_stream.next()).await {
            Err(_) => {
                return Err(format!(
                    "{} stream stalled — no chunk received in 10s, failing over.{}",
                    provider_name, link_hint
                ));
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                return Err(format!(
                    "{} stream error: {}.{}",
                    provider_name, e, link_hint
                ));
            }
            Ok(Some(Ok(bytes))) => {
                line_buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(idx) = line_buf.find('\n') {
                    let raw_line: String = line_buf.drain(..=idx).collect();
                    let line = raw_line.trim_end_matches(&['\n', '\r'][..]).trim();
                    if line.is_empty() || line.starts_with(':') {
                        continue; // blank separator or SSE comment (keepalive)
                    }
                    let data = match line.strip_prefix("data:") {
                        Some(d) => d.trim_start(),
                        None => continue,
                    };
                    if data == "[DONE]" {
                        saw_done = true;
                        break;
                    }
                    let v: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!(
                                "[llm:{}] skip malformed SSE chunk: {} -- data: {}",
                                provider_name,
                                e,
                                data.chars().take(100).collect::<String>()
                            );
                            continue;
                        }
                    };
                    // Some providers return an error object mid-stream instead of a delta.
                    if let Some(err_obj) = v.get("error") {
                        let err_msg = err_obj
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown stream error");
                        return Err(format!(
                            "{} stream error: {}.{}",
                            provider_name, err_msg, link_hint
                        ));
                    }
                    // Actual answer content
                    if let Some(c) = v
                        .pointer("/choices/0/delta/content")
                        .and_then(|x| x.as_str())
                    {
                        content.push_str(c);
                    }
                    // Reasoning trace — kept separate, only surfaced if
                    // content ends up empty. Some providers use `reasoning`,
                    // others `reasoning_content`.
                    for ptr in [
                        "/choices/0/delta/reasoning_content",
                        "/choices/0/delta/reasoning",
                    ] {
                        if let Some(c) = v.pointer(ptr).and_then(|x| x.as_str()) {
                            reasoning.push_str(c);
                        }
                    }
                    // Tool-call deltas, indexed and chunked per OpenAI schema
                    if let Some(tcs) = v
                        .pointer("/choices/0/delta/tool_calls")
                        .and_then(|x| x.as_array())
                    {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                            let entry = tool_accs.entry(idx).or_default();
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                if entry.id.is_empty() {
                                    entry.id = id.to_string();
                                }
                            }
                            if let Some(ty) = tc.get("type").and_then(|i| i.as_str()) {
                                if entry.ty.is_empty() {
                                    entry.ty = ty.to_string();
                                }
                            }
                            if let Some(fname) =
                                tc.pointer("/function/name").and_then(|i| i.as_str())
                            {
                                entry.name.push_str(fname);
                            }
                            if let Some(fargs) =
                                tc.pointer("/function/arguments").and_then(|i| i.as_str())
                            {
                                entry.arguments.push_str(fargs);
                            }
                        }
                    }
                }
                if saw_done {
                    break;
                }
            }
        }
    }

    // Assemble tool_calls in the same OpenAI JSON shape the downstream
    // dispatcher expects, ordered by delta index.
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    if !tool_accs.is_empty() {
        let mut indices: Vec<u64> = tool_accs.keys().copied().collect();
        indices.sort();
        for idx in indices {
            let acc = tool_accs.remove(&idx).unwrap();
            if acc.name.is_empty() && acc.arguments.is_empty() {
                continue;
            }
            tool_calls.push(serde_json::json!({
                "id": acc.id,
                "type": if acc.ty.is_empty() { "function".to_string() } else { acc.ty },
                "function": {
                    "name": acc.name,
                    "arguments": acc.arguments,
                }
            }));
        }
    }

    if !tool_calls.is_empty() {
        info!(
            "[llm:{}] {} tool call(s) returned (streamed)",
            provider_name,
            tool_calls.len()
        );
        // Don't leak the reasoning trace into a tool-call turn's content
        return Ok(LlmResult::ToolCalls {
            content,
            tool_calls,
        });
    }

    // Prefer actual content; fall back to reasoning only if content is empty
    // (some reasoning models emit everything into `reasoning` with no `content`).
    let final_text = if !content.is_empty() { content } else { reasoning };

    if final_text.is_empty() {
        return Err(format!(
            "{} returned an empty response — the model may be overloaded or misconfigured.{}",
            provider_name, link_hint
        ));
    }

    let think_re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let cleaned = think_re.replace_all(&final_text, "").trim().to_string();
    Ok(LlmResult::Text(if cleaned.is_empty() {
        final_text
    } else {
        cleaned
    }))
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
        "stream": true,
    });

    // Add tool definitions if provided
    if let Some(tools) = tools {
        payload["tools"] = serde_json::json!(tools);
    }

    debug!("[llm:{}] POST {} (model={}, messages={}, max_tokens={}, stream=true)",
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

    read_sse_response(resp, &provider.name, &link_hint).await
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
        "stream": true,
    });
    if let Some(tools) = tools {
        payload["tools"] = serde_json::json!(tools);
    }

    debug!(
        "[llm:{}] POST {} (capped, max_tokens={}, messages={}, stream=true)",
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

    read_sse_response(resp, &provider.name, &link_hint).await
}
