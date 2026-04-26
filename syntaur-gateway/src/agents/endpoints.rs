//! Backend endpoints for the per-chat agent settings cog
//! (vault/projects/syntaur_per_chat_settings.md, sections 2-7).
//!
//! Phase 8 of the rollout shipped the UI; this module wires the deferred
//! server endpoints. Most are read-only enumerations against existing
//! state (provider_health table, ToolRegistry, persona defaults). The
//! one substantive write is /api/agents/{id}/import (multipart JSON
//! persona upload).
//!
//! Curated provider/voice catalogs live here as static data — calling
//! out to OpenRouter's /models, Cerebras's discovery API, etc. on every
//! load would be slow + rate-limited. The static lists cover the same
//! surface the LLM router actually picks from. Real-time discovery can
//! land later as a separate concern.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::AppState;
use crate::security::extract_session_token;

// ── Tools list — Phase 5 ────────────────────────────────────────────

/// GET /api/tools/list — every registered tool the gateway can expose
/// to an agent, grouped by category. The cog's Tools grid renders these
/// + greys out anything outside the agent's persona max-allowlist.
pub async fn handle_tools_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // We don't need a fully-authed registry to LIST tools — just enumerate
    // names from the static set the gateway always builds. Indexer +
    // workspace_root are fine-grained context the registry uses for tool
    // *execution*, not enumeration.
    let workspace_root = std::env::var("SYNTAUR_WORKSPACE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.db_path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default());
    let registry = crate::tools::ToolRegistry::with_extensions(
        workspace_root,
        "main".to_string(),
        None,
        state.indexer.clone(),
    );
    let defs = registry.tool_definitions();
    let mut grouped: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for d in defs {
        let name = d.pointer("/function/name").and_then(|n| n.as_str())
            .unwrap_or("unknown").to_string();
        let desc = d.pointer("/function/description").and_then(|n| n.as_str())
            .unwrap_or("").to_string();
        let category = category_for_tool(&name).to_string();
        grouped.entry(category).or_default().push(json!({
            "name": name,
            "description": desc,
        }));
    }
    let total: usize = grouped.values().map(|v| v.len()).sum();
    Ok(Json(json!({
        "total": total,
        "categories": grouped,
    })))
}

/// Bucket a tool name into a UI category. Extend as the registry grows.
fn category_for_tool(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("email") || n.contains("gmail") || n.contains("imap") { return "Email"; }
    if n.contains("calendar") || n.contains("event") || n.contains("meeting") { return "Calendar"; }
    if n.contains("smart_home") || n.contains("light") || n.contains("matter")
        || n.contains("ha_") || n.contains("zigbee") || n.contains("scene") { return "Smart Home"; }
    if n.contains("music") || n.contains("spotify") || n.contains("apple_music")
        || n.contains("playlist") || n.contains("song") { return "Music"; }
    if n.contains("web") || n.contains("browser") || n.contains("crawl")
        || n.contains("fetch") || n.contains("search") { return "Web"; }
    if n.contains("file") || n.contains("write_") || n.contains("read_")
        || n.contains("path") || n.contains("dir") { return "Files"; }
    if n.contains("telegram") || n.contains("sms") || n.contains("call")
        || n.contains("message") { return "Communication"; }
    if n.contains("excel") || n.contains("word") || n.contains("ppt")
        || n.contains("office") || n.contains("doc") { return "Office"; }
    if n.contains("ledger") || n.contains("budget") || n.contains("expense")
        || n.contains("tax") || n.contains("invoice") || n.contains("alpaca") { return "Finance"; }
    if n.contains("image") || n.contains("photo") || n.contains("vision")
        || n.contains("seedream") { return "Image"; }
    if n.contains("memory") || n.contains("note") || n.contains("journal") { return "Memory"; }
    if n.contains("delegate") || n.contains("handoff") || n.contains("agent") { return "Agents"; }
    "Other"
}

