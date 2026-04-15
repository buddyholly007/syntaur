//! Session lifecycle — create, list, kill.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::AppState;
use super::{LiveSession, RingBuffer, SessionBackend};

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub host_id: i64,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 { 80 }
fn default_rows() -> u16 { 24 }

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub host_id: i64,
    pub host_name: String,
    pub cols: u16,
    pub rows: u16,
    pub status: String,
    pub created_at: u64,
}

/// POST /api/terminal/sessions — create a new terminal session.
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    // Check session limit
    {
        let sessions = mgr.sessions.read().await;
        if sessions.len() >= mgr.config.max_sessions {
            return Err((StatusCode::TOO_MANY_REQUESTS, format!(
                "Max {} sessions reached", mgr.config.max_sessions
            )));
        }
    }

    // Look up host
    let host = super::hosts::get_host_by_id(&mgr.db_path, req.host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let session_id = uuid::Uuid::new_v4().to_string();

    if host.is_local {
        // Spawn local PTY
        let shell = if host.default_shell.is_empty() { "/bin/bash" } else { &host.default_shell };
        let pty = super::pty::spawn_pty(shell, req.cols, req.rows)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        let session = LiveSession {
            id: session_id.clone(),
            host_id: req.host_id,
            cols: req.cols,
            rows: req.rows,
            scrollback: RingBuffer::new(mgr.config.scrollback_bytes),
            output_tx: pty.output_tx,
            input_tx: pty.input_tx,
            created_at: Instant::now(),
            last_active: Instant::now(),
            backend: SessionBackend::LocalPty {
                master_fd: pty.master_fd,
                child_pid: pty.child_pid,
            },
            recording: None,
        };

        let session_arc = Arc::new(tokio::sync::Mutex::new(session));
        mgr.sessions.write().await.insert(session_id.clone(), session_arc);

        info!("[terminal:session] created local PTY session {} for host '{}'", session_id, host.name);
    } else {
        // SSH connection
        let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
        let key_path = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());

        let ssh_client = super::ssh::connect_ssh(
            &host.hostname,
            host.port,
            &host.username,
            &key_path,
            req.cols,
            req.rows,
        ).await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("SSH: {}", e)))?;

        let session = LiveSession {
            id: session_id.clone(),
            host_id: req.host_id,
            cols: req.cols,
            rows: req.rows,
            scrollback: RingBuffer::new(mgr.config.scrollback_bytes),
            output_tx: ssh_client.output_tx.clone(),
            input_tx: ssh_client.input_tx.clone(),
            created_at: Instant::now(),
            last_active: Instant::now(),
            backend: SessionBackend::Ssh {
                client: Arc::new(ssh_client),
            },
            recording: None,
        };

        let session_arc = Arc::new(tokio::sync::Mutex::new(session));
        mgr.sessions.write().await.insert(session_id.clone(), session_arc);

        info!("[terminal:session] created SSH session {} to {}@{}", session_id, host.username, host.hostname);
    }

    Ok(Json(json!({
        "session_id": session_id,
        "host_id": req.host_id,
    })))
}

/// GET /api/terminal/sessions — list active sessions.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let sessions = mgr.sessions.read().await;
    let mut list = Vec::new();

    for (id, session) in sessions.iter() {
        let sess = session.lock().await;
        let host = super::hosts::get_host_by_id(&mgr.db_path, sess.host_id)
            .unwrap_or_else(|_| super::TerminalHost {
                id: sess.host_id,
                name: "unknown".into(),
                hostname: "".into(),
                port: 22,
                username: "".into(),
                auth_method: "key".into(),
                private_key: None,
                password: None,
                jump_host_id: None,
                default_shell: "/bin/bash".into(),
                group_name: "".into(),
                tags: "[]".into(),
                color: "#0ea5e9".into(),
                sort_order: 0,
                is_local: false,
                favorite: false,
            });
        list.push(json!({
            "id": id,
            "host_id": sess.host_id,
            "host_name": host.name,
            "cols": sess.cols,
            "rows": sess.rows,
            "created_at": sess.created_at.elapsed().as_secs(),
        }));
    }

    Ok(Json(json!({ "sessions": list })))
}

/// DELETE /api/terminal/sessions/{id} — kill a session.
pub async fn kill_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let removed = mgr.sessions.write().await.remove(&session_id);
    if let Some(session) = removed {
        let sess = session.lock().await;
        match &sess.backend {
            SessionBackend::LocalPty { child_pid, master_fd } => {
                super::pty::kill_pty(*child_pid);
                unsafe { libc::close(*master_fd); }
            }
            SessionBackend::Ssh { client } => {
                client.close().await;
            }
        }
        info!("[terminal:session] killed session {}", session_id);
        Ok(Json(json!({"success": true})))
    } else {
        Err((StatusCode::NOT_FOUND, "session not found".into()))
    }
}
