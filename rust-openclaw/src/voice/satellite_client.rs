//! Satellite client — connects to the ESPHome satellite and runs the voice pipeline.
//!
//! This replaces HA's role entirely. openclaw connects to the satellite
//! as an ESPHome native API client, subscribes to voice assistant events,
//! and handles the full pipeline: STT → LLM → TTS.

use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{Duration, interval, timeout};

/// Global one-shot audio cache for serving TTS audio to the satellite's media player.
/// Keyed by random ID, auto-expires after one fetch.
static TTS_CACHE: std::sync::OnceLock<Mutex<HashMap<String, Vec<u8>>>> = std::sync::OnceLock::new();

/// Global announcement channel — timer task sends announcement text here,
/// satellite client loop picks it up and plays through the speaker.
/// Uses ArcSwap so it can be replaced on reconnect.
static ANNOUNCE_TX: std::sync::OnceLock<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>> =
    std::sync::OnceLock::new();

fn set_announce_tx(tx: tokio::sync::mpsc::UnboundedSender<String>) {
    let store = ANNOUNCE_TX.get_or_init(|| std::sync::Mutex::new(None));
    *store.lock().unwrap() = Some(tx);
}

/// Send an announcement to the satellite speaker. Called by the timer task.
pub fn announce(text: &str) {
    if let Some(store) = ANNOUNCE_TX.get() {
        if let Some(tx) = store.lock().unwrap().as_ref() {
            let _ = tx.send(text.to_string());
        }
    }
}

fn tts_cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    TTS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Store TTS audio and return the URL the satellite can fetch it from.
/// Wraps raw PCM in a WAV header if needed.
async fn cache_tts_audio(audio: Vec<u8>, gateway_port: u16, sample_rate: u32, channels: u16, bits: u16) -> String {
    let id = format!("{:016x}", rand::random::<u64>());
    let url = format!("http://192.168.1.35:{}/voice/tts/{}.wav", gateway_port, id);

    // If audio already has a WAV header, use as-is. Otherwise wrap raw PCM.
    let wav = if audio.len() > 4 && &audio[..4] == b"RIFF" {
        audio
    } else {
        build_wav_header(&audio, sample_rate, channels, bits)
    };

    tts_cache().lock().await.insert(id, wav);
    url
}