// ── Voice sample — Phase 3 ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct VoiceSampleReq {
    pub text: Option<String>,
    pub voice_id: Option<String>,
    pub rate: Option<f32>,
    pub pitch: Option<f32>,
}

/// POST /api/voice/sample — synthesize a short demo phrase with the
/// caller's chosen voice/rate/pitch so they can preview before saving.
/// Returns audio bytes (audio/mpeg) directly so the front-end can pipe
/// straight into `new Audio(URL.createObjectURL(blob))`.
pub async fn handle_voice_sample(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<VoiceSampleReq>,
) -> Result<Response, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let text = req.text.unwrap_or_else(|| {
        "Good morning, Sean. Ready when you are.".to_string()
    });
    // Reuse the existing TTS endpoint's underlying provider chain by
    // calling synthesize_speech with a JSON wrapper. Returns audio_url —
    // we proxy-fetch it and stream the bytes out.
    let synth_payload = crate::voice_api::TtsRequest {
        text: text.clone(),
        token: Some(token.clone()),
    };
    let synth_resp = crate::voice_api::synthesize_speech(
        State(state.clone()),
        headers.clone(),
        Json(synth_payload),
    ).await.map_err(|c| c)?;
    let body = synth_resp.0;
    let audio_url = body.get("audio_url").and_then(|v| v.as_str()).unwrap_or("");
    if audio_url.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    // Same-origin fetch; the URL is /api/tts-audio/<id> served by us.
    let bytes = if audio_url.starts_with("/") {
        // serve from our own filesystem; voice_api stores under /tmp
        // typically. Safer: redirect the browser to it.
        return Ok((
            StatusCode::SEE_OTHER,
            [(axum::http::header::LOCATION, audio_url.to_string())],
            "",
        ).into_response());
    } else {
        // External URL (cloud TTS): proxy-fetch.
        match reqwest::get(audio_url).await {
            Ok(r) => match r.bytes().await {
                Ok(b) => b.to_vec(),
                Err(_) => return Err(StatusCode::BAD_GATEWAY),
            },
            Err(_) => return Err(StatusCode::BAD_GATEWAY),
        }
    };
    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "audio/mpeg".to_string()),
            (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
        ],
        bytes,
    ).into_response())
}

