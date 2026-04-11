//! rust-voice-pipeline — Pure-Rust voice pipeline for Peter.
//!
//! Two modes:
//!
//! ## STT mode (--mode stt)
//! Wyoming STT server — accepts audio from HA's Wyoming integration,
//! runs Parakeet inference locally, returns transcript. Drop-in
//! replacement for HA's Whisper addon.
//!
//! ## Pipeline mode (--mode pipeline, future)
//! Full pipeline — satellite connects directly. Handles wake word
//! detection, STT, intent matching, LLM via syntaur, TTS routing,
//! and audio response. Replaces HA entirely for voice.
//!
//! ## Running
//!
//! ```bash
//! # STT server for HA Wyoming integration
//! RUST_LOG=info rust-voice-pipeline --mode stt --port 10300 --model-dir /opt/models
//!
//! # Full pipeline (future)
//! RUST_LOG=info rust-voice-pipeline --mode pipeline --port 10500 \
//!   --syntaur-url http://192.168.1.35:18789 \
//!   --syntaur-secret <voice_secret> \
//!   --tts-url http://192.168.1.69:10400
//! ```

mod wyoming;
mod stt;
mod intent;

use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tracing::{error, info, warn};

#[derive(Clone)]
struct Config {
    mode: String,
    port: u16,
    model_dir: String,
    syntaur_url: String,
    syntaur_secret: String,
    tts_url: String,
}

impl Config {
    fn from_args() -> Self {
        let mut mode = "stt".to_string();
        let mut port = 10300u16;
        let mut model_dir = "/opt/models/stt".to_string();
        let mut syntaur_url = "http://192.168.1.35:18789".to_string();
        let mut syntaur_secret = String::new();
        let mut tts_url = "http://192.168.1.69:10400".to_string();

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--mode" => { i += 1; mode = args.get(i).cloned().unwrap_or(mode); }
                "--port" => { i += 1; port = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(port); }
                "--model-dir" => { i += 1; model_dir = args.get(i).cloned().unwrap_or(model_dir); }
                "--syntaur-url" => { i += 1; syntaur_url = args.get(i).cloned().unwrap_or(syntaur_url); }
                "--syntaur-secret" => { i += 1; syntaur_secret = args.get(i).cloned().unwrap_or_default(); }
                "--tts-url" => { i += 1; tts_url = args.get(i).cloned().unwrap_or(tts_url); }
                _ => {}
            }
            i += 1;
        }

        Self { mode, port, model_dir, syntaur_url, syntaur_secret, tts_url }
    }
}

// ── Wyoming STT server mode ─────────────────────────────────────────────────

/// Handle a single Wyoming STT session.
/// Receives audio-start/chunk/stop, runs Parakeet, returns transcript.
async fn handle_stt_session(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    engine: Arc<stt::SttEngine>,
) {
    info!("[stt-session] connection from {}", peer);

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let mut audio_buf: Vec<u8> = Vec::new();

    loop {
        let msg = match wyoming::read_message(&mut reader).await {
            Some(m) => m,
            None => break,
        };

        match msg.msg_type.as_str() {
            "describe" => {
                // HA sends "describe" to discover what this Wyoming server offers.
                let info_data = serde_json::json!({
                    "asr": [{
                        "name": "Parakeet TDT 0.6B v3",
                        "description": "NVIDIA Parakeet TDT speech-to-text, INT8 quantized, CPU via sherpa-onnx",
                        "attribution": {
                            "name": "NVIDIA NeMo",
                            "url": "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3"
                        },
                        "installed": true,
                        "version": "0.6b-v3-int8",
                        "models": [{
                            "name": "parakeet-tdt-0.6b-v3-int8",
                            "description": "Parakeet TDT 0.6B v3 INT8",
                            "attribution": {
                                "name": "NVIDIA NeMo",
                                "url": "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3"
                            },
                            "installed": true,
                            "version": "0.6b-v3-int8",
                            "languages": ["en"]
                        }]
                    }],
                    "tts": [],
                    "handle": [],
                    "intent": [],
                    "wake": [],
                    "mic": [],
                    "snd": []
                });
                let _ = wyoming::write_message(
                    &mut write_half,
                    &wyoming::WyomingMessage::new("info").with_data(info_data),
                ).await;
            }
            "audio-start" => {
                audio_buf.clear();
            }
            "audio-chunk" => {
                audio_buf.extend_from_slice(&msg.payload);
            }
            "audio-stop" => {
                if audio_buf.is_empty() {
                    let _ = wyoming::write_message(
                        &mut write_half,
                        &wyoming::WyomingMessage::transcript(""),
                    ).await;
                    continue;
                }

                match engine.transcribe(&audio_buf).await {
                    Ok(text) => {
                        let _ = wyoming::write_message(
                            &mut write_half,
                            &wyoming::WyomingMessage::transcript(&text),
                        ).await;
                    }
                    Err(e) => {
                        error!("[stt-session] transcription failed: {}", e);
                        let _ = wyoming::write_message(
                            &mut write_half,
                            &wyoming::WyomingMessage::transcript(""),
                        ).await;
                    }
                }
                audio_buf.clear();
            }
            other => {
                tracing::debug!("[stt-session] ignoring message type: {}", other);
            }
        }
    }

    info!("[stt-session] {} disconnected", peer);
}

