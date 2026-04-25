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
    /// Most recent rate-limit remaining percent (0-100) parsed from response
    /// headers. 0 = "never observed" → treated as 100 (full) in the accessor.
    rate_limit_remaining_pct: AtomicU32,
    /// Unix epoch seconds of the most recent "hard failure" (4xx/5xx/timeout).
    /// Used as a fast 60s cooldown that skips a provider in ranking without
    /// involving the circuit breaker. Catches structurally-incompatible
    /// providers (groq's 128-tool cap, cerebras-8b's 8k context limit) that
    /// fail on every call but don't accumulate 3 consecutive failures in the
    /// breaker because occasional successes reset its counter.
    last_hard_failure_at: std::sync::atomic::AtomicU64,
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
                rate_limit_remaining_pct: AtomicU32::new(0),
                last_hard_failure_at: std::sync::atomic::AtomicU64::new(0),
            })
        })
        .clone()
}

/// Seconds of "don't even try" cooldown after a hard failure (4xx/5xx/timeout).
/// Kept short so a transient failure doesn't punish a provider for long, but
/// long enough to skip the next probe if the same request shape is tried again
/// in quick succession.
const HARD_FAILURE_COOLDOWN_SECS: u64 = 60;

/// Get current unix timestamp in seconds. Extracted so tests can inject time.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    fn record_rate_limit_pct(&self, pct: u32) {
        // Clamp to 1-100 — 0 is sentinel for "never observed".
        let clamped = pct.clamp(1, 100);
        self.rate_limit_remaining_pct.store(clamped, Ordering::Relaxed);
    }

    /// Rate-limit remaining percent (0-100). Returns 100 when no signal has
    /// been recorded yet so a fresh provider isn't unfairly deprioritized.
    fn rate_limit_pct(&self) -> u32 {
        let v = self.rate_limit_remaining_pct.load(Ordering::Relaxed);
        if v == 0 { 100 } else { v }
    }

    fn record_hard_failure(&self) {
        self.last_hard_failure_at.store(now_epoch_secs(), Ordering::Relaxed);
    }

    fn last_hard_failure_at(&self) -> u64 {
        self.last_hard_failure_at.load(Ordering::Relaxed)
    }
}

/// Parse OpenAI/OpenRouter/Cerebras-style rate-limit headers and return the
/// smallest remaining percent across request + token dimensions. Returns
/// `None` when neither dimension is reported (the provider doesn't surface
/// limits). Shape follows the widely-used `x-ratelimit-{remaining,limit}-{requests,tokens}`
/// convention; providers that use different header names just don't get
/// rate-limit-aware deprioritization and fall back to latency ranking alone.
fn extract_rate_limit_pct(headers: &reqwest::header::HeaderMap) -> Option<u32> {
    let parse = |name: &str| -> Option<f64> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<f64>().ok())
    };

    let mut pct: f64 = 100.0;
    let mut saw_any = false;

    // Minute-window request budget
    for (rem_h, lim_h) in [
        ("x-ratelimit-remaining-requests", "x-ratelimit-limit-requests"),
        ("x-ratelimit-remaining-tokens", "x-ratelimit-limit-tokens"),
    ] {
        if let (Some(r), Some(l)) = (parse(rem_h), parse(lim_h)) {
            if l > 0.0 {
                pct = pct.min(r / l * 100.0);
                saw_any = true;
            }
        }
    }

    if saw_any {
        Some(pct.clamp(0.0, 100.0) as u32)
    } else {
        None
    }
}