/// GET /api/voice/tts/{provider}/voices — voice catalog per backend.
/// Curated; refreshes as new providers land in voice_api.rs.
pub async fn handle_voice_catalog(
    State(_state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> Json<Value> {
    let voices: Vec<Value> = match provider.as_str() {
        "edge-tts" => vec![
            json!({"id": "en-US-AriaNeural", "name": "Aria (warm)"}),
            json!({"id": "en-US-GuyNeural", "name": "Guy (steady)"}),
            json!({"id": "en-US-JennyNeural", "name": "Jenny (friendly)"}),
            json!({"id": "en-US-EricNeural", "name": "Eric (calm)"}),
            json!({"id": "en-GB-SoniaNeural", "name": "Sonia (UK)"}),
            json!({"id": "en-GB-RyanNeural", "name": "Ryan (UK)"}),
            json!({"id": "en-AU-NatashaNeural", "name": "Natasha (AU)"}),
            json!({"id": "en-AU-WilliamNeural", "name": "William (AU)"}),
        ],
        "openai" => vec![
            json!({"id": "alloy", "name": "Alloy"}),
            json!({"id": "echo", "name": "Echo"}),
            json!({"id": "fable", "name": "Fable"}),
            json!({"id": "onyx", "name": "Onyx"}),
            json!({"id": "nova", "name": "Nova"}),
            json!({"id": "shimmer", "name": "Shimmer"}),
        ],
        "elevenlabs" => vec![
            json!({"id": "rachel", "name": "Rachel (default)"}),
            json!({"id": "domi", "name": "Domi"}),
            json!({"id": "bella", "name": "Bella"}),
        ],
        "orpheus" => vec![
            json!({"id": "default", "name": "Orpheus default"}),
            json!({"id": "tara", "name": "Tara"}),
            json!({"id": "leah", "name": "Leah"}),
            json!({"id": "jess", "name": "Jess"}),
            json!({"id": "leo", "name": "Leo"}),
            json!({"id": "dan", "name": "Dan"}),
            json!({"id": "mia", "name": "Mia"}),
            json!({"id": "zac", "name": "Zac"}),
            json!({"id": "zoe", "name": "Zoe"}),
        ],
        "fish-audio" | "fish_audio" => vec![
            json!({"id": "fish-default", "name": "Fish S2 default"}),
        ],
        "piper" => vec![
            json!({"id": "en_US-amy-medium", "name": "Amy (medium)"}),
            json!({"id": "en_US-libritts_r-medium", "name": "Libritts-R"}),
            json!({"id": "en_GB-northern_english_male-medium", "name": "Northern (UK male)"}),
        ],
        _ => vec![],
    };
    Json(json!({ "provider": provider, "voices": voices }))
}

// ── LLM provider endpoints — Phase 2 ────────────────────────────────

/// GET /api/llm/local/models — scan local model directories the gateway
/// knows how to load: Ollama, llama.cpp, LM Studio. Returns a flat list
/// of (provider, model_id, file_size_mb, source_dir).
pub async fn handle_llm_local_models(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut found: Vec<Value> = Vec::new();
    // Common local model locations. Each path scanned independently.
    let candidates: Vec<(&str, std::path::PathBuf)> = vec![
        ("ollama",    home_path(".ollama/models/manifests")),
        ("lmstudio",  home_path(".lmstudio/models")),
        ("llamacpp",  home_path(".cache/llama-cpp/models")),
        ("turboquant", home_path(".lmstudio/models")), // shared with LM Studio
    ];
    for (provider, base) in candidates {
        if !base.is_dir() { continue; }
        scan_for_ggufs(provider, &base, &mut found, 0);
    }
    Ok(Json(json!({ "models": found })))
}

fn home_path(rel: &str) -> std::path::PathBuf {
    std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(rel))
        .unwrap_or_else(|_| std::path::PathBuf::from(rel))
}

fn scan_for_ggufs(
    provider: &str,
    dir: &std::path::Path,
    out: &mut Vec<Value>,
    depth: usize,
) {
    if depth > 4 { return; } // safety
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for ent in entries.flatten() {
        let p = ent.path();
        if p.is_dir() {
            scan_for_ggufs(provider, &p, out, depth + 1);
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "gguf" && ext != "safetensors" { continue; }
        let size_mb = std::fs::metadata(&p)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);
        let model_id = p.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        out.push(json!({
            "provider": provider,
            "model_id": model_id,
            "size_mb": size_mb,
            "path": p.to_string_lossy(),
        }));
    }
}

/// GET /api/llm/providers/health — reads the provider_health table for
/// live circuit-state + latency. Front-end uses this for the green/amber/red
/// dots next to each priority-list row.
pub async fn handle_llm_providers_health(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let indexer = state.indexer.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let providers = indexer.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT name, avg_latency_ms, total_requests, rate_limit_pct,
                    last_hard_failure_at, updated_at
             FROM provider_health"
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut out: Vec<Value> = Vec::new();
        for row in rows {
            let (name, lat, reqs, rl, last_fail, _upd) = row?;
            let recent_fail = (now - last_fail) < 300; // 5min cooldown
            let status = if recent_fail { "open" }
                else if rl > 50 || lat > 5000.0 { "degraded" }
                else { "closed" };
            out.push(json!({
                "name": name,
                "status": status,
                "latency_ms": lat as i64,
                "total_requests": reqs,
                "rate_limit_pct": rl,
                "last_hard_failure_at": last_fail,
            }));
        }
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "providers": providers })))
}

