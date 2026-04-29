//! WebSocket handler for terminal I/O bridging.

use std::sync::Arc;

use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::extract::ws::{Message, WebSocket};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use serde_json::json;

use crate::AppState;
use crate::security::extract_session_token;

/// GET /ws/terminal/{session_id}?stream_token=…  (preferred)
///                        ?token=…                (DEPRECATED, long-lived)
///
/// Auth resolution order:
///   1. `?stream_token=` query param — short-lived URL-scoped, preferred
///   2. `Authorization: Bearer` header — non-browser callers
///   3. `syntaur_token` HttpOnly cookie — post-cookie-auth fallback so
///      sessionStorage-empty browsers don't 401 on upgrade
///   4. `?token=` query param — long-lived session token, DEPRECATED
///      (logs every hit so we can spot any remaining UI call sites
///      before sunsetting the path)
pub async fn ws_terminal_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_path = format!("/ws/terminal/{session_id}");

    // 1. Stream-token path first.
    if let Some(st) = params.get("stream_token") {
        match state.stream_tokens.resolve(st, &request_path) {
            Some(_) => return ws.on_upgrade(move |socket| handle_terminal_ws(socket, state, session_id)).into_response(),
            None => {
                warn!("[ws/terminal] invalid/expired stream_token for {request_path}");
                return axum::http::StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // 2-3. Authorization header or cookie via existing helper.
    let mut token = extract_session_token(&headers);

    // 4. Legacy ?token= — REJECTED by default in v0.6.1+ (same flip as
    // resolve_principal_for_stream). Operators who still need the old
    // accept-with-warn behavior can set SYNTAUR_ALLOW_LEGACY_STREAM_TOKEN=1.
    if token.is_empty() {
        if let Some(legacy) = params.get("token") {
            let allow_legacy = std::env::var("SYNTAUR_ALLOW_LEGACY_STREAM_TOKEN")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allow_legacy {
                warn!(
                    "[ws/terminal] REJECTED legacy ?token= on WebSocket upgrade \
                     for {request_path}: long-lived URL tokens are reject-by-default \
                     since v0.6.1. Call POST /api/auth/stream-token and pass \
                     ?stream_token= instead."
                );
                return axum::http::StatusCode::UNAUTHORIZED.into_response();
            }
            warn!(
                "[ws/terminal] DEPRECATED: long-lived ?token= on WebSocket \
                 upgrade for {request_path}. SYNTAUR_ALLOW_LEGACY_STREAM_TOKEN=1 \
                 is keeping this path alive — migrate to ?stream_token=."
            );
            token = legacy.clone();
        }
    }

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
