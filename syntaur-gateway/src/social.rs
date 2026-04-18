//! Social module backend.
//!
//! Phase 1 scope: connection CRUD for the `/social` → Connections pane.
//! Platform adapters (posting, replying, engagement) land in subsequent
//! phases once each platform's auth flow is wired.
//!
//! Storage: `social_connections` table (schema v44). Per-user, scoped
//! via `resolve_principal` + `user_id` on every query.
//!
//! Credentials are stored as plaintext JSON for v1, matching the rest of
//! the SQLite storage posture. Encryption-at-rest is a cross-module
//! improvement (see `projects/syntaur_security_remediation` in the vault).

use axum::{extract::{Path, State}, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SocialConnection {
    pub id: i64,
    pub platform: String,
    pub display_name: Option<String>,
    pub status: String,
    pub status_detail: Option<String>,
    pub agent_id: Option<String>,
    pub connected_at: i64,
    pub last_verified_at: Option<i64>,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct CreateConnectionRequest {
    pub token: String,
    pub platform: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub credentials: serde_json::Value,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_detail: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct DeleteConnectionRequest {
    pub token: String,
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/social/connections?token=...
///
/// Returns the caller's connections. Credentials are never returned —
/// only metadata the UI needs to render status pills.
pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<SocialConnection>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let agent_filter = params.get("agent_id").cloned();
    let db = state.db_path.clone();

    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<SocialConnection>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Credentials are deliberately NOT selected — the UI never renders them,
        // and omitting them from the query reduces the chance of accidental leak.
        let sql = "SELECT id, platform, display_name, status, status_detail, agent_id, \
                          connected_at, last_verified_at, expires_at \
                   FROM social_connections WHERE user_id = ? \
                   ORDER BY platform";
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let iter = stmt.query_map([uid], |r| {
            Ok(SocialConnection {
                id: r.get(0)?,
                platform: r.get(1)?,
                display_name: r.get(2)?,
                status: r.get(3)?,
                status_detail: r.get(4)?,
                agent_id: r.get(5)?,
                connected_at: r.get(6)?,
                last_verified_at: r.get(7)?,
                expires_at: r.get(8)?,
            })
        }).map_err(|e| e.to_string())?;
        for row in iter {
            let row = row.map_err(|e| e.to_string())?;
            if let Some(ref f) = agent_filter {
                if row.agent_id.as_deref() != Some(f.as_str()) { continue; }
            }
            out.push(row);
        }
        Ok(out)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows))
}

/// POST /api/social/connections
///
/// Upsert a connection. Keyed by (user_id, platform, agent_id): repeated
/// POSTs for the same triple update the existing row rather than inserting
/// a second. This is what lets the Phase-1 import script be idempotent.
pub async fn handle_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateConnectionRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();

    if req.platform.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let creds_str = req.credentials.to_string();
    let status = req.status.unwrap_or_else(|| "connected".to_string());
    let now = chrono::Utc::now().timestamp();
    let db = state.db_path.clone();
    let platform = req.platform.clone();
    let display_name = req.display_name.clone();
    let agent_id = req.agent_id.clone();
    let status_clone = status.clone();
    let status_detail = req.status_detail.clone();
    let expires_at = req.expires_at;

    let result = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Manual upsert on (user_id, platform, COALESCE(agent_id,'')) since
        // we want NULL agent_id to dedupe as the same logical row. SQLite
        // treats NULL as distinct in UNIQUE constraints, so we handle it
        // here explicitly instead of declaring one on the table.
        let existing: Option<i64> = conn.query_row(
            "SELECT id FROM social_connections \
             WHERE user_id = ? AND platform = ? AND COALESCE(agent_id,'') = COALESCE(?,'')",
            rusqlite::params![uid, platform, agent_id],
            |r| r.get(0),
        ).ok();
        if let Some(id) = existing {
            conn.execute(
                "UPDATE social_connections SET \
                   display_name = ?, credentials_json = ?, status = ?, status_detail = ?, \
                   expires_at = ?, last_verified_at = ?, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![
                    display_name, creds_str, status_clone, status_detail,
                    expires_at, now, now, id
                ],
            ).map_err(|e| e.to_string())?;
            Ok(id)
        } else {
            conn.execute(
                "INSERT INTO social_connections \
                   (user_id, platform, display_name, credentials_json, status, status_detail, \
                    agent_id, connected_at, last_verified_at, expires_at, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    uid, platform, display_name, creds_str, status_clone, status_detail,
                    agent_id, now, now, expires_at, now, now
                ],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    log::info!("[social] user={} upserted connection platform={} status={}", uid, req.platform, status);
    Ok(Json(serde_json::json!({ "ok": true, "id": result })))
}

/// DELETE /api/social/connections/:id
///
/// Remove a connection. Only the owning user can delete; admins without
/// a user_id cannot reach through.
pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<DeleteConnectionRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = crate::resolve_principal(&state, &req.token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let deleted = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM social_connections WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid],
        ).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    log::warn!("[social] user={} deleted connection id={}", uid, id);
    Ok(Json(serde_json::json!({ "ok": true })))
}
