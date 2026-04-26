//! Pre-restart autosave + graceful drain.
//!
//! Sean ships gateway updates frequently. Without coordination, an in-flight
//! user (filling a long form, drafting a note) loses everything when the
//! container restarts. This module is the server side of a two-part
//! handshake:
//!
//!   1. **Drain**: deploy.sh hits `POST /api/system/drain` *before* it
//!      sends the SIGTERM. That flips `AppState.restart_pending` to true
//!      and returns 200. The deploy script then sleeps 6–8 seconds before
//!      `docker restart` so connected clients have time to flush.
//!
//!   2. **Autosave**: every page polls `GET /health` every 15 s. When it
//!      sees `restart_pending: true`, the in-memory autosave registry runs
//!      every registered hook and POSTs each result to `/api/drafts/save`.
//!      On reconnect the page calls `GET /api/drafts/{scope}` and rehydrates.
//!
//! Drafts are scoped (`scope`, `scope_key`) so e.g. the chat composer keys
//! on `chat:<conversation_id>` while the journal entry editor keys on
//! `journal:<entry_id>` — multiple in-flight pieces of work can coexist.
//! TTL defaults to 7 days; cleanup happens lazily on each save (cheap
//! `DELETE WHERE ttl_at < now`).

use anyhow::{anyhow, Result};
use axum::{
    extract::{ConnectInfo, Path as AxPath, State},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;

const DRAFT_DEFAULT_TTL_SECS: i64 = 7 * 86400;
const DRAFT_MAX_BYTES: i64 = 256 * 1024;          // single-doc cap; flushes anything bigger to disk-only
const DRAFT_MAX_TOTAL_PER_USER: i64 = 8 * 1024 * 1024; // 8 MB envelope per user

#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub scope: String,
    pub scope_key: String,
    pub value: serde_json::Value,
    /// Optional override (in seconds). Capped to 30 days.
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct DraftEntry {
    pub scope: String,
    pub scope_key: String,
    pub value: serde_json::Value,
    pub bytes: i64,
    pub created_at: i64,
    pub ttl_at: i64,
}

/// POST /api/drafts/save — store a single autosave snapshot.
///
/// Idempotent on `(user_id, scope, scope_key)`. Returns the stored size.
pub async fn handle_save(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<SaveBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let value_str = serde_json::to_string(&body.value).unwrap_or_else(|_| "null".into());
    let bytes = value_str.len() as i64;
    if bytes > DRAFT_MAX_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let ttl = body
        .ttl_secs
        .unwrap_or(DRAFT_DEFAULT_TTL_SECS)
        .clamp(60, 30 * 86400);
    let now = chrono::Utc::now().timestamp();
    let ttl_at = now + ttl;

    let scope = body.scope.clone();
    let scope_key = body.scope_key.clone();

    let stored = tokio::task::spawn_blocking(move || -> Result<()> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        // Lazy GC.
        let _ = conn.execute("DELETE FROM client_drafts WHERE ttl_at < ?", params![now]);
        // Per-user envelope cap — drop oldest first if we'd exceed.
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0) FROM client_drafts WHERE user_id = ?",
                params![user_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if total + bytes > DRAFT_MAX_TOTAL_PER_USER {
            let _ = conn.execute(
                "DELETE FROM client_drafts WHERE id IN (
                    SELECT id FROM client_drafts WHERE user_id = ?
                    ORDER BY created_at ASC LIMIT 4
                )",
                params![user_id],
            );
        }
        conn.execute(
            "INSERT INTO client_drafts (user_id, scope, scope_key, value_json, bytes, created_at, ttl_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, scope, scope_key) DO UPDATE SET
                value_json = excluded.value_json,
                bytes = excluded.bytes,
                created_at = excluded.created_at,
                ttl_at = excluded.ttl_at",
            params![user_id, &scope, &scope_key, &value_str, bytes, now, ttl_at],
        ).map_err(|e| anyhow!("insert: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| { log::warn!("[drafts/save] {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;

    let _ = stored;
    Ok(Json(serde_json::json!({
        "success": true,
        "bytes": bytes,
        "ttl_at": ttl_at,
    })))
}

/// GET /api/drafts/{scope} — list all drafts for a scope (e.g. all
/// in-flight chat composers). Pages call this on load.
pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(scope): AxPath<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let drafts: Vec<DraftEntry> = tokio::task::spawn_blocking(move || -> Vec<DraftEntry> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let now = chrono::Utc::now().timestamp();
        let mut stmt = match conn.prepare(
            "SELECT scope, scope_key, value_json, bytes, created_at, ttl_at
             FROM client_drafts
             WHERE user_id = ? AND scope = ? AND ttl_at >= ?
             ORDER BY created_at DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id, &scope, now], |r| {
            let s: String = r.get(2)?;
            let value: serde_json::Value = serde_json::from_str(&s).unwrap_or(serde_json::Value::Null);
            Ok(DraftEntry {
                scope: r.get(0)?,
                scope_key: r.get(1)?,
                value,
                bytes: r.get(3)?,
                created_at: r.get(4)?,
                ttl_at: r.get(5)?,
            })
        });
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "drafts": drafts })))
}

/// DELETE /api/drafts/{scope}/{scope_key} — drop a single draft (called
/// once the user has either submitted the form or explicitly dismissed
/// the rehydrate prompt).
pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath((scope, scope_key)): AxPath<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let _ = tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return };
        let _ = conn.execute(
            "DELETE FROM client_drafts WHERE user_id = ? AND scope = ? AND scope_key = ?",
            params![user_id, &scope, &scope_key],
        );
    }).await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /api/system/drain — operator-only. Flips `restart_pending` so
/// every connected client's `/health` poll sees the flag and flushes
/// drafts before the deploy script restarts the container.
///
/// Auth: accepts EITHER (a) a request from loopback (deploy.sh runs
/// `docker exec syntaur curl http://127.0.0.1:18789/api/system/drain`,
/// which arrives on 127.0.0.1 from inside the container), OR (b) a
/// bearer token that resolves to an admin principal (operator hitting
/// it from a logged-in browser session). External non-admin callers
/// always get 403 — drain isn't a public action.
pub async fn handle_drain(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let from_loopback = addr.ip().is_loopback();
    if !from_loopback {
        let token = crate::security::bearer_from_headers(&headers);
        let principal = crate::resolve_principal(&state, token).await?;
        if !principal.is_admin() {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    state.restart_pending.store(true, std::sync::atomic::Ordering::SeqCst);
    let drain_started = chrono::Utc::now().timestamp();
    state.restart_pending_since.store(drain_started, std::sync::atomic::Ordering::SeqCst);
    log::info!(
        "[drafts/drain] restart_pending=true — clients will flush autosave (from {}{})",
        addr.ip(), if from_loopback { ", loopback bypass" } else { "" },
    );
    Ok(Json(serde_json::json!({
        "success": true,
        "drain_started_at": drain_started,
        "advice": "wait 6-8s before SIGTERM so clients can POST /api/drafts/save",
    })))
}
