//! Phase 9 — cross-user sharing ACL.
//!
//! Two surfaces:
//!   • **library_shares** — durable per-user share grants. The owner
//!     names another user (or a household member group) and the file/
//!     prefix scope they get. This is *household sharing*, not the
//!     time-limited public-link share in `shares.rs`.
//!   • **applies_to(file_id, viewer_user_id)** helper — the read path
//!     calls this to decide whether to serve a row owned by someone
//!     else. Returns the permission level (`read` or `write`) or None.
//!
//! Endpoint shape mirrors what the /library settings UI talks to:
//!   GET    /api/library/shares                    — list grants I made
//!   GET    /api/library/shares/incoming           — list grants made TO me
//!   POST   /api/library/shares                    — grant
//!   DELETE /api/library/shares/{id}               — revoke
//!
//! Auth: only the owner may create/revoke. The ACL is consulted only
//! when an authenticated principal asks for someone else's row — the
//! base list endpoint still defaults to "show me my own."

use anyhow::{anyhow, Result};
use axum::{extract::{Path as AxPath, State}, http::StatusCode, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ShareGrant {
    /// Recipient user id. Must exist in `users`.
    pub shared_with_user_id: i64,
    /// `'file' | 'prefix'`. Prefix lets you grant whole subtrees
    /// (e.g. `tax/2026/` → spouse gets every 2026 tax doc going forward).
    pub scope_kind: String,
    pub scope_value: String,
    /// `'read' | 'write'`. Write is rare; only for tightly-coupled
    /// household roles (spouse on the same finances).
    pub permission: String,
}

#[derive(Debug, Serialize)]
pub struct ShareRow {
    pub id: i64,
    pub owner_user_id: i64,
    pub shared_with_user_id: i64,
    pub scope_kind: String,
    pub scope_value: String,
    pub permission: String,
    pub created_at: i64,
}

pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let rows: Vec<ShareRow> = tokio::task::spawn_blocking(move || -> Vec<ShareRow> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT id, owner_user_id, shared_with_user_id, scope_kind, scope_value, permission, created_at
             FROM library_shares WHERE owner_user_id = ? ORDER BY created_at DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id], |r| Ok(ShareRow {
            id: r.get(0)?, owner_user_id: r.get(1)?, shared_with_user_id: r.get(2)?,
            scope_kind: r.get(3)?, scope_value: r.get(4)?,
            permission: r.get(5)?, created_at: r.get(6)?,
        }));
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "shares": rows })))
}

pub async fn handle_list_incoming(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let rows: Vec<ShareRow> = tokio::task::spawn_blocking(move || -> Vec<ShareRow> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT id, owner_user_id, shared_with_user_id, scope_kind, scope_value, permission, created_at
             FROM library_shares WHERE shared_with_user_id = ? ORDER BY created_at DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id], |r| Ok(ShareRow {
            id: r.get(0)?, owner_user_id: r.get(1)?, shared_with_user_id: r.get(2)?,
            scope_kind: r.get(3)?, scope_value: r.get(4)?,
            permission: r.get(5)?, created_at: r.get(6)?,
        }));
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "shares": rows })))
}

pub async fn handle_create(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ShareGrant>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    if !matches!(body.scope_kind.as_str(), "file" | "prefix") { return Err(StatusCode::BAD_REQUEST); }
    if !matches!(body.permission.as_str(), "read" | "write") { return Err(StatusCode::BAD_REQUEST); }
    if body.shared_with_user_id == user_id { return Err(StatusCode::BAD_REQUEST); }
    let db_path = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    let inserted: Result<i64> = tokio::task::spawn_blocking(move || -> Result<i64> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        // Verify ownership for `file` scope.
        if body.scope_kind == "file" {
            let owns: Option<i64> = conn.query_row(
                "SELECT 1 FROM library_files WHERE id = ? AND user_id = ?",
                params![body.scope_value.parse::<i64>().unwrap_or(0), user_id],
                |r| r.get(0),
            ).ok();
            if owns.is_none() { return Err(anyhow!("not owner")); }
        }
        // Verify recipient exists.
        let recipient_exists: Option<i64> = conn.query_row(
            "SELECT 1 FROM users WHERE id = ?",
            params![body.shared_with_user_id], |r| r.get(0),
        ).ok();
        if recipient_exists.is_none() { return Err(anyhow!("recipient unknown")); }
        conn.execute(
            "INSERT INTO library_shares
             (owner_user_id, shared_with_user_id, scope_kind, scope_value, permission, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_user_id, shared_with_user_id, scope_kind, scope_value)
             DO UPDATE SET permission = excluded.permission",
            params![user_id, body.shared_with_user_id, &body.scope_kind, &body.scope_value, &body.permission, now],
        ).map_err(|e| anyhow!("insert: {e}"))?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|e| anyhow!("join: {e}")).and_then(|r| r);

    match inserted {
        Ok(id) => Ok(Json(serde_json::json!({ "success": true, "id": id }))),
        Err(e) => {
            log::warn!("[library/acl] create grant failed: {e}");
            Err(StatusCode::FORBIDDEN)
        }
    }
}

pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(id): AxPath<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let _ = tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return };
        let _ = conn.execute(
            "DELETE FROM library_shares WHERE id = ? AND owner_user_id = ?",
            params![id, user_id],
        );
    }).await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Read helper for the file-content path: does `viewer_user_id` have a
/// valid grant on `file_id` (owned by someone else)?
///
/// Returns Some("read") | Some("write") | None.
#[allow(dead_code)]
pub fn applies_to(
    conn: &rusqlite::Connection,
    file_id: i64,
    viewer_user_id: i64,
) -> Option<String> {
    // First, find the owner + relative_path.
    let row: Option<(i64, String)> = conn.query_row(
        "SELECT user_id, relative_path FROM library_files WHERE id = ?",
        params![file_id], |r| Ok((r.get(0)?, r.get(1)?)),
    ).ok();
    let (owner_user_id, rel_path) = row?;
    if owner_user_id == viewer_user_id { return Some("write".into()); }

    // Direct file grant.
    if let Ok(perm) = conn.query_row::<String, _, _>(
        "SELECT permission FROM library_shares
         WHERE owner_user_id = ? AND shared_with_user_id = ?
           AND scope_kind = 'file' AND scope_value = ?",
        params![owner_user_id, viewer_user_id, file_id.to_string()], |r| r.get(0),
    ) {
        return Some(perm);
    }
    // Prefix grant.
    if let Ok(perm) = conn.query_row::<String, _, _>(
        "SELECT permission FROM library_shares
         WHERE owner_user_id = ? AND shared_with_user_id = ?
           AND scope_kind = 'prefix' AND ? LIKE scope_value || '%'",
        params![owner_user_id, viewer_user_id, rel_path], |r| r.get(0),
    ) {
        return Some(perm);
    }
    None
}