/// Pure scoring function for one provider. Smaller = higher priority.
/// Extracted from `ranked_order` so the scoring logic is unit-testable
/// without async or shared global state.
///
/// Two modes controlled by `position_dominant`:
///
/// - **Latency-first (default, `position_dominant=false`):** position acts
///   as a `POSITION_TIEBREAK_MS`-per-slot tiebreaker. A provider N positions
///   later must beat the earlier provider by more than `N * 250 ms` of
///   measured latency to win. This restores real weight to live latency
///   signals that the old scoring ignored.
/// - **Position-dominant (legacy, `SYNTAUR_RANKING_MODE=position`):** config
///   order dominates by 6 orders of magnitude — same behavior as pre-v0.5.2.
///   Kept as an escape hatch in case a latency pick degrades quality.
///
/// `seconds_since_hard_failure = 0` means "no hard failure recorded yet";
/// any non-zero value below `HARD_FAILURE_COOLDOWN_SECS` flags the provider
/// as in cooldown — treated like unavailable so ranking skips it.
///
/// Unavailable providers (circuit Open or in hard-failure cooldown) always
/// lose to any available one, with preserved config-order retry sequence.
fn score_provider(
    position: usize,
    is_available: bool,
    avg_latency_ms: f64,
    total_requests: u64,
    in_flight: u32,
    rate_limit_pct: u32,
    seconds_since_hard_failure: u64,
    position_dominant: bool,
) -> f64 {
    const POSITION_MULT_LEGACY: f64 = 1_000_000.0;
    const POSITION_TIEBREAK_MS: f64 = 250.0;
    const DEFAULT_LATENCY_MS: f64 = 500.0;
    const UNAVAILABLE_BIAS: f64 = 1.0e11;

    let in_hard_cooldown = seconds_since_hard_failure > 0
        && seconds_since_hard_failure < HARD_FAILURE_COOLDOWN_SECS;

    if !is_available || in_hard_cooldown {
        return UNAVAILABLE_BIAS + (position as f64) * POSITION_MULT_LEGACY;
    }

    if position_dominant {
        let base = if avg_latency_ms > 0.0 { avg_latency_ms } else { DEFAULT_LATENCY_MS };
        let penalty = (in_flight as f64) * base.max(DEFAULT_LATENCY_MS) * 0.5;
        return (position as f64) * POSITION_MULT_LEGACY + base + penalty;
    }

    let effective_lat = if avg_latency_ms > 0.0 && total_requests >= 1 {
        avg_latency_ms
    } else {
        DEFAULT_LATENCY_MS
    };

    // Rate-limit aware deprioritization: below 20% remaining, add up to 2000ms
    // of equivalent-latency penalty so a near-exhausted provider loses to a
    // healthier one before it starts returning 429s.
    let rl_penalty = if rate_limit_pct < 20 {
        (20 - rate_limit_pct) as f64 * 100.0
    } else {
        0.0
    };

    let in_flight_penalty = (in_flight as f64) * effective_lat.max(DEFAULT_LATENCY_MS) * 0.5;

    (position as f64) * POSITION_TIEBREAK_MS + effective_lat + rl_penalty + in_flight_penalty
}

fn position_dominant_mode() -> bool {
    std::env::var("SYNTAUR_RANKING_MODE")
        .ok()
        .as_deref()
        == Some("position")
}

// ── Phase 6: persist provider reputation across restarts ────────────────────
//
// Without persistence, every container restart wipes the in-memory latency
// EMA + hard-failure cooldown stored in `PROVIDER_METRICS`. The chain then
// re-discovers that NIM 49B is slow / lmstudio is unreachable / cerebras
// 429s on burst load every cold start, so the first 1-2 requests after each
// deploy pay the full discovery cost.
//
// `ProviderHealthStore` wraps a SQLite connection (the gateway's `index.db`).
// Two operations:
//   - `load_into_globals` — at startup, read every row, populate
//     `PROVIDER_METRICS`. Runs once before any LLM traffic.
//   - `flush_globals` — walk PROVIDER_METRICS, upsert each entry to the table.
//     Called periodically by `spawn_flusher` (default 30s) so writes are
//     batched and don't block LLM hot paths. 30s window is also short enough
//     that a crash at any point loses at most 30s of metric drift.
//
// Schema: `provider_health` table (v68). Keyed by provider name only —
// matches the existing `provider_metrics(name)` keying. Same-endpoint
// different-models providers should use distinct names in config to get
// independent reputation tracking.

