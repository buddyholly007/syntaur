//! WebSocket endpoint for browser-based voice input.
//!
//! Bridges the browser's WebSocket audio to the Parakeet STT server
//! via the Wyoming protocol (TCP). The browser sends raw int16 PCM
//! chunks, we accumulate them, and when the client sends a "stop"
//! message, we forward the audio to STT and return the transcript.
//!
//! Route: GET /ws/stt (WebSocket upgrade)
//!
//! Protocol (client → server):
//!   - Text: `{"type":"start"}` — begin recording session
//!   - Binary: raw int16 LE PCM at 16kHz mono
//!   - Text: `{"type":"stop"}` — end session, trigger STT, get transcript
//!
//! Protocol (server → client):
//!   - Text: `{"type":"transcript","text":"..."}` — STT result
//!   - Text: `{"type":"error","message":"..."}` — error

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const STT_HOST: &str = "127.0.0.1:10300";

/// Axum handler for WebSocket upgrade.
pub async fn ws_stt_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_stt_session)
}

async fn handle_stt_session(mut socket: WebSocket) {
    let mut pcm_buffer: Vec<i16> = Vec::new();
    let mut active = false;

    while let Some(msg) = socket.recv().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                debug!("[ws/stt] recv error: {}", e);
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                let text = text.as_str();
                if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(text) {
                    match cmd.get("type").and_then(|t| t.as_str()) {
                        Some("start") => {
                            pcm_buffer.clear();
                            active = true;
                            debug!("[ws/stt] session started");
                        }
                        Some("stop") => {
                            if !active || pcm_buffer.len() < 1600 {
                                // < 0.1s, skip
                                let _ = socket.send(Message::Text(
                                    serde_json::json!({"type":"transcript","text":""}).to_string().into()
                                )).await;
                                active = false;
                                continue;
                            }
                            active = false;

                            info!("[ws/stt] processing {:.1}s audio",
                                pcm_buffer.len() as f64 / 16000.0);

                            // Run STT
                            match run_wyoming_stt(&pcm_buffer).await {
                                Ok(text) => {
                                    info!("[ws/stt] transcript: {}",
                                        &text[..text.len().min(80)]);
                                    let resp = serde_json::json!({
                                        "type": "transcript",
                                        "text": text,
                                    });
                                    let _ = socket.send(Message::Text(resp.to_string().into())).await;
                                }
                                Err(e) => {
                                    warn!("[ws/stt] STT failed: {}", e);
                                    let resp = serde_json::json!({
                                        "type": "error",
                                        "message": format!("STT failed: {}", e),
                                    });
                                    let _ = socket.send(Message::Text(resp.to_string().into())).await;
                                }
                            }

                            pcm_buffer.clear();
                        }
                        _ => {}
                    }
                }
            }
            Message::Binary(data) => {
                let data = data.as_ref();
                if !active || data.len() % 2 != 0 {
                    continue;
                }
                // Decode int16 LE PCM
                let samples: Vec<i16> = data
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();
                pcm_buffer.extend_from_slice(&samples);
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    debug!("[ws/stt] session closed");
}

/// Send PCM audio to Parakeet STT via Wyoming protocol, return transcript.
async fn run_wyoming_stt(pcm: &[i16]) -> Result<String, String> {
    let audio_bytes: Vec<u8> = pcm.iter().flat_map(|&s| s.to_le_bytes()).collect();

    let stream = TcpStream::connect(STT_HOST)
        .await
        .map_err(|e| format!("connect: {}", e))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // audio-start
    let h = serde_json::json!({
        "type": "audio-start", "version": "1.0.0",
        "data_length": 0, "payload_length": 0,
    });
    write_half.write_all(format!("{}\n", h).as_bytes()).await
        .map_err(|e| format!("write: {}", e))?;

    // audio-chunk
    let h = serde_json::json!({
        "type": "audio-chunk", "version": "1.0.0",
        "data_length": 0, "payload_length": audio_bytes.len(),
    });
    write_half.write_all(format!("{}\n", h).as_bytes()).await
        .map_err(|e| format!("write: {}", e))?;
    write_half.write_all(&audio_bytes).await
        .map_err(|e| format!("write audio: {}", e))?;

    // audio-stop
    let h = serde_json::json!({
        "type": "audio-stop", "version": "1.0.0",
        "data_length": 0, "payload_length": 0,
    });
    write_half.write_all(format!("{}\n", h).as_bytes()).await
        .map_err(|e| format!("write: {}", e))?;
    write_half.flush().await.map_err(|e| format!("flush: {}", e))?;

    // Read response
    let mut line = String::new();
    reader.read_line(&mut line).await
        .map_err(|e| format!("read: {}", e))?;

    let header: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("parse: {}", e))?;

    let data_len = header.get("data_length")
        .and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    if data_len > 0 {
        let mut buf = vec![0u8; data_len];
        reader.read_exact(&mut buf).await
            .map_err(|e| format!("read data: {}", e))?;
        let data: serde_json::Value = serde_json::from_slice(&buf)
            .map_err(|e| format!("parse data: {}", e))?;
        Ok(data.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
    } else {
        Ok(String::new())
    }
}
