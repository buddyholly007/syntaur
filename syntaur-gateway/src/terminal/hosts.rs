//! Host CRUD — REST handlers + DB operations.

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use log::info;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;
use super::TerminalHost;

/// GET /api/terminal/hosts
pub async fn list_hosts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;
    let hosts = get_all_hosts(&mgr.db_path)?;
    Ok(Json(json!({ "hosts": hosts })))
}

#[derive(Deserialize)]
pub struct CreateHostRequest {
    pub name: String,
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default = "default_auth")]
    pub auth_method: String,
    pub private_key: Option<String>,
    pub password: Option<String>,
    pub jump_host_id: Option<i64>,
    #[serde(default = "default_shell")]
    pub default_shell: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub favorite: bool,
}

fn default_port() -> u16 { 22 }
fn default_username() -> String { "sean".into() }
fn default_auth() -> String { "key".into() }
fn default_shell() -> String { "/bin/bash".into() }
fn default_color() -> String { "#0ea5e9".into() }

/// POST /api/terminal/hosts
pub async fn create_host(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateHostRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    // Encrypt credentials if present
    let enc_key = req.private_key.as_deref().map(|k| {
        crate::crypto::encrypt(&mgr.master_key, k).unwrap_or_else(|_| k.to_string())
    });
    let enc_pass = req.password.as_deref().map(|p| {
        crate::crypto::encrypt(&mgr.master_key, p).unwrap_or_else(|_| p.to_string())
    });

    let now = chrono::Utc::now().timestamp();
    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    conn.execute(
        "INSERT INTO terminal_hosts \
         (name, hostname, port, username, auth_method, private_key, password, \
          jump_host_id, default_shell, group_name, tags, color, sort_order, \
          is_local, favorite, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
        params![
            req.name, req.hostname, req.port, req.username, req.auth_method,
            enc_key, enc_pass, req.jump_host_id, req.default_shell,
            req.group_name, if req.tags.is_empty() { "[]".to_string() } else { req.tags },
            req.color, req.is_local as i32, req.favorite as i32, now, now
        ],
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let id = conn.last_insert_rowid();
    info!("[terminal:hosts] created host '{}' (id={})", req.name, id);

    Ok(Json(json!({ "id": id, "name": req.name })))
}

/// PUT /api/terminal/hosts/{id}
pub async fn update_host(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Json(req): Json<CreateHostRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let enc_key = req.private_key.as_deref().map(|k| {
        crate::crypto::encrypt(&mgr.master_key, k).unwrap_or_else(|_| k.to_string())
    });
    let enc_pass = req.password.as_deref().map(|p| {
        crate::crypto::encrypt(&mgr.master_key, p).unwrap_or_else(|_| p.to_string())
    });

    let now = chrono::Utc::now().timestamp();
    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    conn.execute(
        "UPDATE terminal_hosts SET name=?, hostname=?, port=?, username=?, auth_method=?, \
         private_key=?, password=?, jump_host_id=?, default_shell=?, group_name=?, tags=?, \
         color=?, is_local=?, favorite=?, updated_at=? WHERE id=?",
        params![
            req.name, req.hostname, req.port, req.username, req.auth_method,
            enc_key, enc_pass, req.jump_host_id, req.default_shell,
            req.group_name, if req.tags.is_empty() { "[]".to_string() } else { req.tags },
            req.color, req.is_local as i32, req.favorite as i32, now, host_id
        ],
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

/// DELETE /api/terminal/hosts/{id}
pub async fn delete_host(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    conn.execute("DELETE FROM terminal_hosts WHERE id=?", params![host_id])
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

/// POST /api/terminal/hosts/{id}/test — test SSH connection.
pub async fn test_connection(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    if host.is_local {
        return Ok(Json(json!({ "success": true, "message": "local host" })));
    }

    let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
    let key_path = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());

    // Try connecting, then immediately disconnect
    match super::ssh::connect_ssh(&host.hostname, host.port, &host.username, &key_path, 80, 24).await {
        Ok(client) => {
            client.close().await;
            Ok(Json(json!({ "success": true, "message": format!("connected to {}@{}", host.username, host.hostname) })))
        }
        Err(e) => {
            Ok(Json(json!({ "success": false, "message": e })))
        }
    }
}

// --- DB helpers ---

pub fn get_all_hosts(db_path: &Path) -> Result<Vec<TerminalHost>, (StatusCode, String)> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn.prepare(
        "SELECT id, name, hostname, port, username, auth_method, private_key, password, \
         jump_host_id, default_shell, group_name, tags, color, sort_order, is_local, favorite \
         FROM terminal_hosts ORDER BY sort_order, name"
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let hosts = stmt.query_map([], |row| {
        Ok(TerminalHost {
            id: row.get(0)?,
            name: row.get(1)?,
            hostname: row.get(2)?,
            port: row.get::<_, i32>(3)? as u16,
            username: row.get(4)?,
            auth_method: row.get(5)?,
            private_key: row.get(6)?,
            password: row.get(7)?,
            jump_host_id: row.get(8)?,
            default_shell: row.get(9)?,
            group_name: row.get(10)?,
            tags: row.get(11)?,
            color: row.get(12)?,
            sort_order: row.get(13)?,
            is_local: row.get::<_, i32>(14)? != 0,
            favorite: row.get::<_, i32>(15)? != 0,
        })
    }).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .filter_map(|r| r.ok())
    .collect();

    Ok(hosts)
}

pub fn get_host_by_id(db_path: &Path, id: i64) -> Result<TerminalHost, String> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open db: {}", e))?;

    conn.query_row(
        "SELECT id, name, hostname, port, username, auth_method, private_key, password, \
         jump_host_id, default_shell, group_name, tags, color, sort_order, is_local, favorite \
         FROM terminal_hosts WHERE id=?",
        params![id],
        |row| Ok(TerminalHost {
            id: row.get(0)?,
            name: row.get(1)?,
            hostname: row.get(2)?,
            port: row.get::<_, i32>(3)? as u16,
            username: row.get(4)?,
            auth_method: row.get(5)?,
            private_key: row.get(6)?,
            password: row.get(7)?,
            jump_host_id: row.get(8)?,
            default_shell: row.get(9)?,
            group_name: row.get(10)?,
            tags: row.get(11)?,
            color: row.get(12)?,
            sort_order: row.get(13)?,
            is_local: row.get::<_, i32>(14)? != 0,
            favorite: row.get::<_, i32>(15)? != 0,
        }),
    ).map_err(|e| format!("host {} not found: {}", id, e))
}