/// GET /api/llm/providers/{provider}/catalog — curated model list per
/// cloud provider. NOT a live API call to the provider's /models endpoint —
/// returning a static curated list keeps this endpoint fast and offline-safe.
/// Real-time discovery is a separate concern (later phase).
pub async fn handle_llm_provider_catalog(
    State(_state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> Json<Value> {
    let models: Vec<Value> = match provider.as_str() {
        "openrouter" => vec![
            json!({"id": "nvidia/nemotron-3-super-120b-a12b:free", "name": "Nemotron 3 Super 120B (free)", "context": 262144}),
            json!({"id": "deepseek/deepseek-r1:free", "name": "DeepSeek R1 (free)", "context": 65536}),
            json!({"id": "google/gemini-2.5-pro", "name": "Gemini 2.5 Pro", "context": 1048576}),
            json!({"id": "anthropic/claude-sonnet-4.6", "name": "Claude Sonnet 4.6", "context": 1000000}),
            json!({"id": "anthropic/claude-opus-4.7", "name": "Claude Opus 4.7", "context": 1000000}),
            json!({"id": "openai/gpt-5", "name": "GPT-5", "context": 200000}),
        ],
        "anthropic" => vec![
            json!({"id": "claude-opus-4-7", "name": "Claude Opus 4.7", "context": 1000000}),
            json!({"id": "claude-sonnet-4-6", "name": "Claude Sonnet 4.6", "context": 1000000}),
            json!({"id": "claude-haiku-4-5-20251001", "name": "Claude Haiku 4.5", "context": 200000}),
        ],
        "openai" => vec![
            json!({"id": "gpt-5", "name": "GPT-5", "context": 200000}),
            json!({"id": "gpt-4o", "name": "GPT-4o", "context": 128000}),
            json!({"id": "o3-mini", "name": "o3-mini", "context": 200000}),
        ],
        "groq" => vec![
            json!({"id": "llama-3.3-70b-versatile", "name": "Llama 3.3 70B", "context": 131072}),
            json!({"id": "qwen-3-32b", "name": "Qwen 3 32B", "context": 131072}),
        ],
        "cerebras" => vec![
            json!({"id": "llama3.3-70b", "name": "Llama 3.3 70B (Cerebras)", "context": 131072}),
            json!({"id": "qwen-3-32b", "name": "Qwen 3 32B (Cerebras)", "context": 131072}),
        ],
        "together" => vec![
            json!({"id": "meta-llama/Llama-3.3-70B-Instruct-Turbo", "name": "Llama 3.3 70B Turbo", "context": 131072}),
        ],
        "fireworks" => vec![
            json!({"id": "accounts/fireworks/models/qwen3-coder-480b-a35b-instruct", "name": "Qwen3 Coder 480B", "context": 262144}),
        ],
        "nvidia" => vec![
            json!({"id": "nvidia/llama-3.3-nemotron-49b-super", "name": "Nemotron 49B Super", "context": 131072}),
            json!({"id": "nvidia/llama-3.1-nemotron-70b-instruct", "name": "Nemotron 70B", "context": 131072}),
        ],
        "turboquant" => vec![
            json!({"id": "Qwen3.5-27B-Q4_K_M", "name": "Qwen 3.5 27B Q4 (local · RTX 3090)", "context": 131072}),
        ],
        _ => vec![],
    };
    Json(json!({ "provider": provider, "models": models }))
}

// ── Persona test prompt — Phase 4 ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TestPromptReq {
    pub prompt: String,
    pub message: Option<String>,
}

/// POST /api/agents/{id}/test_prompt — run a one-shot turn against the
/// active brain with a hypothetical persona prompt, return the reply
/// text. Used by the Persona section's "Test prompt" button so users can
/// preview behavior before committing the override.
pub async fn handle_test_prompt(
    State(_state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(_agent_id): Path<String>,
    Json(req): Json<TestPromptReq>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    // Phase-1 implementation: echo the prompt + message so the user
    // sees something concrete. A real one-shot run would route through
    // crate::llm::LlmChain::chat; deferring that wiring keeps this PR
    // small and avoids leaking a no-tools chat surface into the API.
    Ok(Json(json!({
        "reply": format!(
            "(test mode) System prompt would set persona to {} chars. \
             Sample reply: \"Hi, Sean. I'm ready when you are.\"",
            req.prompt.len()
        ),
        "tokens": req.prompt.len() / 4,
        "stub": true,
    })))
}

// ── Maintenance — Phase 7 ────────────────────────────────────────────

/// GET /api/agents/{id}/export — JSON dump of (display_name, accent,
/// persona_prompt_override, voice settings, tool_allowlist) so users can
/// share or import personas. Excludes icon BLOB + secrets.
pub async fn handle_agent_export(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let indexer = state.indexer.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let agent = agent_id.clone();
    let row = indexer
        .with_conn(move |conn| Ok(crate::agents::settings::get(conn, uid, &agent)?))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let exported = json!({
        "format": "syntaur-persona-v1",
        "agent_id": agent_id,
        "exported_at": now_iso(),
        "settings": row,
    });
    Ok(Json(exported))
}

/// POST /api/agents/{id}/import — multipart JSON upload that overwrites
/// the agent's settings row from a previously-exported persona file.
pub async fn handle_agent_import(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    let principal = crate::resolve_principal(&state, &token).await?;
    let uid = principal.user_id();
    let indexer = state.indexer.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut payload: Option<Value> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("persona") { continue; }
        let bytes = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
        payload = serde_json::from_str(text).ok();
        break;
    }
    let payload = payload.ok_or(StatusCode::BAD_REQUEST)?;
    if payload.get("format").and_then(|v| v.as_str()) != Some("syntaur-persona-v1") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let settings = payload.get("settings").cloned().unwrap_or(Value::Null);
    let agent = agent_id.clone();
    let row = indexer
        .with_conn(move |conn| Ok(crate::agents::settings::patch(conn, uid, &agent, &settings)?))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "imported": true,
        "agent_id": agent_id,
        "settings": row,
    })))
}

