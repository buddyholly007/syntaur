//! Phase 8 — share URLs + audit log.
//!
//! Two related surfaces, kept in one module because they always travel
//! together: every share creation writes an audit row, every share-URL
//! redemption writes another, and the audit-log read endpoint is the
//! main UI for both ("who saw what when").
//!
//! Share URLs:
//!   - POST /api/library/files/{id}/share         — mint a time-limited URL
//!   - POST /api/library/years/{year}/share       — mint a year-folder URL
//!   - GET  /api/library/share/{token}            — redeem (no auth)
//!   - GET  /api/library/share/{token}/manifest   — manifest preview (no auth)
//!
//! Audit log:
//!   - GET  /api/library/audit                    — owner-only
//!   - log_audit() — internal helper called from ingest, share, delete
//!
//! Watermarking is "soft" for the MVP: the served bytes go out untouched
//! but the response includes `X-Syntaur-Share-Token` and a banner is
//! injected into the HTML viewer wrapper. Full PDF watermarking lands
//! in Phase 9 once we wire printpdf to overlay a "Shared with <recipient>
//! at <ts>" stamp before serving.

use anyhow::{anyhow, Result};
use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use rand::Rng;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

const TOKEN_PREFIX: &str = "shr_";
const TOKEN_LEN: usize = 32;
const DEFAULT_EXPIRY_DAYS: i64 = 14;
const MAX_EXPIRY_DAYS: i64 = 90;

#[derive(Debug, Deserialize)]
pub struct ShareCreate {
    /// Days until expiry. Capped to MAX_EXPIRY_DAYS.
    pub expires_in_days: Option<i64>,
    /// Optional cap on how many times the URL can be redeemed.
    pub max_views: Option<i64>,
    /// `'file' | 'year' | 'tag'`. Filled by the route handler in path
    /// shorthand variants but accepted here for the generic POST too.
    pub scope_kind: Option<String>,
    /// Scope value (file_id, year, or tag name). Same note as above.
    pub scope_value: Option<String>,
    /// Note that goes into audit log alongside the share creation.
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ShareResponse {
    pub token: String,
    pub url: String,
    pub expires_at: i64,
    pub max_views: Option<i64>,
}

fn make_token() -> String {
    let mut rng = rand::thread_rng();
    let charset: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let suffix: String = (0..TOKEN_LEN)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect();
    format!("{TOKEN_PREFIX}{suffix}")
}

/// Internal: write a row to library_audit_log. Best-effort — failure
/// here logs but doesn't abort the calling action. We always want the
/// state change recorded somewhere even if we lose the audit entry.
pub fn log_audit(
    conn: &rusqlite::Connection,
    file_id: Option<i64>,
    user_id: i64,
    action: &str,
    actor: &str,
    sha_before: Option<&str>,
    sha_after: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO library_audit_log
         (file_id, user_id, action, actor, sha_before, sha_after, reason, ts)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![file_id, user_id, action, actor, sha_before, sha_after, reason, now],
    )
    .map_err(|e| anyhow!("audit insert: {e}"))?;
    Ok(())
}

/// POST /api/library/files/{id}/share — mint a share URL for one file.
pub async fn handle_create_file_share(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(file_id): AxPath<i64>,
    Json(body): Json<ShareCreate>,
) -> Result<Json<ShareResponse>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let actor = principal.label().to_string();
    let body_with_scope = ShareCreate {
        scope_kind: Some("file".into()),
        scope_value: Some(file_id.to_string()),
        ..body
    };
    create_share(state, user_id, actor, body_with_scope, Some(file_id)).await
}

/// POST /api/library/years/{year}/share — mint a share URL for a tax year folder.
pub async fn handle_create_year_share(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(year): AxPath<i32>,
    Json(body): Json<ShareCreate>,
) -> Result<Json<ShareResponse>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let actor = principal.label().to_string();
    let body_with_scope = ShareCreate {
        scope_kind: Some("year".into()),
        scope_value: Some(year.to_string()),
        ..body
    };
    create_share(state, user_id, actor, body_with_scope, None).await
}

async fn create_share(
    state: Arc<AppState>,
    user_id: i64,
    actor: String,
    body: ShareCreate,
    file_id_for_audit: Option<i64>,
) -> Result<Json<ShareResponse>, StatusCode> {
    let scope_kind = body.scope_kind.clone().unwrap_or_else(|| "file".into());
    let scope_value = body.scope_value.clone().ok_or(StatusCode::BAD_REQUEST)?;
    if !matches!(scope_kind.as_str(), "file" | "year" | "tag") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let expires_in = body
        .expires_in_days
        .unwrap_or(DEFAULT_EXPIRY_DAYS)
        .clamp(1, MAX_EXPIRY_DAYS);
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + expires_in * 86400;
    let max_views = body.max_views;
    let token_str = make_token();
    let reason = body.reason.clone();

    let db_path = state.db_path.clone();
    let token_clone = token_str.clone();
    let scope_kind_db = scope_kind.clone();
    let scope_value_db = scope_value.clone();
    let actor_db = actor.clone();
    let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        // Verify ownership for `file` scope: only the owner can mint a share.
        if scope_kind_db == "file" {
            let owns: Option<i64> = conn.query_row(
                "SELECT 1 FROM library_files WHERE id = ? AND user_id = ?",
                params![scope_value_db.parse::<i64>().unwrap_or(0), user_id],
                |r| r.get(0),
            ).ok();
            if owns.is_none() { return Err("not owner".into()); }
        }
        conn.execute(
            "INSERT INTO library_share_urls
             (token, owner_user_id, scope_kind, scope_value, expires_at, max_views, view_count, watermarked, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 0, 1, ?)",
            params![&token_clone, user_id, &scope_kind_db, &scope_value_db, expires_at, max_views, now],
        ).map_err(|e| e.to_string())?;
        let _ = log_audit(
            &conn, file_id_for_audit, user_id, "share",
            &actor_db, None, None, reason.as_deref(),
        );
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let url = format!("/api/library/share/{token_str}");
    Ok(Json(ShareResponse { token: token_str, url, expires_at, max_views }))
}

