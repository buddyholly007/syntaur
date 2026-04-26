//! Phase 5 — Tag system (Paperless-parity).
//!
//! Tag CRUD + tag-suggestion-engine helpers + smart-folder management.
//! Auto-tags fire from the classifier output (year, month, kind, vendor,
//! entity, confidence). User tags layer on top.

use anyhow::{anyhow, Result};
use axum::{extract::{Path as AxPath, State}, http::StatusCode, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Serialize)]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub color: Option<String>,
    pub file_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct TagCreate {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TagApplyBody {
    pub tag_ids: Vec<i64>,
}

pub async fn handle_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let rows = tokio::task::spawn_blocking(move || -> Vec<TagRow> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT t.id, t.name, t.kind, t.color, COUNT(ft.file_id)
             FROM library_tags t
             LEFT JOIN library_file_tags ft ON ft.tag_id = t.id
             WHERE t.user_id = ?
             GROUP BY t.id ORDER BY t.kind, t.name"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id], |r| Ok(TagRow {
            id: r.get(0)?, name: r.get(1)?, kind: r.get(2)?,
            color: r.get(3)?, file_count: r.get(4)?,
        }));
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "tags": rows })))
}

pub async fn handle_create(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<TagCreate>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let name = body.name.trim().to_string();
    if name.is_empty() { return Err(StatusCode::BAD_REQUEST); }

    let id: Result<i64, String> = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO library_tags (user_id, name, kind, color, created_at) VALUES (?, ?, 'user', ?, ?)",
            params![user_id, &name, &body.color, now],
        ).map_err(|e| e.to_string())?;
        let id: i64 = conn.query_row(
            "SELECT id FROM library_tags WHERE user_id = ? AND name = ?",
            params![user_id, &name], |r| r.get(0)
        ).map_err(|e| e.to_string())?;
        Ok(id)
    }).await.unwrap_or_else(|e| Err(e.to_string()));

    match id {
        Ok(i) => Ok(Json(serde_json::json!({ "success": true, "id": i }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn handle_apply(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxPath(file_id): AxPath<i64>,
    Json(body): Json<TagApplyBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        let owned: Option<i64> = conn.query_row(
            "SELECT 1 FROM library_files WHERE id = ? AND user_id = ?",
            params![file_id, user_id], |r| r.get(0)
        ).ok();
        if owned.is_none() { return Err("not found".into()); }
        for tid in body.tag_ids {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO library_file_tags (file_id, tag_id) VALUES (?, ?)",
                params![file_id, tid],
            );
        }
        Ok(())
    }).await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Helper called by ingest path — apply system tags from a Classification.
/// Tags created on first use (idempotent INSERT OR IGNORE).
pub fn apply_system_tags(
    conn: &rusqlite::Connection,
    user_id: i64,
    file_id: i64,
    classification: &crate::library::Classification,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut to_apply: Vec<String> = vec![
        format!("kind:{}", classification.kind),
    ];
    if let Some(y) = classification.year { to_apply.push(format!("year:{y}")); }
    if let Some(e) = &classification.entity { to_apply.push(format!("entity:{e}")); }
    if let Some(v) = &classification.vendor {
        to_apply.push(format!("vendor:{}", crate::library::cleanup::normalize_vendor(v)));
    }
    if let Some(f) = &classification.form_type { to_apply.push(format!("form:{}", f.to_lowercase())); }
    if classification.confidence < 0.85 { to_apply.push("needs_review".into()); }

    for name in to_apply {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO library_tags (user_id, name, kind, color, created_at) VALUES (?, ?, 'system', NULL, ?)",
            params![user_id, &name, now],
        );
        let tag_id: Option<i64> = conn.query_row(
            "SELECT id FROM library_tags WHERE user_id = ? AND name = ?",
            params![user_id, &name], |r| r.get(0)
        ).ok();
        if let Some(tid) = tag_id {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO library_file_tags (file_id, tag_id) VALUES (?, ?)",
                params![file_id, tid],
            );
        }
    }
    Ok(())
}