// ── Full pipeline mode (future) ─────────────────────────────────────────────

async fn handle_pipeline_session(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    engine: Arc<stt::SttEngine>,
    http: Arc<reqwest::Client>,
    config: Arc<Config>,
) {
    info!("[pipeline] connection from {}", peer);

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let mut audio_buf: Vec<u8> = Vec::new();

    loop {
        let msg = match wyoming::read_message(&mut reader).await {
            Some(m) => m,
            None => break,
        };

        match msg.msg_type.as_str() {
            "audio-start" => {
                audio_buf.clear();
            }
            "audio-chunk" => {
                audio_buf.extend_from_slice(&msg.payload);
            }
            "audio-stop" => {
                if audio_buf.is_empty() {
                    continue;
                }

                // 1. STT
                let transcript = match engine.transcribe(&audio_buf).await {
                    Ok(t) if !t.is_empty() => t,
                    Ok(_) => {
                        audio_buf.clear();
                        continue;
                    }
                    Err(e) => {
                        error!("[pipeline] STT failed: {}", e);
                        audio_buf.clear();
                        continue;
                    }
                };

                let _ = wyoming::write_message(
                    &mut write_half,
                    &wyoming::WyomingMessage::transcript(&transcript),
                ).await;

                // 2. LLM via syntaur
                let response = match call_syntaur_voice_chat(&http, &config, &transcript).await {
                    Ok(text) => text,
                    Err(e) => {
                        error!("[pipeline] LLM failed: {}", e);
                        "Sorry, I'm having trouble right now.".to_string()
                    }
                };

                info!("[pipeline] response: '{}'", response.chars().take(100).collect::<String>());

                // 3. Send synthesize request back
                let _ = wyoming::write_message(
                    &mut write_half,
                    &wyoming::WyomingMessage::synthesize(&response),
                ).await;

                audio_buf.clear();
            }
            "run-pipeline" => {
                info!("[pipeline] run-pipeline request");
            }
            _ => {}
        }
    }

    info!("[pipeline] {} disconnected", peer);
}

async fn call_syntaur_voice_chat(
    http: &reqwest::Client,
    config: &Config,
    transcript: &str,
) -> Result<String, String> {
    let url = format!("{}/v1/chat/completions", config.syntaur_url);
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": transcript}],
    });

    let mut req = http.post(&url).json(&body);
    if !config.syntaur_secret.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", config.syntaur_secret));
    }

    let resp = req
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("syntaur: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("syntaur: HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
    body.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "empty response".to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    let config = Arc::new(Config::from_args());

    // Load Parakeet model
    info!("rust-voice-pipeline v0.1.0");
    info!("  mode: {}", config.mode);
    info!("  model-dir: {}", config.model_dir);

    let engine = match stt::SttEngine::new(&config.model_dir).await {
        Ok(e) => Arc::new(e),
        Err(e) => {
            error!("Failed to load STT model: {}", e);
            std::process::exit(1);
        }
    };

    let bind = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&bind).await.expect("bind failed");
    info!("listening on {}", bind);

    match config.mode.as_str() {
        "stt" => {
            info!("Wyoming STT server mode — waiting for connections");
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let engine = Arc::clone(&engine);
                        tokio::spawn(handle_stt_session(stream, peer, engine));
                    }
                    Err(e) => error!("accept: {}", e),
                }
            }
        }
        "pipeline" => {
            let http = Arc::new(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap_or_default(),
            );
            info!("Full pipeline mode");
            info!("  syntaur: {}", config.syntaur_url);
            info!("  TTS: {}", config.tts_url);
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let engine = Arc::clone(&engine);
                        let http = Arc::clone(&http);
                        let config = Arc::clone(&config);
                        tokio::spawn(handle_pipeline_session(stream, peer, engine, http, config));
                    }
                    Err(e) => error!("accept: {}", e),
                }
            }
        }
        other => {
            error!("unknown mode: {}. Use --mode stt or --mode pipeline", other);
            std::process::exit(1);
        }
    }
}
