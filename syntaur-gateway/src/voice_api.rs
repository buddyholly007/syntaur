//! HTTP API endpoints for the Voice Journal module.
//!
//! Read surface (file-backed):
//! - GET /api/journal?date=YYYY-MM-DD — get journal for a date (default today)
//! - GET /api/journal/dates — list available journal dates
//! - GET /api/journal/search?q=term&max=5 — search across journals
//! - GET /api/journal/sessions?limit=20 — list recording sessions
//! - GET /api/journal/export — concatenated markdown of every day
//!
//! Moments (DB-backed, per-user, isolated):
//! - POST /api/journal/moments {token, date, text, source?, time_of_day?, note?}
//! - GET  /api/journal/moments?limit=&date=
//! - DELETE /api/journal/moments/:id
//!
//! Training review (file-backed):
//! - GET /api/journal/training — list training clips + wake-word samples
//! - DELETE /api/journal/training/clip {token, path}
//!
//! Settings:
//! - GET /api/journal/settings — voice_journal config (read-only) + user prefs

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use log::{info, warn};
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

// ── Journal export ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExportQuery {
    pub token: Option<String>,
}

/// GET /api/journal/export — concatenated markdown of every day, newest first.
/// Returns `text/markdown`. For a journal user this is the "take my words and go"
/// escape hatch — no lock-in.
pub async fn export_journal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<ExportQuery>,
) -> Result<axum::response::Response, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let dir = voice_journal::config().data_dir().join("journal");
    let mut dates: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".md") {
                dates.push(stem.to_string());
            }
        }
    }
    dates.sort();
    dates.reverse();

    let mut out = String::from("# Voice Journal — full export\n\n");
    for d in &dates {
        let path = dir.join(format!("{}.md", d));
        if let Ok(content) = std::fs::read_to_string(&path) {
            out.push_str(&content);
            out.push_str("\n\n---\n\n");
        }
    }

    use axum::http::header;
    Ok(axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "text/markdown; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"voice-journal.md\"")
        .body(axum::body::Body::from(out))
        .unwrap())
}

// ── Moments (DB-backed, per-user) ────────────────────────────────────

#[derive(Deserialize)]
pub struct MomentCreate {
    pub token: String,
    pub date: String,
    pub text: String,
    pub source: Option<String>,
    pub time_of_day: Option<String>,
    pub note: Option<String>,
}

/// POST /api/journal/moments — star a fragment from a day's entries.
pub async fn create_moment(
    State(state): State<Arc<AppState>>,
    Json(body): Json<MomentCreate>,
) -> Result<Json<Value>, StatusCode> {
    let principal = crate::resolve_principal_scoped(&state, &body.token, "voice_ingest").await?;
    let uid = principal.user_id();
    let text = body.text.trim().to_string();
    if text.is_empty() || text.len() > 2000 { return Err(StatusCode::BAD_REQUEST); }

    let db = state.db_path.clone();
    let date = body.date;
    let source = body.source;
    let time_of_day = body.time_of_day;
    let note = body.note;
    let now = chrono::Utc::now().timestamp();

    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "INSERT INTO journal_moments (user_id, date, text, source, time_of_day, note, created_at)
             VALUES (?,?,?,?,?,?,?)",
            rusqlite::params![uid, date, text, source, time_of_day, note, now],
        )?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match res {
        Ok(id) => Ok(Json(json!({"ok": true, "id": id}))),
        Err(e) => {
            warn!("[journal] moment insert failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub struct MomentsQuery {
    pub token: Option<String>,
    pub limit: Option<i64>,
    pub date: Option<String>,
}

/// GET /api/journal/moments — list the caller's starred moments.
pub async fn list_moments(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<MomentsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal_scoped(&state, &token, "voice_ingest").await?;
    let uid = principal.user_id();
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let date_filter = q.date;

    let db = state.db_path.clone();
    let rows = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut out = Vec::new();
        if let Some(d) = date_filter {
            let mut stmt = conn.prepare(
                "SELECT id, date, text, source, time_of_day, note, created_at
                 FROM journal_moments WHERE user_id = ? AND date = ?
                 ORDER BY created_at DESC LIMIT ?"
            )?;
            let rows = stmt.query_map(rusqlite::params![uid, d, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "date": r.get::<_, String>(1)?,
                    "text": r.get::<_, String>(2)?,
                    "source": r.get::<_, Option<String>>(3)?,
                    "time_of_day": r.get::<_, Option<String>>(4)?,
                    "note": r.get::<_, Option<String>>(5)?,
                    "created_at": r.get::<_, i64>(6)?,
                }))
            })?;
            for row in rows { out.push(row?); }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, date, text, source, time_of_day, note, created_at
                 FROM journal_moments WHERE user_id = ?
                 ORDER BY created_at DESC LIMIT ?"
            )?;
            let rows = stmt.query_map(rusqlite::params![uid, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "date": r.get::<_, String>(1)?,
                    "text": r.get::<_, String>(2)?,
                    "source": r.get::<_, Option<String>>(3)?,
                    "time_of_day": r.get::<_, Option<String>>(4)?,
                    "note": r.get::<_, Option<String>>(5)?,
                    "created_at": r.get::<_, i64>(6)?,
                }))
            })?;
            for row in rows { out.push(row?); }
        }
        Ok(out)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({"moments": rows, "count": rows.len()})))
}