/// Build a minimal WAV file from raw PCM data.
fn build_wav_header(pcm: &[u8], sample_rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let byte_rate = sample_rate * channels as u32 * bits as u32 / 8;
    let block_align = channels * bits / 8;
    let data_size = pcm.len() as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Fetch and remove cached TTS audio. Called by the HTTP handler.
pub async fn take_tts_audio(id: &str) -> Option<Vec<u8>> {
    tts_cache().lock().await.remove(id)
}

use super::esphome_api::*;

/// Configuration for the satellite connection.
#[derive(Clone, Debug)]
pub struct SatelliteConfig {
    /// Satellite IP:port (e.g., "192.168.1.190:6053")
    pub host: String,
    /// Noise PSK (base64-encoded 32-byte key)
    pub noise_psk: String,
    /// STT server URL (rust-voice-pipeline Wyoming, e.g., "127.0.0.1:10300")
    pub stt_host: String,
    /// syntaur voice_chat URL (e.g., "http://127.0.0.1:18789")
    pub gateway_url: String,
    /// syntaur voice_chat bearer token
    pub gateway_secret: String,
    /// TTS Wyoming server (e.g., "192.168.1.69:10400")
    pub tts_host: String,
}

/// Run the satellite client loop. Connects, handles voice pipeline,
/// reconnects on disconnect. This function never returns under normal
/// operation — it runs as a background task.
pub async fn run_satellite_client(config: SatelliteConfig) {
    loop {
        info!("[satellite] connecting to {}", config.host);
        match connect_and_run(&config).await {
            Ok(()) => info!("[satellite] disconnected cleanly"),
            Err(e) => warn!("[satellite] connection error: {}", e),
        }
        info!("[satellite] reconnecting in 5s...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn connect_and_run(config: &SatelliteConfig) -> Result<(), String> {
    // Decode the Noise PSK
    use base64::Engine;
    let psk_bytes = base64::engine::general_purpose::STANDARD
        .decode(&config.noise_psk)
        .map_err(|e| format!("bad noise_psk base64: {}", e))?;
    if psk_bytes.len() != 32 {
        return Err(format!("noise_psk must be 32 bytes, got {}", psk_bytes.len()));
    }
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&psk_bytes);

    let stream = timeout(Duration::from_secs(10), TcpStream::connect(&config.host))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| format!("connect: {}", e))?;

    // Noise encrypted handshake
    let mut transport = NoiseTransport::handshake(stream, &psk).await?;

    // 1. Hello
    let hello = build_hello_request("syntaur-voice/0.1.0");
    transport.write_message(MSG_HELLO_REQUEST, &hello).await?;

    let resp = transport
        .read_message()
        .await
        .ok_or("no hello response")?;
    if resp.msg_type != MSG_HELLO_RESPONSE {
        return Err(format!("expected HelloResponse(2), got type {}", resp.msg_type));
    }
    let hello_resp = parse_hello_response(&resp.payload);
    info!(
        "[satellite] connected: {} (API v{}.{}, info={})",
        hello_resp.name,
        hello_resp.api_version_major,
        hello_resp.api_version_minor,
        hello_resp.server_info
    );

    // 2. Device info
    transport.write_message(MSG_DEVICE_INFO_REQUEST, &[]).await?;
    let resp = transport
        .read_message()
        .await
        .ok_or("no device info response")?;
    if resp.msg_type == MSG_DEVICE_INFO_RESPONSE {
        let dev = parse_device_info_response(&resp.payload);
        let flags = dev.voice_assistant_feature_flags;
        info!(
            "[satellite] device: {} flags=0x{:x} (speaker={}, api_audio={}, timers={}, announce={})",
            dev.name, flags,
            flags & 2 != 0, flags & 4 != 0, flags & 8 != 0, flags & 16 != 0,
        );
    }

    // 3. Subscribe to voice assistant with API audio
    let sub = build_subscribe_voice_assistant(true, true);
    transport.write_message(MSG_SUBSCRIBE_VOICE_ASSISTANT, &sub).await?;
    info!("[satellite] subscribed to voice assistant (API audio mode)");

    // 4. Main loop
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();
    let mut audio_buf: Vec<u8> = Vec::new();
    let mut pipeline_active = false;
    let mut conversation_id = String::new();
    let mut last_tts_time: Option<std::time::Instant> = None;
    let mut last_tts_duration_secs: u64 = 3; // default cooldown
    let mut followup_allowed = false; // allow one conversation restart for follow-up
    let mut pending_announcements: Vec<String> = Vec::new();

    // Set up the announcement channel (timer → satellite speaker)
    let (announce_tx, mut announce_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    set_announce_tx(announce_tx);

    // We can't easily split a NoiseTransport, so we use a single-threaded
    // loop with periodic ping checks via timeout
    let mut last_ping = std::time::Instant::now();

    loop {
        // Send ping every 15s to keep connection alive
        if last_ping.elapsed() > Duration::from_secs(15) {
            transport.write_message(MSG_PING_REQUEST, &[]).await?;
            last_ping = std::time::Instant::now();
        }

        // Drain pending announcements when pipeline is idle
        if !pipeline_active && !pending_announcements.is_empty() {
            let texts: Vec<String> = pending_announcements.drain(..).collect();
            for text in texts {
                info!("[satellite] playing queued announcement: {}", text);
                let tts_result = run_tts(&config.tts_host, &text).await;
                if let Ok((audio, rate, ch, bits)) = tts_result {
                    if !audio.is_empty() {
                        let url = cache_tts_audio(audio, 18789, rate, ch, bits).await;
                        let mut enc = super::esphome_api::ProtoEncoder::new();
                        enc.encode_string(1, &url);
                        enc.encode_string(2, &text);
                        let payload = enc.finish();
                        let _ = transport.write_message(MSG_VOICE_ASSISTANT_ANNOUNCE, &payload).await;
                    }
                }
            }
        }

        // Check for announcements (timer expiry) alongside satellite messages.
        // Use select! to handle both channels concurrently.
        enum Event {
            SatMsg(Option<RawMessage>),
            Announce(String),
            Timeout,
        }

        let event = tokio::select! {
            msg = transport.read_message() => Event::SatMsg(msg),
            text = announce_rx.recv() => match text {
                Some(t) => Event::Announce(t),
                None => Event::Timeout,
            },
            _ = tokio::time::sleep(Duration::from_secs(20)) => Event::Timeout,
        };

        let msg = match event {
            Event::SatMsg(Some(msg)) => msg,
            Event::SatMsg(None) => {
                info!("[satellite] connection closed");
                break;
            }
            Event::Announce(text) => {
                if pipeline_active {
                    // Queue for after pipeline finishes
                    info!("[satellite] queuing announcement (pipeline active): {}", text);
                    pending_announcements.push(text);
                } else {
                    info!("[satellite] announcement: {}", text);
                    let tts_result = run_tts(&config.tts_host, &text).await;
                    if let Ok((audio, rate, ch, bits)) = tts_result {
                        if !audio.is_empty() {
                            let url = cache_tts_audio(audio, 18789, rate, ch, bits).await;
                            let mut enc = super::esphome_api::ProtoEncoder::new();
                            enc.encode_string(1, &url);
                            enc.encode_string(2, &text);
                            let payload = enc.finish();
                            let _ = transport.write_message(MSG_VOICE_ASSISTANT_ANNOUNCE, &payload).await;
                            info!("[satellite] announce URL: {}", url);
                        }
                    }
                }
                continue;
            }
            Event::Timeout => {
                // Send a ping to keep connection alive
                if transport.write_message(MSG_PING_REQUEST, &[]).await.is_err() {
                    warn!("[satellite] ping failed, reconnecting");
                    break;
                }
                last_ping = std::time::Instant::now();
                continue;
            }
        };

        match msg.msg_type {
            MSG_PING_REQUEST => {
                let _ = transport.write_message(MSG_PING_RESPONSE, &[]).await;
            }
            MSG_PING_RESPONSE => {}
            MSG_DISCONNECT_REQUEST => {
                let _ = transport.write_message(MSG_DISCONNECT_RESPONSE, &[]).await;
                info!("[satellite] disconnect requested");
                break;
            }
            MSG_VOICE_ASSISTANT_REQUEST => {
                let req = parse_voice_assistant_request(&msg.payload);
                if req.start {
                    // Echo cooldown: ignore conversation restarts within 3s
                    // of a TTS response (the mic picks up Peter's own voice)
                    if req.wake_word_phrase.is_empty() {
                        if let Some(t) = last_tts_time {
                            let cooldown = Duration::from_secs(last_tts_duration_secs + 1);
                            if t.elapsed() < cooldown {
                                if followup_allowed {
                                    // Allow one follow-up — but we'll check the STT result
                                    // after recording. If it's just echo ("Yeah", "Mm"), we
                                    // discard and exit. This flag just lets audio recording start.
                                    info!("[satellite] allowing follow-up conversation");
                                    followup_allowed = false;
                                } else {
                                    // Second+ restart during cooldown — kill it
                                    info!("[satellite] ignoring conversation restart (echo cooldown)");
                                    let ev = build_voice_assistant_event(EVENT_ERROR, &[
                                        ("code", "idle"), ("message", "")
                                    ]);
                                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                    let ev = build_voice_assistant_event(EVENT_RUN_END, &[]);
                                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                    continue;
                                }
                            }
                        }
                    }
                    info!(
                        "[satellite] pipeline START (conv={}, wake={})",
                        req.conversation_id, req.wake_word_phrase
                    );
                    pipeline_active = true;
                    conversation_id = req.conversation_id;
                    audio_buf.clear();

                    let resp = build_voice_assistant_response(0, false);
                    transport.write_message(MSG_VOICE_ASSISTANT_RESPONSE, &resp).await?;

                    let ev = build_voice_assistant_event(EVENT_RUN_START, &[]);
                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                    let ev = build_voice_assistant_event(EVENT_STT_START, &[]);
                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                } else {
                    info!("[satellite] pipeline STOP");
                    // If we have accumulated audio, process it now
                    if !audio_buf.is_empty() && audio_buf.len() > 3200 {
                        info!("[satellite] processing {} bytes on stop ({:.1}s)",
                            audio_buf.len(), audio_buf.len() as f64 / 32000.0);

                        let result = run_voice_pipeline(config, &http, &audio_buf, &conversation_id).await;

                        match result {
                            Ok(pr) if !pr.transcript.is_empty() => {
                                let ev = build_voice_assistant_event(EVENT_STT_END, &[("text", &pr.transcript)]);
                                transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                let ev = build_voice_assistant_event(EVENT_INTENT_START, &[]);
                                transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                let ev = build_voice_assistant_event(EVENT_INTENT_END, &[
                                    ("conversation_id", &conversation_id),
                                    ("continue_conversation", "0"),
                                ]);
                                transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                if !pr.tts_audio.is_empty() {
                                    // Calculate audio duration for echo cooldown
                                    let bytes_per_sample = (pr.tts_width / 8) as u32 * pr.tts_channels as u32;
                                    let audio_secs = if pr.tts_sample_rate > 0 && bytes_per_sample > 0 {
                                        pr.tts_audio.len() as u64 / (pr.tts_sample_rate as u64 * bytes_per_sample as u64)
                                    } else { 3 };

                                    let tts_url = cache_tts_audio(pr.tts_audio, 18789, pr.tts_sample_rate, pr.tts_channels, pr.tts_width).await;
                                    let ev = build_voice_assistant_event(EVENT_TTS_START, &[("text", &pr.response_text)]);
                                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                    let ev = build_voice_assistant_event(EVENT_TTS_END, &[("url", &tts_url)]);
                                    transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                                    last_tts_time = Some(std::time::Instant::now());
                                    last_tts_duration_secs = audio_secs.max(2);
                                    followup_allowed = true;
                                    info!("[satellite] echo cooldown: {}s (TTS {}s)", last_tts_duration_secs + 1, audio_secs);
                                }
                            }
                            Ok(_) => info!("[satellite] empty transcript, skipping"),
                            Err(e) => {
                                error!("[satellite] pipeline error: {}", e);
                                let ev = build_voice_assistant_event(EVENT_ERROR, &[("code", "pipeline_error"), ("message", &e)]);
                                transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                            }
                        }
                        let ev = build_voice_assistant_event(EVENT_RUN_END, &[]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                        audio_buf.clear();
                    }
                    pipeline_active = false;
                }
            }
            MSG_VOICE_ASSISTANT_AUDIO if pipeline_active => {
                let audio = parse_voice_assistant_audio(&msg.payload);
                if !audio.data.is_empty() {
                    audio_buf.extend_from_slice(&audio.data);
                }

                // The satellite streams mic audio continuously (even silence).
                // We use energy-based VAD: track if we've heard speech (high
                // energy), then process when energy drops back to silence.
                if !audio.end {
                    let max_bytes = 32000 * 8; // 8 seconds max
                    let mut heard_speech = false;
                    let mut silence_chunks = 0;
                    let speech_threshold = 1500i64; // RMS threshold for speech (high to avoid echo)
                    let silence_needed = 12; // ~12 chunks of silence @ ~32ms each ≈ 384ms

                    // Check energy of current chunk
                    let rms = audio_rms(&audio.data);
                    if rms > speech_threshold { heard_speech = true; silence_chunks = 0; }
                    else if heard_speech { silence_chunks += 1; }

                    // Keep reading until we detect end of speech or hit max
                    while audio_buf.len() < max_bytes {
                        if heard_speech && silence_chunks >= silence_needed {
                            info!("[satellite] speech ended (VAD: {}ms silence)", silence_chunks * 32);
                            break;
                        }
                        // If we've been recording 4+ seconds with no speech, bail
                        if !heard_speech && audio_buf.len() > 32000 * 4 {
                            info!("[satellite] no speech detected in 4s, aborting");
                            audio_buf.clear();
                            let ev = build_voice_assistant_event(EVENT_ERROR, &[
                                ("code", "idle"), ("message", "")
                            ]);
                            transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                            let ev = build_voice_assistant_event(EVENT_RUN_END, &[]);
                            transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                            pipeline_active = false;
                            continue;
                        }
                        match timeout(Duration::from_millis(50), transport.read_message()).await {
                            Ok(Some(next)) if next.msg_type == MSG_VOICE_ASSISTANT_AUDIO => {
                                let a = parse_voice_assistant_audio(&next.payload);
                                if !a.data.is_empty() {
                                    let chunk_rms = audio_rms(&a.data);
                                    if chunk_rms > speech_threshold {
                                        heard_speech = true;
                                        silence_chunks = 0;
                                    } else if heard_speech {
                                        silence_chunks += 1;
                                    }
                                    audio_buf.extend_from_slice(&a.data);
                                }
                                if a.end { break; }
                            }
                            Ok(Some(next)) if next.msg_type == MSG_VOICE_ASSISTANT_REQUEST => {
                                let req = parse_voice_assistant_request(&next.payload);
                                if !req.start {
                                    info!("[satellite] pipeline STOP during recording");
                                    break;
                                }
                            }
                            Ok(Some(next)) if next.msg_type == MSG_PING_REQUEST => {
                                let _ = transport.write_message(MSG_PING_RESPONSE, &[]).await;
                            }
                            Ok(Some(_)) => {} // ignore other messages
                            Ok(None) => break,
                            Err(_) => {} // timeout, keep looping
                        }
                    }
                }

                if audio_buf.len() < 3200 || !pipeline_active {
                    audio_buf.clear();
                    continue;
                }

                info!("[satellite] processing audio: {} bytes ({:.1}s)",
                    audio_buf.len(), audio_buf.len() as f64 / 32000.0);

                let result = run_voice_pipeline(config, &http, &audio_buf, &conversation_id).await;

                match result {
                    Ok(pr) if pr.response_text.is_empty() => {
                        // Echo artifact filtered — silently end the pipeline
                        info!("[satellite] echo filtered, ending silently");
                        let ev = build_voice_assistant_event(EVENT_ERROR, &[
                            ("code", "idle"), ("message", "")
                        ]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                        let ev = build_voice_assistant_event(EVENT_RUN_END, &[]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                        audio_buf.clear();
                        pipeline_active = false;
                        continue;
                    }
                    Ok(pr) => {
                        // STT_END with transcript
                        let ev = build_voice_assistant_event(EVENT_STT_END, &[("text", &pr.transcript)]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;

                        // INTENT_START
                        let ev = build_voice_assistant_event(EVENT_INTENT_START, &[]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;

                        // INTENT_END with conversation_id
                        let ev = build_voice_assistant_event(EVENT_INTENT_END, &[
                            ("conversation_id", &conversation_id),
                            ("continue_conversation", "0"),
                        ]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;

                        // TTS — cache audio and send URL for satellite's media player
                        if !pr.tts_audio.is_empty() {
                            let bytes_per_sample = (pr.tts_width / 8) as u32 * pr.tts_channels as u32;
                            let audio_secs = if pr.tts_sample_rate > 0 && bytes_per_sample > 0 {
                                pr.tts_audio.len() as u64 / (pr.tts_sample_rate as u64 * bytes_per_sample as u64)
                            } else { 3 };

                            let tts_url = cache_tts_audio(pr.tts_audio, 18789, pr.tts_sample_rate, pr.tts_channels, pr.tts_width).await;
                            info!("[satellite] TTS URL: {}", tts_url);

                            let ev = build_voice_assistant_event(EVENT_TTS_START, &[("text", &pr.response_text)]);
                            transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                            let ev = build_voice_assistant_event(EVENT_TTS_END, &[("url", &tts_url)]);
                            transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                            last_tts_time = Some(std::time::Instant::now());
                            last_tts_duration_secs = audio_secs.max(2);
                            followup_allowed = true;
                            info!("[satellite] echo cooldown: {}s (TTS {}s)", last_tts_duration_secs + 1, audio_secs);
                        }
                    }
                    Err(e) => {
                        error!("[satellite] pipeline error: {}", e);
                        let ev = build_voice_assistant_event(EVENT_ERROR, &[("code", "pipeline_error"), ("message", &e)]);
                        transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                    }
                }
                // RUN_END
                let ev = build_voice_assistant_event(EVENT_RUN_END, &[]);
                transport.write_message(MSG_VOICE_ASSISTANT_EVENT, &ev).await?;
                audio_buf.clear();
                pipeline_active = false;
            }
            MSG_LIST_ENTITIES_REQUEST => {
                let _ = transport.write_message(MSG_LIST_ENTITIES_DONE, &[]).await;
            }
            MSG_VOICE_ASSISTANT_CONFIG_REQUEST => {
                let _ = transport.write_message(MSG_VOICE_ASSISTANT_CONFIG_RESPONSE, &[]).await;
            }
            MSG_VOICE_ASSISTANT_ANNOUNCE_FINISHED => {
                info!("[satellite] announcement playback finished");
            }
            _ => {
                debug!("[satellite] unhandled message type {}", msg.msg_type);
            }
        }
    }

    Ok(())
}

/// Detect if a transcript is likely echo from TTS playback, not a real user command.
/// Short single-word filler responses picked up by the mic during/after TTS.
fn is_echo_artifact(transcript: &str) -> bool {
    let t = transcript.trim().trim_matches('.').trim_matches(',').to_lowercase();
    let word_count = t.split_whitespace().count();

    // Single word filler — almost always echo
    if word_count <= 1 {
        let echo_words = [
            "yeah", "yes", "yep", "yup", "no", "nah", "nope",
            "ok", "okay", "sure", "right", "mm", "hmm", "uh",
            "huh", "oh", "ah", "so", "well", "hey", "hi",
            "thanks", "cool", "nice", "good", "great", "alright",
        ];
        return echo_words.iter().any(|&w| t == w);
    }

    // Two-word filler
    if word_count == 2 {
        let echo_phrases = [
            "yeah yeah", "ok ok", "all right", "oh yeah", "oh ok",
            "sounds good", "got it", "thank you", "you good",
            "for sure", "no problem", "of course",
        ];
        return echo_phrases.iter().any(|&p| t == p);
    }

    false
}

/// Calculate RMS (root mean square) energy of 16-bit PCM audio.
fn audio_rms(pcm: &[u8]) -> i64 {
    if pcm.len() < 2 { return 0; }
    let mut sum: i64 = 0;
    let mut count: i64 = 0;
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as i64;
        sum += sample * sample;
        count += 1;
    }
    if count == 0 { return 0; }
    ((sum / count) as f64).sqrt() as i64
}

struct PipelineResult {
    transcript: String,
    response_text: String,
    tts_audio: Vec<u8>,
    tts_sample_rate: u32,
    tts_channels: u16,
    tts_width: u16,
}

async fn run_voice_pipeline(
    config: &SatelliteConfig,
    http: &reqwest::Client,
    audio_pcm: &[u8],
    _conversation_id: &str,
) -> Result<PipelineResult, String> {
    // 1. STT — send audio to rust-voice-pipeline Wyoming server
    let transcript = run_stt(&config.stt_host, audio_pcm).await?;
    if transcript.is_empty() {
        return Ok(PipelineResult {
            transcript: String::new(),
            response_text: String::new(),
            tts_audio: Vec::new(),
            tts_sample_rate: 0,
            tts_channels: 0,
            tts_width: 0,
        });
    }
    info!("[pipeline] STT: \"{}\"", transcript);

    // Filter echo artifacts — short single-word responses from TTS playback
    // being picked up by the microphone. These are NOT real user commands.
    if is_echo_artifact(&transcript) {
        info!("[pipeline] filtered echo artifact: \"{}\"", transcript);
        return Ok(PipelineResult {
            transcript,
            response_text: String::new(),
            tts_audio: Vec::new(),
            tts_sample_rate: 0,
            tts_channels: 0,
            tts_width: 0,
        });
    }

    // 2. LLM — call syntaur voice_chat
    let response_text = call_gateway(http, &config.gateway_url, &config.gateway_secret, &transcript).await?;
    info!("[pipeline] LLM: \"{}\"", response_text.chars().take(80).collect::<String>());

    // 3. TTS — get audio from Fish Audio Wyoming server
    let (tts_audio, tts_rate, tts_ch, tts_w) = run_tts(&config.tts_host, &response_text).await.unwrap_or_default();
    info!("[pipeline] TTS: {} bytes audio ({}Hz {}ch {}bit)", tts_audio.len(), tts_rate, tts_ch, tts_w);

    Ok(PipelineResult {
        transcript,
        response_text,
        tts_audio,
        tts_sample_rate: tts_rate,
        tts_channels: tts_ch,
        tts_width: tts_w,
    })
}

/// Send audio to the Wyoming STT server and get transcript.
async fn run_stt(stt_host: &str, audio_pcm: &[u8]) -> Result<String, String> {
    use tokio::io::BufReader;

    let stream = TcpStream::connect(stt_host)
        .await
        .map_err(|e| format!("stt connect: {}", e))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Send audio-start
    let start_header = serde_json::json!({
        "type": "audio-start",
        "version": "1.0.0",
        "data_length": 0,
        "payload_length": 0,
    });
    write_half
        .write_all(format!("{}\n", start_header).as_bytes())
        .await
        .map_err(|e| format!("stt write: {}", e))?;

    // Send audio-chunk with PCM data
    let chunk_header = serde_json::json!({
        "type": "audio-chunk",
        "version": "1.0.0",
        "data_length": 0,
        "payload_length": audio_pcm.len(),
    });
    write_half
        .write_all(format!("{}\n", chunk_header).as_bytes())
        .await
        .map_err(|e| format!("stt write: {}", e))?;
    write_half
        .write_all(audio_pcm)
        .await
        .map_err(|e| format!("stt write audio: {}", e))?;

    // Send audio-stop
    let stop_header = serde_json::json!({
        "type": "audio-stop",
        "version": "1.0.0",
        "data_length": 0,
        "payload_length": 0,
    });
    write_half
        .write_all(format!("{}\n", stop_header).as_bytes())
        .await
        .map_err(|e| format!("stt write: {}", e))?;
    write_half.flush().await.map_err(|e| format!("stt flush: {}", e))?;

    // Read transcript response
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
        .await
        .map_err(|e| format!("stt read: {}", e))?;

    let header: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("stt parse header: {}", e))?;

    let data_len = header
        .get("data_length")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let data = if data_len > 0 {
        let mut buf = vec![0u8; data_len];
        tokio::io::AsyncReadExt::read_exact(&mut reader, &mut buf)
            .await
            .map_err(|e| format!("stt read data: {}", e))?;
        serde_json::from_slice::<serde_json::Value>(&buf).ok()
    } else {
        None
    };

    let text = data
        .and_then(|d| d.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();

    Ok(text)
}

/// Call syntaur voice_chat and get the response text.
async fn call_gateway(
    http: &reqwest::Client,
    gateway_url: &str,
    gateway_secret: &str,
    transcript: &str,
) -> Result<String, String> {
    let url = format!("{}/v1/chat/completions", gateway_url);
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": transcript}],
    });

    let mut req = http.post(&url).json(&body);
    if !gateway_secret.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", gateway_secret));
    }

    let resp = req.send().await.map_err(|e| format!("syntaur: {}", e))?;
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
        .ok_or_else(|| "empty syntaur response".to_string())
}

/// Send text to the Wyoming TTS server and get audio back.
/// Returns (audio_pcm, sample_rate, channels, bits_per_sample).
async fn run_tts(tts_host: &str, text: &str) -> Result<(Vec<u8>, u32, u16, u16), String> {
    let stream = TcpStream::connect(tts_host)
        .await
        .map_err(|e| format!("tts connect: {}", e))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Send synthesize request
    let data = serde_json::json!({"text": text});
    let data_bytes = serde_json::to_vec(&data).unwrap_or_default();
    let header = serde_json::json!({
        "type": "synthesize",
        "version": "1.0.0",
        "data_length": data_bytes.len(),
        "payload_length": 0,
    });
    write_half
        .write_all(format!("{}\n", header).as_bytes())
        .await
        .map_err(|e| format!("tts write: {}", e))?;
    write_half
        .write_all(&data_bytes)
        .await
        .map_err(|e| format!("tts write data: {}", e))?;
    write_half.flush().await.map_err(|e| format!("tts flush: {}", e))?;

    // Read audio chunks until audio-stop
    let mut audio_buf = Vec::new();
    let mut sample_rate = 22050u32;
    let mut channels = 1u16;
    let mut width = 2u16;

    loop {
        let mut line = String::new();
        match tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let header: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(h) => h,
            Err(_) => break,
        };

        let msg_type = header.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let data_length = header.get("data_length").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let payload_length = header.get("payload_length").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        // Read data bytes (format info for audio-start)
        if data_length > 0 {
            let mut data_buf = vec![0u8; data_length];
            let _ = tokio::io::AsyncReadExt::read_exact(&mut reader, &mut data_buf).await;
            if msg_type == "audio-start" {
                if let Ok(d) = serde_json::from_slice::<serde_json::Value>(&data_buf) {
                    sample_rate = d.get("rate").and_then(|v| v.as_u64()).unwrap_or(22050) as u32;
                    channels = d.get("channels").and_then(|v| v.as_u64()).unwrap_or(1) as u16;
                    width = d.get("width").and_then(|v| v.as_u64()).unwrap_or(2) as u16;
                    info!("[tts] audio format: {}Hz {}ch {}byte", sample_rate, channels, width);
                }
            }
        }

        // Read payload (audio)
        if payload_length > 0 {
            let mut buf = vec![0u8; payload_length];
            let _ = tokio::io::AsyncReadExt::read_exact(&mut reader, &mut buf).await;
            if msg_type == "audio-chunk" {
                audio_buf.extend_from_slice(&buf);
            }
        }

        if msg_type == "audio-stop" {
            break;
        }
    }

    Ok((audio_buf, sample_rate, channels, width * 8))
}