use rusqlite::Connection;

pub struct ProviderHealthStore {
    db_path: std::path::PathBuf,
}

impl ProviderHealthStore {
    pub fn new(db_path: std::path::PathBuf) -> Self {
        Self { db_path }
    }

    /// Read every persisted provider row and seed PROVIDER_METRICS so the
    /// first ranked_order computation after startup uses post-restart history
    /// instead of the cold-start defaults. Idempotent — running twice on the
    /// same DB just rewrites the same atomics.
    pub fn load_into_globals(&self) -> Result<usize, String> {
        let conn = Connection::open(&self.db_path)
            .map_err(|e| format!("provider_health: open {}: {}", self.db_path.display(), e))?;
        let mut stmt = conn
            .prepare(
                "SELECT name, avg_latency_ms, total_requests, rate_limit_pct, last_hard_failure_at \
                 FROM provider_health",
            )
            .map_err(|e| format!("provider_health: prepare: {}", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| format!("provider_health: query: {}", e))?;

        let mut loaded = 0usize;
        for row in rows {
            let (name, avg_ms, total, rl_pct, last_fail) = row
                .map_err(|e| format!("provider_health: row: {}", e))?;
            let m = provider_metrics(&name);
            // Convert ms back to us — same encoding `record_latency` writes.
            let ema_us = (avg_ms.max(0.0) * 1000.0) as u64;
            m.latency_ema_us.store(ema_us, Ordering::Relaxed);
            m.total_requests.store(total.max(0) as u64, Ordering::Relaxed);
            m.rate_limit_remaining_pct
                .store(rl_pct.clamp(0, 100) as u32, Ordering::Relaxed);
            m.last_hard_failure_at
                .store(last_fail.max(0) as u64, Ordering::Relaxed);
            loaded += 1;
        }
        Ok(loaded)
    }

    /// Walk PROVIDER_METRICS once and UPSERT every entry. Holds the metrics
    /// map's std::sync::Mutex for the snapshot only — actual SQLite writes
    /// happen with the map unlocked so LLM call paths aren't blocked.
    pub fn flush_globals(&self) -> Result<usize, String> {
        // Snapshot the global map under the std mutex — we copy the per-entry
        // values out and release the lock before doing any I/O.
        let snapshot: Vec<(String, f64, u64, u32, u64)> = {
            let map = PROVIDER_METRICS
                .get_or_init(|| std::sync::Mutex::new(HashMap::new()));
            let guard = map.lock().map_err(|e| format!("metrics map poisoned: {}", e))?;
            guard
                .iter()
                .map(|(name, m)| {
                    (
                        name.clone(),
                        m.avg_latency_ms(),
                        m.total_requests.load(Ordering::Relaxed),
                        m.rate_limit_remaining_pct.load(Ordering::Relaxed),
                        m.last_hard_failure_at.load(Ordering::Relaxed),
                    )
                })
                .collect()
        };

        if snapshot.is_empty() {
            return Ok(0);
        }

        let now = now_epoch_secs() as i64;
        let mut conn = Connection::open(&self.db_path)
            .map_err(|e| format!("provider_health: open: {}", e))?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("provider_health: tx: {}", e))?;
        for (name, avg_ms, total, rl_pct, last_fail) in &snapshot {
            tx.execute(
                "INSERT INTO provider_health \
                   (name, avg_latency_ms, total_requests, rate_limit_pct, last_hard_failure_at, updated_at) \
                   VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(name) DO UPDATE SET \
                   avg_latency_ms = excluded.avg_latency_ms, \
                   total_requests = excluded.total_requests, \
                   rate_limit_pct = excluded.rate_limit_pct, \
                   last_hard_failure_at = excluded.last_hard_failure_at, \
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    name,
                    *avg_ms,
                    *total as i64,
                    *rl_pct as i64,
                    *last_fail as i64,
                    now,
                ],
            )
            .map_err(|e| format!("provider_health: upsert {}: {}", name, e))?;
        }
        tx.commit()
            .map_err(|e| format!("provider_health: commit: {}", e))?;
        Ok(snapshot.len())
    }

    /// Spawn a background flusher that calls `flush_globals` every
    /// `interval_secs` seconds. Cheap on idle (one row per declared provider,
    /// max ~10) — write traffic stays well below SQLite's contention threshold.
    pub fn spawn_flusher(self: Arc<Self>, interval_secs: u64) {
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(interval_secs.max(1));
            loop {
                tokio::time::sleep(interval).await;
                match self.flush_globals() {
                    Ok(n) if n > 0 => debug!("[provider_health] flushed {} entries", n),
                    Ok(_) => {}
                    Err(e) => warn!("[provider_health] flush error: {}", e),
                }
            }
        });
    }
}

