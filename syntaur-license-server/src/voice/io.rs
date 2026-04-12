//! Universal voice I/O — works from any web browser or HTTP client.
//!
//! No special hardware needed. Supports:
//! - POST /api/v1/stt — upload audio → get transcript
//! - POST /api/v1/tts — send text → get audio URL
//! - POST /api/v1/voice — full round-trip: audio in → transcript → LLM → TTS → audio URL
//!
//! STT backend: configurable Wyoming endpoint (Parakeet) or OpenAI Whisper API
//! TTS backend: configurable Wyoming endpoint (Fish Audio) or OpenAI TTS API

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Configuration for STT/TTS backends.
#[derive(Debug, Clone)]
pub struct VoiceIoConfig {
    /// Wyoming STT server URL (e.g. http://192.168.1.35:10300)
    pub stt_url: Option<String>,
    /// Wyoming TTS server URL (e.g. http://192.168.1.69:10400)
    pub tts_url: Option<String>,
    /// OpenAI-compatible STT URL (e.g. https://api.openai.com/v1/audio/transcriptions)
    pub openai_stt_url: Option<String>,
    /// OpenAI-compatible TTS URL (e.g. https://api.openai.com/v1/audio/speech)
    pub openai_tts_url: Option<String>,
    /// API key for OpenAI STT/TTS
    pub openai_api_key: Option<String>,
}

impl VoiceIoConfig {
    pub fn from_env() -> Self {
        Self {
            stt_url: std::env::var("STT_URL").ok(),
            tts_url: std::env::var("TTS_URL").ok(),
            openai_stt_url: std::env::var("OPENAI_STT_URL").ok(),
            openai_tts_url: std::env::var("OPENAI_TTS_URL").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
        }
    }

    pub fn has_stt(&self) -> bool {
        self.stt_url.is_some() || self.openai_stt_url.is_some()
    }

    pub fn has_tts(&self) -> bool {
        // Always true — Edge TTS is the built-in default (zero config)
        true
    }
}

pub struct VoiceIoState {
    pub config: VoiceIoConfig,
    pub client: reqwest::Client,
    /// TTS audio cache: id → (wav_bytes, created_at)
    pub tts_cache: Mutex<HashMap<String, (Vec<u8>, std::time::Instant)>>,
}

// ── STT ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SttResponse {
    pub text: String,
    pub duration_ms: u64,
    pub backend: String,
}

/// POST /api/v1/stt — accepts audio bytes, returns transcript.
/// Content-Type should be audio/wav, audio/webm, or audio/ogg.
pub async fn handle_stt(
    State(state): State<Arc<VoiceIoState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SttResponse>, (StatusCode, String)> {
    let start = std::time::Instant::now();
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/wav");

    info!("[stt] received {} bytes ({})", body.len(), content_type);

    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty audio body".into()));
    }

    // Try Wyoming STT first, then OpenAI-compatible
    if let Some(ref url) = state.config.stt_url {
        match wyoming_stt(&state.client, url, &body).await {
            Ok(text) => {
                return Ok(Json(SttResponse {
                    text,
                    duration_ms: start.elapsed().as_millis() as u64,
                    backend: "wyoming".into(),
                }));
            }
            Err(e) => warn!("[stt] wyoming failed: {}", e),
        }
    }

    if let (Some(ref url), Some(ref key)) =
        (&state.config.openai_stt_url, &state.config.openai_api_key)
    {
        match openai_stt(&state.client, url, key, &body, content_type).await {
            Ok(text) => {
                return Ok(Json(SttResponse {
                    text,
                    duration_ms: start.elapsed().as_millis() as u64,
                    backend: "openai".into(),
                }));
            }
            Err(e) => warn!("[stt] openai failed: {}", e),
        }
    }

    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        "Speech-to-text is not configured. Set STT_URL (for Wyoming/Parakeet) or OPENAI_STT_URL + OPENAI_API_KEY (for Whisper) in your environment.\n\n\
        Get an OpenAI API key for Whisper STT:\nhttps://platform.openai.com/api-keys".into(),
    ))
}

