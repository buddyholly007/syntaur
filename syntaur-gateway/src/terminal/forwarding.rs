//! Port forwarding — local and remote tunnels via SSH.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use log::info;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

#[derive(Deserialize)]
pub struct CreateForwardRequest {
    pub host_id: i64,
    pub direction: String,
    #[serde(default = "default_bind")]
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    #[serde(default)]
    pub auto_start: bool,
}

fn default_bind() -> String { "127.0.0.1".into() }

/// GET /api/terminal/forwards
pub async fn list_forwards(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn.prepare(
        "SELECT f.id, f.host_id, h.name, f.direction, f.bind_host, f.bind_port, \
         f.target_host, f.target_port, f.auto_start \
         FROM terminal_port_forwards f \
         JOIN terminal_hosts h ON h.id = f.host_id \
         ORDER BY f.id"
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let forwards: Vec<Value> = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "host_id": row.get::<_, i64>(1)?,
            "host_name": row.get::<_, String>(2)?,
            "direction": row.get::<_, String>(3)?,
            "bind_host": row.get::<_, String>(4)?,
            "bind_port": row.get::<_, i32>(5)?,
            "target_host": row.get::<_, String>(6)?,
            "target_port": row.get::<_, i32>(7)?,
            "auto_start": row.get::<_, i32>(8)? != 0,
        }))
    }).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .filter_map(|r| r.ok())
    .collect();

    Ok(Json(json!({ "forwards": forwards })))
}

/// POST /api/terminal/forwards
pub async fn create_forward(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateForwardRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let now = chrono::Utc::now().timestamp();
    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    conn.execute(
        "INSERT INTO terminal_port_forwards \
         (host_id, direction, bind_host, bind_port, target_host, target_port, auto_start, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            req.host_id, req.direction, req.bind_host, req.bind_port,
            req.target_host, req.target_port, req.auto_start as i32, now
        ],
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let id = conn.last_insert_rowid();
    info!("[terminal:forwarding] created {} forward {}:{} → {}:{}", req.direction, req.bind_host, req.bind_port, req.target_host, req.target_port);

    // TODO: actually start the tunnel via SSH in a background task

    Ok(Json(json!({ "id": id, "success": true })))
}

/// DELETE /api/terminal/forwards/{id}
pub async fn delete_forward(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(fwd_id): axum::extract::Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let conn = rusqlite::Connection::open(&mgr.db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    conn.execute("DELETE FROM terminal_port_forwards WHERE id=?", params![fwd_id])
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // TODO: stop running tunnel

    Ok(Json(json!({ "success": true })))
}