/// True when an error string from `call_provider` / `call_provider_capped`
/// indicates a "hard" failure that should put the provider in the 60s
/// skip-in-ranking cooldown. Covers rate limits, auth/billing errors,
/// bad-request responses (tool count over limit, context too long), and 5xx.
/// Timeouts are handled separately via the `was_timeout` flag.
fn is_structural_error(err: &str) -> bool {
    err.contains("HTTP 400")
        || err.contains("HTTP 402")
        || err.contains("HTTP 403")
        || err.contains("HTTP 408")
        || err.contains("HTTP 413")
        || err.contains("HTTP 422")
        || err.contains("HTTP 429")
        || err.contains("rate limited")
        || err.contains("server error")
        || err.contains("stream error")
        || err.contains("empty response")
        || err.contains("request failed")
        || err.contains("Can't reach")
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
    /// Two ranking modes, switched by `SYNTAUR_RANKING_MODE`:
    ///
    /// - **Latency-first (default):** measured latency is the primary signal;
    ///   config position is a `250ms`-per-slot tiebreaker. A provider placed
    ///   N positions later must beat the earlier provider by more than
    ///   `N × 250ms` of observed latency to win. Restores weight to live
    ///   data the pre-v0.5.2 scoring was ignoring (see
    ///   `sessions/2026-04-24-response-time-perf.md` for the diagnosis).
    /// - **Position-dominant (`SYNTAUR_RANKING_MODE=position`):** legacy
    ///   behavior where position multiplies by 1M and dominates latency.
    ///   Keep as an escape hatch if a latency-first pick degrades quality.
    ///
    /// See `score_provider` for the pure scoring function + its tests.
    async fn ranked_order(&self) -> Vec<usize> {
        let position_dominant = position_dominant_mode();
        let now = now_epoch_secs();
        let mut scored: Vec<(usize, f64)> = Vec::with_capacity(self.providers.len());

        for (i, provider) in self.providers.iter().enumerate() {
            let circuit = provider.circuit.lock().await;
            let is_available = circuit.is_available();
            drop(circuit);

            let metrics = provider_metrics(&provider.name);
            let avg_lat = metrics.avg_latency_ms();
            let total = metrics.total_requests.load(Ordering::Relaxed);
            let active = metrics.in_flight.load(Ordering::Relaxed);
            let rl_pct = metrics.rate_limit_pct();
            let last_fail = metrics.last_hard_failure_at();
            let secs_since_fail = if last_fail == 0 { 0 } else { now.saturating_sub(last_fail) };

            let score = score_provider(
                i,
                is_available,
                avg_lat,
                total,
                active,
                rl_pct,
                secs_since_fail,
                position_dominant,
            );

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
                    let is_hard_failure = was_timeout || is_structural_error(&e);
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_failure(was_timeout);
                    }
                    if is_hard_failure {
                        metrics.record_hard_failure();
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
                    let is_hard_failure = was_timeout || is_structural_error(&e);
                    {
                        let mut circuit = provider.circuit.lock().await;
                        circuit.record_failure(was_timeout);
                    }
                    if is_hard_failure {
                        metrics.record_hard_failure();
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

    if let Some(pct) = extract_rate_limit_pct(resp.headers()) {
        provider_metrics(&provider.name).record_rate_limit_pct(pct);
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

    if let Some(pct) = extract_rate_limit_pct(resp.headers()) {
        provider_metrics(&provider.name).record_rate_limit_pct(pct);
    }

    read_sse_response(resp, &provider.name, &link_hint).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    /// Helper to score a default "healthy, well-measured, never-failed" provider at position `i`.
    fn score_at(pos: usize, lat_ms: f64) -> f64 {
        score_provider(pos, true, lat_ms, 10, 0, 100, 0, false)
    }

    #[test]
    fn latency_first_picks_faster_provider_at_later_position() {
        // Real-world case from the 2026-04-24 baseline:
        //   position 0 openrouter @ 2857ms  vs  position 4 cerebras @ 555ms.
        // Under legacy scoring openrouter wins by 6 orders of magnitude; under
        // latency-first it should lose by ~1300 ms worth of score.
        let or_score = score_at(0, 2857.0);
        let cer_score = score_at(4, 555.0);
        assert!(cer_score < or_score,
            "cerebras (score={}) should win over openrouter (score={}) in latency-first mode",
            cer_score, or_score);
        // Sanity: cerebras margin is 2857 - 555 - 4*250 = 1302 ms.
        assert!((or_score - cer_score - 1302.0).abs() < 1.0);
    }

    #[test]
    fn tiebreaker_prefers_earlier_position_when_latency_equal() {
        let s0 = score_at(0, 500.0);
        let s1 = score_at(1, 500.0);
        assert!(s0 < s1, "equal latency: position 0 wins");
    }

    #[test]
    fn earlier_position_still_wins_when_latency_diff_below_tiebreak() {
        // Position 0 @ 600ms vs position 1 @ 500ms. Diff = 100ms, tiebreak = 250ms.
        let s0 = score_at(0, 600.0);
        let s1 = score_at(1, 500.0);
        assert!(s0 < s1, "100ms latency diff < 250ms tiebreak; position 0 wins");
    }

    #[test]
    fn later_position_wins_when_latency_diff_exceeds_tiebreak() {
        // Position 0 @ 800ms vs position 1 @ 400ms. Diff = 400ms > 250ms.
        let s0 = score_at(0, 800.0);
        let s1 = score_at(1, 400.0);
        assert!(s1 < s0, "400ms latency win > 250ms tiebreak; position 1 wins");
    }

    #[test]
    fn unavailable_provider_always_loses_to_available() {
        let unavailable = score_provider(0, false, 10.0, 100, 0, 100, 0, false);
        let very_slow_available = score_provider(6, true, 59_000.0, 100, 3, 100, 0, false);
        assert!(
            very_slow_available < unavailable,
            "even a 59s available provider must outrank any unavailable one"
        );
    }

    #[test]
    fn unavailable_providers_preserve_config_order() {
        let u0 = score_provider(0, false, 0.0, 0, 0, 100, 0, false);
        let u3 = score_provider(3, false, 0.0, 0, 0, 100, 0, false);
        assert!(u0 < u3, "unavailable retry order must follow config order");
    }

    #[test]
    fn rate_limit_penalty_only_kicks_in_below_20_percent() {
        let healthy = score_provider(0, true, 500.0, 10, 0, 50, 0, false);
        let near_limit = score_provider(0, true, 500.0, 10, 0, 15, 0, false);
        let at_limit = score_provider(0, true, 500.0, 10, 0, 0, 0, false);
        assert_eq!(healthy, 500.0, "no penalty at 50% remaining");
        assert_eq!(near_limit, 500.0 + 500.0, "5pp below threshold → +500ms penalty");
        assert_eq!(at_limit, 500.0 + 2000.0, "0% remaining → +2000ms penalty");
    }

    #[test]
    fn untested_provider_gets_default_latency_not_zero() {
        // n=0 avg=0.0 shouldn't score as a 0ms superstar.
        let untested = score_provider(0, true, 0.0, 0, 0, 100, 0, false);
        let tested_fast = score_provider(0, true, 100.0, 5, 0, 100, 0, false);
        assert_eq!(untested, 500.0, "untested provider uses 500ms default");
        assert!(tested_fast < untested, "a proven-fast provider should beat an untested one");
    }

    #[test]
    fn in_flight_penalty_discourages_piling_onto_busy_provider() {
        let idle = score_provider(0, true, 500.0, 10, 0, 100, 0, false);
        let one_in_flight = score_provider(0, true, 500.0, 10, 1, 100, 0, false);
        assert!(one_in_flight > idle, "in-flight request adds penalty");
        assert_eq!(one_in_flight - idle, 250.0);
    }

    #[test]
    fn position_dominant_mode_reverts_to_legacy_scoring() {
        let or_score = score_provider(0, true, 2857.0, 10, 0, 100, 0, true);
        let cer_score = score_provider(4, true, 555.0, 10, 0, 100, 0, true);
        assert!(or_score < cer_score,
            "legacy mode: position dominates even when latency says otherwise");
    }

    #[test]
    fn rate_limit_headers_parsed_from_openai_style() {
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("25"));
        h.insert("x-ratelimit-limit-requests", HeaderValue::from_static("100"));
        assert_eq!(extract_rate_limit_pct(&h), Some(25));
    }

    #[test]
    fn rate_limit_takes_min_of_request_and_token_dimensions() {
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("80"));
        h.insert("x-ratelimit-limit-requests", HeaderValue::from_static("100"));
        h.insert("x-ratelimit-remaining-tokens", HeaderValue::from_static("1000"));
        h.insert("x-ratelimit-limit-tokens", HeaderValue::from_static("10000"));
        assert_eq!(extract_rate_limit_pct(&h), Some(10));
    }

    #[test]
    fn rate_limit_returns_none_when_headers_missing() {
        let h = HeaderMap::new();
        assert_eq!(extract_rate_limit_pct(&h), None);
    }

    #[test]
    fn rate_limit_handles_unparseable_and_zero_limit() {
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("not a number"));
        h.insert("x-ratelimit-limit-requests", HeaderValue::from_static("0"));
        assert_eq!(extract_rate_limit_pct(&h), None);
    }

    #[test]
    fn rate_limit_clamps_above_100() {
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("200"));
        h.insert("x-ratelimit-limit-requests", HeaderValue::from_static("100"));
        assert_eq!(extract_rate_limit_pct(&h), Some(100));
    }

    // ── Hard-failure cooldown ────────────────────────────────────────────

    #[test]
    fn provider_in_hard_cooldown_loses_to_slower_healthy_one() {
        // Fast provider at position 4 with recent 400/429, vs. slow provider
        // at position 0 with no recent failures. The cooldown should win.
        //   secs_since_fail=10 → still in 60s cooldown → treat as unavailable.
        let in_cooldown_fast = score_provider(4, true, 100.0, 10, 0, 100, 10, false);
        let slow_but_healthy = score_provider(0, true, 5000.0, 10, 0, 100, 0, false);
        assert!(
            slow_but_healthy < in_cooldown_fast,
            "cooldown treats 4xx'd provider as unavailable; slow-but-healthy wins"
        );
    }

    #[test]
    fn hard_cooldown_expires_after_60s() {
        // 61s after the hard failure → back to normal ranking.
        let post_cooldown = score_provider(4, true, 100.0, 10, 0, 100, 61, false);
        let slow = score_provider(0, true, 5000.0, 10, 0, 100, 0, false);
        assert!(post_cooldown < slow, "after 60s cooldown, latency-first takes over again");
    }

    #[test]
    fn cooldown_respects_unavailable_bias_ordering() {
        // Unavailable + in-cooldown both bubble up to the unavailable tier.
        // Within that tier they preserve config-order.
        let cooldown_pos0 = score_provider(0, true, 100.0, 10, 0, 100, 5, false);
        let cooldown_pos3 = score_provider(3, true, 100.0, 10, 0, 100, 5, false);
        assert!(cooldown_pos0 < cooldown_pos3,
            "in-cooldown providers still prefer earlier config positions");
    }

    #[test]
    fn zero_seconds_since_failure_means_never_failed_not_just_failed() {
        // secs_since_fail=0 is the sentinel for "no hard failure recorded".
        // A provider with secs=0 must be treated as healthy, not in cooldown.
        let never_failed = score_provider(0, true, 500.0, 10, 0, 100, 0, false);
        let healthy_reference = score_provider(0, true, 500.0, 10, 0, 100, 999, false);
        assert_eq!(never_failed, healthy_reference,
            "secs=0 is 'never failed' sentinel, same score as a long-ago failure");
    }

    // ── Structural error classifier ──────────────────────────────────────

    #[test]
    fn is_structural_error_catches_http_400_series() {
        assert!(is_structural_error("groq returned HTTP 400 — 'tools' : maximum 128"));
        assert!(is_structural_error("cerebras returned HTTP 402 — billing"));
        assert!(is_structural_error("provider returned HTTP 413 — too large"));
        assert!(is_structural_error("provider returned HTTP 422 — unprocessable"));
        assert!(is_structural_error("provider returned HTTP 429 — too many requests"));
    }

    #[test]
    fn is_structural_error_catches_rate_limit_and_server_errors() {
        assert!(is_structural_error("cerebras is rate limited (HTTP 429)"));
        assert!(is_structural_error("openrouter returned server error (502)"));
        assert!(is_structural_error("stream error: read ECONNRESET"));
        assert!(is_structural_error("openrouter returned an empty response"));
    }

    #[test]
    fn is_structural_error_catches_connection_failures() {
        assert!(is_structural_error("Can't reach lmstudio — check the server"));
        assert!(is_structural_error("openrouter request failed: dns resolution"));
    }

    #[test]
    fn is_structural_error_does_not_flag_success_strings() {
        // Paranoid check: common success-path phrases must NOT trigger a cooldown.
        assert!(!is_structural_error("text 4 chars"));
        assert!(!is_structural_error("2 tool calls"));
        assert!(!is_structural_error(""));
    }

    // ── Phase 6: ProviderHealthStore round-trip ──────────────────────────

    /// Apply just the v68 migration to a fresh in-memory connection — keeps
    /// these tests isolated from the full schema migration ladder so a future
    /// schema change doesn't break this test suite. Mirrors the provider_health
    /// CREATE TABLE in `index/schema.rs`.
    fn create_provider_health_table(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_health (
                name TEXT PRIMARY KEY,
                avg_latency_ms REAL NOT NULL DEFAULT 0,
                total_requests INTEGER NOT NULL DEFAULT 0,
                rate_limit_pct INTEGER NOT NULL DEFAULT 0,
                last_hard_failure_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
    }

    /// Reset a provider's globals to known values without going through the
    /// public API (those public methods don't expose all the atomics directly).
    fn reset_metrics(name: &str, avg_ms: f64, total: u64, rl_pct: u32, last_fail: u64) {
        let m = provider_metrics(name);
        m.latency_ema_us
            .store((avg_ms * 1000.0) as u64, Ordering::Relaxed);
        m.total_requests.store(total, Ordering::Relaxed);
        m.rate_limit_remaining_pct.store(rl_pct, Ordering::Relaxed);
        m.last_hard_failure_at.store(last_fail, Ordering::Relaxed);
        m.in_flight.store(0, Ordering::Relaxed);
    }

    #[test]
    fn provider_health_store_flush_then_load_round_trip() {
        // Use a unique provider name so the global PROVIDER_METRICS map doesn't
        // collide with other tests running in parallel.
        let name = "test-roundtrip-provider-001";
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        create_provider_health_table(&path);

        // Seed: pretend this provider has 7 successful calls averaging 1234ms,
        // 35% rate-limit headroom, and a hard failure 1500s ago.
        reset_metrics(name, 1234.0, 7, 35, 1_700_000_000);

        let store = ProviderHealthStore::new(path.clone());
        let n = store.flush_globals().unwrap();
        assert!(n >= 1, "flush should have written at least our seed row");

        // Wipe the in-memory globals to simulate a process restart.
        reset_metrics(name, 0.0, 0, 0, 0);

        // Load back from DB.
        let loaded = store.load_into_globals().unwrap();
        assert!(loaded >= 1, "load should have rehydrated at least our row");

        let m = provider_metrics(name);
        // EMA roundtrip: 1234.0 ms encoded as 1234000 us.
        assert_eq!(m.latency_ema_us.load(Ordering::Relaxed), 1_234_000);
        assert_eq!(m.total_requests.load(Ordering::Relaxed), 7);
        assert_eq!(m.rate_limit_remaining_pct.load(Ordering::Relaxed), 35);
        assert_eq!(m.last_hard_failure_at.load(Ordering::Relaxed), 1_700_000_000);
    }

    #[test]
    fn provider_health_store_flush_is_idempotent_upsert() {
        // Multiple flushes with no changes between them must not duplicate rows
        // or change values. The ON CONFLICT(name) DO UPDATE clause carries this.
        let name = "test-idempotent-provider-002";
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        create_provider_health_table(&path);
        reset_metrics(name, 555.0, 10, 80, 0);

        let store = ProviderHealthStore::new(path.clone());
        store.flush_globals().unwrap();
        store.flush_globals().unwrap();
        store.flush_globals().unwrap();

        let conn = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_health WHERE name = ?",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "three flushes of same name produce exactly one row");
    }

    #[test]
    fn provider_health_store_flush_picks_up_metric_changes() {
        // Sequence: flush v1 → mutate metrics → flush v2 → load → verify v2 won.
        let name = "test-update-provider-003";
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        create_provider_health_table(&path);

        reset_metrics(name, 1000.0, 5, 100, 0);
        let store = ProviderHealthStore::new(path.clone());
        store.flush_globals().unwrap();

        // Simulate further activity.
        reset_metrics(name, 2500.0, 12, 40, 1_700_000_000);
        store.flush_globals().unwrap();

        // Reset in-memory and load — should see v2 values.
        reset_metrics(name, 0.0, 0, 0, 0);
        store.load_into_globals().unwrap();
        let m = provider_metrics(name);
        assert_eq!(m.latency_ema_us.load(Ordering::Relaxed), 2_500_000);
        assert_eq!(m.total_requests.load(Ordering::Relaxed), 12);
        assert_eq!(m.rate_limit_remaining_pct.load(Ordering::Relaxed), 40);
        assert_eq!(m.last_hard_failure_at.load(Ordering::Relaxed), 1_700_000_000);
    }

    #[test]
    fn provider_health_store_load_empty_db_returns_zero_not_error() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        create_provider_health_table(&path);

        let store = ProviderHealthStore::new(path);
        let n = store.load_into_globals().unwrap();
        assert_eq!(n, 0, "empty provider_health table loads 0 entries cleanly");
    }

    #[test]
    fn provider_health_store_load_missing_table_returns_error() {
        // If migration hasn't run (no provider_health table), load surfaces an
        // error so the caller can choose to log + continue with cold metrics.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // Note: NOT calling create_provider_health_table — DB exists but no schema.
        let store = ProviderHealthStore::new(path);
        let result = store.load_into_globals();
        assert!(result.is_err(), "missing table → error, caller decides to continue");
    }
}