async fn wyoming_stt(client: &reqwest::Client, base_url: &str, audio: &[u8]) -> Result<String, String> {
    // Wyoming uses a simple HTTP POST with audio body
    let url = format!("{}/stt", base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("Content-Type", "audio/wav")
        .body(audio.to_vec())
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Can't reach STT server at {} — check that it's running: {}", base_url, e))?;

    if !resp.status().is_success() {
        return Err(format!("STT server returned HTTP {} — it may be overloaded or misconfigured", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("STT server returned an unexpected response format: {}", e))?;

    Ok(body
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

async fn openai_stt(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    audio: &[u8],
    content_type: &str,
) -> Result<String, String> {
    let ext = match content_type {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mp3" | "audio/mpeg" => "mp3",
        _ => "wav",
    };

    let part = reqwest::multipart::Part::bytes(audio.to_vec())
        .file_name(format!("audio.{}", ext))
        .mime_str(content_type)
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .part("file", part);

    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Can't reach OpenAI STT service: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let _body = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 => " — check your OPENAI_API_KEY.\n\nManage API keys:\nhttps://platform.openai.com/api-keys",
            429 => " — rate limited, try again shortly.\n\nCheck usage limits:\nhttps://platform.openai.com/usage",
            _ => "",
        };
        return Err(format!("OpenAI STT returned HTTP {}{}", status, hint));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("OpenAI STT returned an unexpected response format: {}", e))?;

    Ok(body
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

// ── TTS ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default = "default_voice")]
    pub voice: String,
}

fn default_voice() -> String {
    "alloy".into()
}

#[derive(Serialize)]
pub struct TtsResponse {
    pub audio_url: String,
    pub duration_ms: u64,
    pub backend: String,
}

/// POST /api/v1/tts — accepts text, returns audio URL.
pub async fn handle_tts(
    State(state): State<Arc<VoiceIoState>>,
    Json(req): Json<TtsRequest>,
) -> Result<Json<TtsResponse>, (StatusCode, String)> {
    let start = std::time::Instant::now();

    if req.text.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty text".into()));
    }

    info!("[tts] generating for {} chars", req.text.len());

    let audio = generate_tts(&state, &req.text, &req.voice).await?;

    // Cache the audio
    let id = uuid::Uuid::new_v4().to_string();
    let url = format!("/api/v1/tts/audio/{}.wav", id);

    {
        let mut cache = state.tts_cache.lock().await;
        cache.insert(id.clone(), (audio, std::time::Instant::now()));

        // Evict old entries (> 5 min)
        let now = std::time::Instant::now();
        cache.retain(|_, (_, created)| now.duration_since(*created).as_secs() < 300);
    }

    Ok(Json(TtsResponse {
        audio_url: url,
        duration_ms: start.elapsed().as_millis() as u64,
        backend: if state.config.tts_url.is_some() {
            "wyoming"
        } else {
            "openai"
        }
        .into(),
    }))
}

/// GET /api/v1/tts/audio/{id}.wav — serve cached TTS audio.
pub async fn handle_tts_audio(
    State(state): State<Arc<VoiceIoState>>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Result<(HeaderMap, Vec<u8>), StatusCode> {
    let id = filename.trim_end_matches(".wav");
    let mut cache = state.tts_cache.lock().await;
    let (audio, _) = cache.remove(id).ok_or(StatusCode::NOT_FOUND)?;

    let mut headers = HeaderMap::new();
    headers.insert("content-type", "audio/wav".parse().unwrap());
    headers.insert(
        "content-length",
        audio.len().to_string().parse().unwrap(),
    );
    Ok((headers, audio))
}

async fn generate_tts(
    state: &VoiceIoState,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, (StatusCode, String)> {
    // Try Wyoming TTS first
    if let Some(ref url) = state.config.tts_url {
        match wyoming_tts(&state.client, url, text).await {
            Ok(audio) => return Ok(audio),
            Err(e) => warn!("[tts] wyoming failed: {}", e),
        }
    }

    // Try OpenAI TTS
    if let (Some(ref url), Some(ref key)) =
        (&state.config.openai_tts_url, &state.config.openai_api_key)
    {
        match openai_tts(&state.client, url, key, text, voice).await {
            Ok(audio) => return Ok(audio),
            Err(e) => warn!("[tts] openai failed: {}", e),
        }
    }

    // Default fallback: Edge TTS (free, zero config, neural quality)
    match super::edge_tts::synthesize(text, voice).await {
        Ok(audio) => Ok(audio),
        Err(e) => {
            warn!("[tts] edge-tts failed: {}", e);
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Voice synthesis is temporarily unavailable — all TTS backends failed. Last error: {}", e),
            ))
        }
    }
}

async fn wyoming_tts(client: &reqwest::Client, base_url: &str, text: &str) -> Result<Vec<u8>, String> {
    let url = format!("{}/tts", base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({"text": text}))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Can't reach TTS server at {} — check that it's running: {}", base_url, e))?;

    if !resp.status().is_success() {
        return Err(format!("TTS server returned HTTP {} — it may be overloaded or misconfigured", resp.status()));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("TTS server returned audio but it couldn't be read: {}", e))
}

async fn openai_tts(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, String> {
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "tts-1",
            "input": text,
            "voice": voice,
            "response_format": "wav",
        }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Can't reach OpenAI TTS service: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let hint = match status.as_u16() {
            401 => " — check your OPENAI_API_KEY.\n\nManage API keys:\nhttps://platform.openai.com/api-keys",
            429 => " — rate limited, try again shortly.\n\nCheck usage limits:\nhttps://platform.openai.com/usage",
            _ => "",
        };
        return Err(format!("OpenAI TTS returned HTTP {}{}", status, hint));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("OpenAI TTS returned audio but it couldn't be read: {}", e))
}

// Voice round-trip (audio → STT → LLM → TTS) is done client-side by
// chaining: POST /api/v1/stt → POST /api/v1/chat → POST /api/v1/tts.
// This keeps each endpoint independently testable and avoids dual-state
// extraction complexity in axum.
