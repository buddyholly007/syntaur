//! WebSocket handler for terminal I/O bridging.

use std::sync::Arc;

use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::extract::ws::{Message, WebSocket};
use axum::response::IntoResponse;
use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use serde_json::json;

use crate::AppState;

/// GET /ws/terminal/{session_id}?token=...
pub async fn ws_terminal_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    // Auth check
    let token = params.get("token").cloned().unwrap_or_default();
    if token.is_empty() {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    if let Ok(None) | Err(_) = state.users.resolve_token(&token).await {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    ws.on_upgrade(move |socket| handle_terminal_ws(socket, state, session_id))
        .into_response()
}

async fn handle_terminal_ws(socket: WebSocket, state: Arc<AppState>, session_id: String) {
    let (mut ws_sink, mut ws_stream) = socket.split();

    // Find session
    let mgr = match &state.terminal {
        Some(m) => m,
        None => {
            let _ = ws_sink.send(Message::Text(
                json!({"type":"error","message":"terminal module disabled"}).to_string().into(),
            )).await;
            return;
        }
    };

    let session = {
        let sessions = mgr.sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let session = match session {
        Some(s) => s,
        None => {
            let _ = ws_sink.send(Message::Text(
                json!({"type":"error","message":"session not found"}).to_string().into(),
            )).await;
            return;
        }
    };

    // Send scrollback
    {
        let sess = session.lock().await;
        let sb = sess.scrollback.as_bytes();
        if !sb.is_empty() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(sb);
            let _ = ws_sink.send(Message::Text(
                json!({"type":"scrollback","data": b64}).to_string().into(),
            )).await;
        }
    }

    // Get channels
    let (mut output_rx, input_tx) = {
        let sess = session.lock().await;
        (sess.output_tx.subscribe(), sess.input_tx.clone())
    };

    info!("[terminal:ws] client attached to session {}", session_id);

    // Bidirectional relay
    loop {
        tokio::select! {
            // PTY output → WebSocket
            Ok(data) = output_rx.recv() => {
                // Also record in scrollback
                {
                    let mut sess = session.lock().await;
                    sess.scrollback.extend(&data);
                    sess.last_active = std::time::Instant::now();
                }
                if ws_sink.send(Message::Binary(data.to_vec().into())).await.is_err() {
                    break;
                }
            }
            // WebSocket input → PTY
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        let _ = input_tx.send(Bytes::from(data.to_vec())).await;
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            match cmd.get("type").and_then(|t| t.as_str()) {
                                Some("resize") => {
                                    let cols = cmd.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                                    let rows = cmd.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                                    let mut sess = session.lock().await;
                                    sess.cols = cols;
                                    sess.rows = rows;
                                    match &sess.backend {
                                        super::SessionBackend::LocalPty { master_fd, .. } => {
                                            super::pty::resize_pty(*master_fd, cols, rows);
                                        }
                                        super::SessionBackend::Ssh { client } => {
                                            client.resize(cols, rows).await;
                                        }
                                    }
                                }
                                Some("ping") => {
                                    let _ = ws_sink.send(Message::Text(
                                        json!({"type":"pong"}).to_string().into(),
                                    )).await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("[terminal:ws] client detached from session {}", session_id);
}
