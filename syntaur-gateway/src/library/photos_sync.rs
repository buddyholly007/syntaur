//! Phase 7 — Apple Photos two-way sync (gateway side).
//!
//! The actual PhotoKit polling lives on the Mac Mini host (only macOS
//! exposes the PhotoKit API). This module is the gateway endpoint
//! surface the Mac agent calls into:
//!
//!   POST /api/library/photos/sync/pull  — agent uploads new Apple-side
//!     photos as multipart-batches; we ingest each through the standard
//!     pipeline (with hint=photo).
//!   GET  /api/library/photos/sync/since — agent fetches our additions
//!     since the last cursor so it can push them back to Apple Photos.
//!   POST /api/library/photos/sync/cursor — agent acks what it processed.
//!
//! The Mac agent is a small Swift / Python launchd job that (a) watches
//! PHFetchResult deltas every 15 min, posts new items to /pull, and
//! (b) pulls our /since list, calls PHAssetCreationRequest for each new
//! file, then PUTs the cursor.

use axum::{extract::{Query, State}, http::StatusCode, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct SinceQuery { pub cursor: Option<String>, pub limit: Option<i64> }

#[derive(Debug, Serialize)]
pub struct SinceResponse {
    pub photos: Vec<PhotoItem>,
    pub next_cursor: String,
}

#[derive(Debug, Serialize)]
pub struct PhotoItem {
    pub file_id: i64,
    pub relative_path: String,
    pub original_filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub doc_date: Option<String>,
    pub url: String,            // /api/library/files/{id}/content
}

pub async fn handle_since(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<SinceQuery>,
) -> Result<Json<SinceResponse>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let cursor: i64 = q.cursor.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let db_path = state.db_path.clone();

    let (photos, next): (Vec<PhotoItem>, i64) = tokio::task::spawn_blocking(move || -> (Vec<PhotoItem>, i64) {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return (vec![], cursor) };
        let mut stmt = match conn.prepare(
            "SELECT id, relative_path, original_filename, content_type, size_bytes, doc_date, scan_date
             FROM library_files
             WHERE user_id = ? AND kind = 'photo' AND status = 'filed' AND scan_date > ?
             ORDER BY scan_date ASC LIMIT ?"
        ) { Ok(s) => s, Err(_) => return (vec![], cursor) };
        let mapped = stmt.query_map(params![user_id, cursor, limit], |r| {
            Ok((PhotoItem {
                file_id: r.get(0)?,
                relative_path: r.get(1)?,
                original_filename: r.get(2)?,
                content_type: r.get(3)?,
                size_bytes: r.get(4)?,
                doc_date: r.get(5)?,
                url: format!("/api/library/files/{}/content", r.get::<_, i64>(0)?),
            }, r.get::<_, i64>(6)?))
        });
        match mapped {
            Ok(it) => {
                let mut last_ts = cursor;
                let items: Vec<PhotoItem> = it.filter_map(|r| r.ok()).map(|(p, ts)| { last_ts = ts.max(last_ts); p }).collect();
                (items, last_ts)
            },
            Err(_) => (vec![], cursor),
        }
    }).await.unwrap_or_default();

    Ok(Json(SinceResponse { photos, next_cursor: next.to_string() }))
}

#[derive(Debug, Deserialize)]
pub struct CursorAck {
    pub processed_through: String, // matches next_cursor we returned
}

pub async fn handle_cursor_ack(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<CursorAck>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();
    let cursor = body.processed_through.clone();
    let now = chrono::Utc::now().timestamp();

    let _ = tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return };
        let _ = conn.execute(
            "INSERT INTO library_apple_photos_sync (user_id, last_push_cursor, last_sync_at) VALUES (?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET last_push_cursor = excluded.last_push_cursor, last_sync_at = excluded.last_sync_at",
            params![user_id, &cursor, now],
        );
    }).await;

    Ok(Json(serde_json::json!({ "success": true })))
}