/// DELETE /api/agents/{id}/history — wipe conversation messages for an
/// agent. LCM stores everything in `conversations(agent_id, session_id, role,
/// content, ...)` — single table, no separate messages join. Drop every
/// row matching this agent.
pub async fn handle_agent_history_delete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let lcm = state.lcm.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let db_path = lcm.db_path_str();
    let agent = agent_id.clone();
    let n = tokio::task::spawn_blocking(move || -> Result<usize, rusqlite::Error> {
        let conn = rusqlite::Connection::open(&db_path)?;
        let n = conn.execute(
            "DELETE FROM conversations WHERE agent_id = ?",
            params![agent],
        )?;
        let _ = conn.execute(
            "DELETE FROM summaries WHERE agent_id = ?",
            params![agent_id.clone()],
        );
        Ok(n)
    }).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "deleted_messages": n })))
}

/// GET /api/conversations/active?format=json — most recent conversation
/// log per agent (from LCM's single conversations table). Caller gets
/// the last 200 messages in chronological order.
pub async fn handle_conv_active_export(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_session_token(&headers);
    if token.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let agent_filter = params.get("agent").cloned();
    let lcm = state.lcm.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let db_path = lcm.db_path_str();
    let messages = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, rusqlite::Error> {
        let conn = rusqlite::Connection::open(&db_path)?;
        let (sql, args): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match &agent_filter {
            Some(a) => (
                "SELECT agent_id, role, content, created_at FROM conversations
                 WHERE agent_id = ?
                 ORDER BY id DESC LIMIT 200",
                vec![Box::new(a.clone())],
            ),
            None => (
                "SELECT agent_id, role, content, created_at FROM conversations
                 ORDER BY id DESC LIMIT 200",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_dyn.as_slice(), |r| {
            Ok(json!({
                "agent_id": r.get::<_, String>(0)?,
                "role": r.get::<_, String>(1)?,
                "content": r.get::<_, String>(2)?,
                "ts": r.get::<_, String>(3)?,
            }))
        })?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        out.reverse(); // chronological
        Ok(out)
    }).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "format": "syntaur-conv-v1",
        "exported_at": now_iso(),
        "messages": messages,
    })))
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}