/// DELETE /api/journal/moments/:id — unstar.
pub async fn delete_moment(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<MomentsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    let principal = crate::resolve_principal_scoped(&state, &token, "voice_ingest").await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let deleted = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "DELETE FROM journal_moments WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
        )
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if deleted == 0 { return Err(StatusCode::NOT_FOUND); }
    info!("[journal] moment {} deleted by user {}", id, uid);
    Ok(Json(json!({"ok": true})))
}

// ── Training review ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TrainingQuery {
    pub token: Option<String>,
    pub limit: Option<usize>,
}

/// GET /api/journal/training — list training clips + wake-word samples.
/// Scans `<data_dir>/training/` and `<data_dir>/wake-word/` for .wav files
/// and returns filename, size, mtime. No audio streamed — just metadata.
pub async fn list_training(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<TrainingQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let root = voice_journal::config().data_dir();

    fn scan(dir: std::path::PathBuf, limit: usize) -> Vec<Value> {
        let mut out: Vec<(i64, Value)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new(); };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".wav") { continue; }
            let meta = match e.metadata() { Ok(m) => m, Err(_) => continue };
            let size = meta.len();
            let mtime = meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push((mtime, json!({
                "name": name,
                "size_bytes": size,
                "modified": mtime,
            })));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out.into_iter().take(limit).map(|(_, v)| v).collect()
    }

    let clips = scan(root.join("training"), limit);
    let wake = scan(root.join("wake-word"), limit);
    Ok(Json(json!({
        "clips": clips,
        "wake_words": wake,
        "clip_count": clips.len(),
        "wake_word_count": wake.len(),
    })))
}

#[derive(Deserialize)]
pub struct TrainingDelete {
    pub token: String,
    pub kind: String,   // "clip" | "wake_word"
    pub name: String,   // filename only, no path separators
}

/// POST /api/journal/training/delete — remove a single .wav (clip or wake-word).
/// Name is sanitized against path escapes; only *.wav inside the known subdirs
/// is deletable.
pub async fn delete_training(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TrainingDelete>,
) -> Result<Json<Value>, StatusCode> {
    require_voice_auth(&state, &body.token).await?;

    // Harden against traversal
    if body.name.contains('/') || body.name.contains('\\') || body.name.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !body.name.ends_with(".wav") { return Err(StatusCode::BAD_REQUEST); }
    let sub = match body.kind.as_str() {
        "clip" => "training",
        "wake_word" => "wake-word",
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let path = voice_journal::config().data_dir().join(sub).join(&body.name);
    if !path.exists() { return Err(StatusCode::NOT_FOUND); }
    match std::fs::remove_file(&path) {
        Ok(_) => {
            warn!("[journal] training {} removed: {}", body.kind, body.name);
            Ok(Json(json!({"ok": true})))
        }
        Err(e) => {
            warn!("[journal] training delete failed {}: {e}", body.name);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ── Settings ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SettingsQuery {
    pub token: Option<String>,
}

/// GET /api/journal/settings — read-only voice_journal config + journal counts.
pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SettingsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let token = extract_token(&headers, q.token.as_deref());
    require_voice_auth(&state, &token).await?;

    let cfg = voice_journal::config();
    let dir = cfg.data_dir();
    let journal_count = std::fs::read_dir(dir.join("journal"))
        .map(|r| r.filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
            .count())
        .unwrap_or(0);

    Ok(Json(json!({
        "storage_path": cfg.storage_path,
        "wearable_port": cfg.wearable_port,
        "wake_word": cfg.wake_word,
        "consent_mode": cfg.consent_mode,
        "auto_cleanup_days": cfg.auto_cleanup_days,
        "training_clips": cfg.training_clips,
        "wake_word_min_clips": cfg.wake_word_min_clips,
        "journal_days_recorded": journal_count,
    })))
}