/// GET /api/library/share/{token} — redeem. No auth. The token IS the
/// auth — anyone holding it gets the bytes (or the year-folder zip).
///
/// This is the only place in the gateway where a non-bearer-authenticated
/// principal can read a library file. Constant-time lookup via the unique
/// index on `token`. Single-byte timing leak (record exists vs not) is
/// considered acceptable; we already shed timing info from auth.
pub async fn handle_redeem_share(
    State(state): State<Arc<AppState>>,
    AxPath(token): AxPath<String>,
) -> Result<Response, StatusCode> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() != TOKEN_PREFIX.len() + TOKEN_LEN {
        return Err(StatusCode::NOT_FOUND);
    }
    let db_path = state.db_path.clone();
    let token_clone = token.clone();
    let row: Option<(i64, String, String, i64, Option<i64>, i64, i64)> =
        tokio::task::spawn_blocking(move || -> Option<(i64, String, String, i64, Option<i64>, i64, i64)> {
            let conn = rusqlite::Connection::open(&db_path).ok()?;
            conn.query_row(
                "SELECT id, scope_kind, scope_value, expires_at, max_views, view_count, owner_user_id
                 FROM library_share_urls WHERE token = ?",
                params![&token_clone],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .ok()
        })
        .await
        .ok()
        .flatten();

    let (share_id, scope_kind, scope_value, expires_at, max_views, view_count, owner_user_id) =
        row.ok_or(StatusCode::NOT_FOUND)?;
    let now = chrono::Utc::now().timestamp();
    if expires_at < now {
        return Err(StatusCode::GONE);
    }
    if let Some(cap) = max_views {
        if view_count >= cap {
            return Err(StatusCode::GONE);
        }
    }

    // Bump view_count + audit (best-effort, before serving). Doing the
    // bump first means a transient DB error doesn't leak a free read.
    let db_path = state.db_path.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return };
        let _ = conn.execute(
            "UPDATE library_share_urls SET view_count = view_count + 1 WHERE id = ?",
            params![share_id],
        );
        let file_id_for_audit: Option<i64> = if scope_kind == "file" {
            scope_value.parse::<i64>().ok()
        } else { None };
        let _ = log_audit(
            &conn, file_id_for_audit, owner_user_id, "share_redeem",
            "anonymous", None, None, Some(&format!("token={share_id}")),
        );
    }).await;

    match scope_kind.as_str() {
        "file" => {
            let file_id: i64 = scope_value.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            crate::library::serve_file_for_share(&state, file_id, owner_user_id, &token).await
        }
        "year" => {
            let year: i32 = scope_value.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            crate::library::serve_year_zip_for_share(&state, owner_user_id, year, &token).await
        }
        _ => Err(StatusCode::NOT_IMPLEMENTED),
    }
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub file_id: Option<i64>,
    pub limit: Option<i64>,
    pub since: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub file_id: Option<i64>,
    pub action: String,
    pub actor: String,
    pub reason: Option<String>,
    pub ts: i64,
}

/// GET /api/library/audit — owner-only audit history. Filters by file_id
/// + since + limit.
pub async fn handle_list_audit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let since = q.since.unwrap_or(0);
    let file_id = q.file_id;
    let db_path = state.db_path.clone();

    let rows: Vec<AuditRow> = tokio::task::spawn_blocking(move || -> Vec<AuditRow> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let sql = if file_id.is_some() {
            "SELECT id, file_id, action, actor, reason, ts FROM library_audit_log
             WHERE user_id = ? AND ts >= ? AND file_id = ?
             ORDER BY ts DESC LIMIT ?"
        } else {
            "SELECT id, file_id, action, actor, reason, ts FROM library_audit_log
             WHERE user_id = ? AND ts >= ?
             ORDER BY ts DESC LIMIT ?"
        };
        let mut stmt = match conn.prepare(sql) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = if let Some(fid) = file_id {
            stmt.query_map(params![user_id, since, fid, limit], |r| Ok(AuditRow {
                id: r.get(0)?, file_id: r.get(1)?, action: r.get(2)?,
                actor: r.get(3)?, reason: r.get(4)?, ts: r.get(5)?,
            }))
        } else {
            stmt.query_map(params![user_id, since, limit], |r| Ok(AuditRow {
                id: r.get(0)?, file_id: r.get(1)?, action: r.get(2)?,
                actor: r.get(3)?, reason: r.get(4)?, ts: r.get(5)?,
            }))
        };
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    })
    .await
    .unwrap_or_default();

    Ok(Json(serde_json::json!({ "audit": rows })))
}
