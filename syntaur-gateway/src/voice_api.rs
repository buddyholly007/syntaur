//! HTTP API endpoints for the Voice Journal module.
//!
//! - GET /api/journal?date=YYYY-MM-DD — get journal for a date (default today)
//! - GET /api/journal/dates — list available journal dates
//! - GET /api/journal/search?q=term&max=5 — search across journals
//! - GET /api/journal/sessions?limit=20 — list recording sessions

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use log::info;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;
use crate::tools::voice_journal;
use crate::voice::satellite_client;

/// Check voice auth if configured. Returns Ok(()) or 401.
async fn require_voice_auth(state: &AppState, token: &str) -> Result<(), StatusCode> {
    if !state.config.security.require_voice_auth {
        return Ok(());
    }
    crate::resolve_principal(state, token).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct JournalQuery {
    pub date: Option<String>,
    pub token: Option<String>,
}

pub async fn get_journal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<JournalQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let date = q.date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let dir = voice_journal::config().data_dir().join("journal");
    let path = dir.join(format!("{}.md", date));

    Ok(match std::fs::read_to_string(&path) {
        Ok(content) => Json(json!({
            "date": date,
            "content": content,
            "entries": content.lines()
                .filter(|l| l.starts_with("**"))
                .count(),
        })),
        Err(_) => Json(json!({
            "date": date,
            "content": null,
            "entries": 0,
        })),
    })
}

#[derive(Deserialize)]
pub struct DatesQuery {
    pub token: Option<String>,
}

pub async fn get_journal_dates(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<DatesQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let dir = voice_journal::config().data_dir().join("journal");
    let mut dates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
    }
    dates.sort();
    dates.reverse();
    Ok(Json(json!({ "dates": dates })))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub max: Option<usize>,
    pub token: Option<String>,
}

pub async fn search_journal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let max = q.max.unwrap_or(5);
    let dir = voice_journal::config().data_dir().join("journal");
    let query_lower = q.q.to_lowercase();
    let mut results = Vec::new();

    let mut dates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
    }
    dates.sort();
    dates.reverse();

    for date in &dates {
        let path = dir.join(format!("{}.md", date));
        if let Ok(content) = std::fs::read_to_string(&path) {
            let matches: Vec<&str> = content.lines()
                .filter(|l| l.to_lowercase().contains(&query_lower))
                .collect();
            if !matches.is_empty() {
                results.push(json!({
                    "date": date,
                    "matches": matches,
                    "count": matches.len(),
                }));
            }
            if results.len() >= max {
                break;
            }
        }
    }

    Ok(Json(json!({
        "query": q.q,
        "results": results,
        "total_days": results.len(),
    })))
}

#[derive(Deserialize)]
pub struct SessionsQuery {
    pub limit: Option<usize>,
    pub token: Option<String>,
}

pub async fn get_sessions(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SessionsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let limit = q.limit.unwrap_or(20);
    let sessions = voice_journal::load_sessions();
    let recent: Vec<_> = sessions.iter().rev().take(limit).collect();

    let total_duration: f64 = sessions.iter().map(|s| s.duration_secs).sum();
    let total_clips: usize = sessions.iter().map(|s| s.training_clips).sum();

    Ok(Json(json!({
        "sessions": recent,
        "total": sessions.len(),
        "total_duration_hours": total_duration / 3600.0,
        "total_training_clips": total_clips,
    })))
}

// ── TTS endpoint ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TtsRequest {
    pub text: String,
    pub token: Option<String>,
}

/// POST /api/tts — synthesize text to speech, return audio URL.
pub async fn synthesize_speech(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TtsRequest>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, req.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let text = req.text.trim();
    if text.is_empty() {
        return Ok(Json(json!({ "error": "empty text" })));
    }

    info!("[tts-api] synthesizing: {}", &text[..text.len().min(60)]);

    // Fire duck event for ~estimated duration (150ms per word, min 2s, max 30s)
    let words = text.split_whitespace().count().max(1);
    let est_dur = (words as f64 * 0.15).clamp(2.0, 30.0) as i64;
    crate::music::trigger_duck(true, est_dur).await;
    match satellite_client::run_tts(crate::voice_ws::TTS_HOST, text).await {
        Ok((audio, rate, ch, bits)) => {
            let url = satellite_client::cache_tts_audio(audio, 18789, rate, ch, bits).await;
            Ok(Json(json!({
                "audio_url": url,
                "estimated_duration_secs": est_dur,
            })))
        }
        Err(e) => {
            Ok(Json(json!({ "error": format!("TTS failed: {}", e) })))
        }
    }
}

/// Extract bearer token from Authorization header, falling back to query param.
fn extract_token(headers: &axum::http::HeaderMap, qs_token: Option<&str>) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.to_string())
        .or_else(|| qs_token.map(|s| s.to_string()))
        .unwrap_or_default()
}
