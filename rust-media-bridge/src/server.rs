//! HTTP + WebSocket server. Listens on 127.0.0.1:18790 for commands from
//! the Syntaur gateway / browser tab. All endpoints are localhost-only by
//! default — remote machines can't command audio playback here.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use crate::events::{
    BridgeEvent, DuckRequest, PlayRequest, SeekRequest, StatusResponse, VolumeRequest,
};
use crate::providers;
use crate::state::BridgeState;

pub const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn serve(bind: &str, state: Arc<BridgeState>) -> Result<()> {
    // Allow browser tabs from any origin to reach us — the port is
    // localhost-only so the only callers are the user's own browser on
    // the same machine.
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/status", get(handle_status))
        .route("/play", post(handle_play))
        .route("/pause", post(handle_pause))
        .route("/resume", post(handle_resume))
        .route("/stop", post(handle_stop))
        .route("/next", post(handle_next))
        .route("/prev", post(handle_prev))
        .route("/seek", post(handle_seek))
        .route("/volume", post(handle_volume))
        .route("/duck", post(handle_duck))
        .route("/ws", get(handle_ws))
        .layer(cors)
        .with_state(state);

    let listener = TcpListener::bind(bind).await?;
    log::info!("syntaur-media-bridge listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_status(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    let browser = state.browser.lock().await;
    let chromium_ready = browser.as_ref().map(|b| b.is_alive()).unwrap_or(false);
    drop(browser);

    let audio_backend = state
        .audio_backend
        .clone()
        .unwrap_or_else(|| crate::audio::default_backend_name().to_string());

    let now_playing = state.now_playing.read().await.clone();

    let authed_providers =
        crate::auth::detect_authed_providers(&state.data_dir).unwrap_or_default();

    Json(StatusResponse {
        running: true,
        version: BRIDGE_VERSION.to_string(),
        chromium_ready,
        authed_providers,
        audio_backend,
        now_playing,
    })
}

async fn handle_play(
    State(state): State<Arc<BridgeState>>,
    Json(req): Json<PlayRequest>,
) -> impl IntoResponse {
    match providers::play(&state, &req).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => {
            log::error!("play failed: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": e.to_string()})),
            )
        }
    }
}

async fn handle_pause(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    match providers::pause(&state).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_resume(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    match providers::resume(&state).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_stop(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    match providers::stop(&state).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_next(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    match providers::next_track(&state).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_prev(State(state): State<Arc<BridgeState>>) -> impl IntoResponse {
    match providers::prev_track(&state).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_seek(
    State(state): State<Arc<BridgeState>>,
    Json(req): Json<SeekRequest>,
) -> impl IntoResponse {
    match providers::seek(&state, req.position_ms).await {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => internal_error(e),
    }
}

async fn handle_volume(
    State(state): State<Arc<BridgeState>>,
    Json(req): Json<VolumeRequest>,
) -> impl IntoResponse {
    let level = req.level.clamp(0.0, 1.0);
    // Remember user's desired level so unducking restores it
    {
        let mut d = state.ducking.write().await;
        d.user_volume = level;
    }
    // Apply to audio pipeline (instant, local)
    if let Ok(Some(audio)) = state.ensure_audio().await {
        audio.set_volume(level).await;
    }
    // Also set provider-side volume when possible — belt-and-suspenders;
    // some services have their own loudness normalization that's nicer
    // when volume is on the player rather than our pipeline.
    let _ = providers::set_provider_volume(&state, level).await;
    state.emit(BridgeEvent::Volume { level });
    (StatusCode::OK, Json(json!({"ok": true, "level": level})))
}

async fn handle_duck(
    State(state): State<Arc<BridgeState>>,
    Json(req): Json<DuckRequest>,
) -> impl IntoResponse {
    let target_level = req.level.unwrap_or(0.2).clamp(0.0, 1.0);

    let effective = {
        let mut d = state.ducking.write().await;
        d.active = req.active;
        if let Some(l) = req.level {
            d.level = l.clamp(0.0, 1.0);
        }
        if req.active {
            target_level
        } else {
            d.user_volume
        }
    };

    if let Ok(Some(audio)) = state.ensure_audio().await {
        audio.set_volume(effective).await;
    }
    state.emit(BridgeEvent::Ducking { active: req.active });
    (
        StatusCode::OK,
        Json(json!({"ok": true, "active": req.active, "effective_level": effective})),
    )
}

fn internal_error(e: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    log::error!("bridge error: {:#}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"ok": false, "error": e.to_string()})),
    )
}

async fn handle_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<BridgeState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: Arc<BridgeState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.events_tx.subscribe();

    // Initial hello so client has baseline state
    let authed = crate::auth::detect_authed_providers(&state.data_dir).unwrap_or_default();
    let chromium_ready = state
        .browser
        .lock()
        .await
        .as_ref()
        .map(|b| b.is_alive())
        .unwrap_or(false);
    let hello = BridgeEvent::Hello {
        bridge_version: BRIDGE_VERSION.to_string(),
        chromium_ready,
        authed_providers: authed,
    };
    if let Ok(text) = serde_json::to_string(&hello) {
        let _ = sender.send(Message::Text(text.into())).await;
    }

    let send_task = tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let text = match serde_json::to_string(&ev) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Receive loop just drains client messages. The WS is mostly server->
    // client (event push); we accept client pings/pongs and ignore the rest.
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}
