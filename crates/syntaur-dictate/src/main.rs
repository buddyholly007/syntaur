//! syntaur-dictate — streaming dictation client backed by NVIDIA
//! Parakeet-unified-en-0.6b (ONNX, via parakeet-rs).
//!
//! Architecture
//! ============
//!
//! ```text
//!   cpal::Device (Blue Yeti, 16kHz mono)
//!         │  f32 chunks
//!         ▼
//!   accumulator → fixed-size CHUNK_SAMPLES windows
//!         │
//!         ▼
//!   ParakeetUnified::transcribe_chunk
//!         │  emits ONLY the delta string for this window
//!         ▼
//!   live? → ydotool type   |   muted? → drop
//! ```
//!
//! State machine
//! =============
//!
//! - **muted** (default at boot): mic captured, recognizer ingesting
//!   so the encoder cache stays warm, but emitted text is discarded.
//!   SIGUSR1 → live.
//! - **live**: every transcribe_chunk delta types via ydotool's CLI
//!   (which talks to ydotoold via UNIX socket).
//!   SIGUSR1 → muted.
//! - SIGTERM → graceful shutdown.
//!
//! Replaces the Vosk-based `syntaur-dictate-bg.service` (2.7 GB
//! resident → ~600 MB; ~9% WER → ~6%; lowercase no-PnC → punctuated +
//! capitalised).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel as cc;
use log::{error, info, warn};
use parakeet_rs::Nemotron;

#[derive(Parser, Debug)]
#[command(name = "syntaur-dictate", about = "Streaming dictation via Parakeet-unified")]
struct Args {
    /// Directory holding encoder.onnx + encoder.onnx.data + decoder_joint.onnx
    /// + tokenizer.model. parakeet-rs accepts the int8 .onnx variants under
    /// the same names if encoder.onnx + encoder.onnx.data are missing — but
    /// the rename pattern is "drop the .int8" suffix the bobNight bundle
    /// ships with. Easiest: symlink encoder.int8.onnx → encoder.onnx etc.
    #[arg(long, default_value = "/home/sean/.local/share/nemotron-streaming-onnx")]
    model_dir: PathBuf,

    /// Substring matched against host input devices. The first matching
    /// device is selected. "" → system default.
    #[arg(long, default_value = "Blue")]
    mic_match: String,

    /// Echo every emitted delta to stdout in addition to typing it.
    #[arg(long)]
    verbose_text: bool,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    info!("syntaur-dictate starting (model_dir={})", args.model_dir.display());

    let live = Arc::new(AtomicBool::new(false));
    install_signal_handlers(live.clone());

    let (audio_tx, audio_rx) = cc::bounded::<Vec<f32>>(256);
    let _stream = open_mic(&args.mic_match, audio_tx)?;
    info!("mic capture armed (default = muted; send SIGUSR1 to toggle)");

    // Custom ExecutionConfig: disable graph optimization. The bobNight
    // ONNX export (and apparently eschmidbauer's too) carries fused
    // QuickGelu ops that ort 2.0.0-rc.12 fails on with
    // "GetElementType is not implemented" when its own optimizer
    // tries to re-fuse them. Telling ort to leave the graph alone
    // bypasses the bug.
    // Nemotron is parakeet-rs's primary/proven streaming model.
    // 8960 samples = 560 ms chunks per the upstream example.
    // (Switched here from parakeet-unified-en-0.6b after the bobNight
    // and eschmidbauer ONNX conversions both failed ort 2.0.0-rc.12
    // with "GetElementType is not implemented" — the conversions
    // emit fused ops the current ort can't introspect. The watcher
    // at ~/.local/bin/check-parakeet-unified-onnx will notify when
    // a working unified ONNX lands; swap to it then. Trade-off
    // accepted: Nemotron lacks built-in punctuation/capitalisation.)
    let mut model = Nemotron::from_pretrained(&args.model_dir, None)
        .map_err(|e| anyhow!("load nemotron-streaming: {e:?}"))?;
    let chunk_samples: usize = 8960; // 560 ms @ 16 kHz
    info!(
        "nemotron-streaming ready (chunk = {} samples / 560 ms)",
        chunk_samples
    );

    // Buffer cpal frames until we have ≥1 chunk of samples, then feed
    // chunk-by-chunk through the recognizer. Anything left in the
    // buffer waits for the next batch.
    let mut buf: Vec<f32> = Vec::with_capacity(chunk_samples * 4);
    loop {
        // Block briefly so we don't spin while the user is silent.
        match audio_rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(c) => buf.extend(c),
            Err(cc::RecvTimeoutError::Timeout) => continue,
            Err(cc::RecvTimeoutError::Disconnected) => {
                error!("audio channel closed; exiting");
                break;
            }
        }
        while buf.len() >= chunk_samples {
            let chunk: Vec<f32> = buf.drain(..chunk_samples).collect();
            // Skip ASR entirely while muted. The earlier design ran
            // transcribe_chunk every 560 ms even when muted to "keep the
            // encoder cache warm", which sat at ~120% CPU for days at a
            // time. Chunks are independent enough that a fresh stream
            // recovers full accuracy within 1-2 chunks (~1.1 s) of the
            // first live audio after toggle-live, which is an acceptable
            // trade for not pinning a core forever.
            if !live.load(Ordering::Relaxed) {
                continue;
            }
            // Nemotron::transcribe_chunk returns the new token string
            // for this 560 ms window (already a delta).
            let delta = match model.transcribe_chunk(&chunk) {
                Ok(s) => s,
                Err(e) => {
                    warn!("transcribe_chunk: {e:?}");
                    continue;
                }
            };
            if delta.is_empty() {
                continue;
            }
            if args.verbose_text {
                println!("{delta}");
            }
            if let Err(e) = type_via_ydotool(&delta) {
                warn!("type failed: {e}");
            }
        }
    }
    Ok(())
}

fn install_signal_handlers(live: Arc<AtomicBool>) {
    use signal_hook::consts::{SIGINT, SIGTERM, SIGUSR1};
    std::thread::spawn(move || {
        let mut signals =
            signal_hook::iterator::Signals::new([SIGUSR1, SIGINT, SIGTERM]).expect("signals");
        for sig in &mut signals {
            match sig {
                SIGUSR1 => {
                    let new = !live.load(Ordering::Relaxed);
                    live.store(new, Ordering::Relaxed);
                    info!("toggle → {}", if new { "LIVE" } else { "muted" });
                    let icon = if new { "🎤" } else { "🔇" };
                    let label = if new { "live — type as you speak" } else { "muted" };
                    let _ = std::process::Command::new("notify-send")
                        .args(["-t", "1000", "Dictation", &format!("{icon} {label}")])
                        .status();
                }
                SIGINT | SIGTERM => {
                    info!("shutdown signal");
                    std::process::exit(0);
                }
                _ => {}
            }
        }
    });
}

// ── Audio capture (cpal) ────────────────────────────────────────────

fn open_mic(mic_match: &str, tx: cc::Sender<Vec<f32>>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = if mic_match.is_empty() {
        host.default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?
    } else {
        host.input_devices()?
            .find(|d| d.name().map(|n| n.contains(mic_match)).unwrap_or(false))
            .ok_or_else(|| anyhow!("no input device matching {mic_match:?}"))?
    };
    info!("mic device = {}", device.name()?);

    let supported = device.default_input_config()?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let mut cfg: cpal::StreamConfig = supported.into();
    cfg.buffer_size = cpal::BufferSize::Default;

    const TARGET: u32 = 16000;
    let stream = device.build_input_stream(
        &cfg,
        move |data: &[f32], _| {
            // Downmix → mono.
            let mono = if channels == 1 {
                data.to_vec()
            } else {
                let n = channels as usize;
                data.chunks_exact(n)
                    .map(|f| f.iter().sum::<f32>() / n as f32)
                    .collect()
            };
            // Resample → 16 kHz. Linear interpolation; cheap, adequate
            // for speech ASR. Pro audio resamplers (rubato/libsamplerate)
            // exist but the WER delta is sub-perceptual for dictation.
            let resampled: Vec<f32> = if sample_rate == TARGET {
                mono
            } else {
                let ratio = TARGET as f32 / sample_rate as f32;
                let out_len = (mono.len() as f32 * ratio) as usize;
                (0..out_len)
                    .map(|i| {
                        let pos = i as f32 / ratio;
                        let i0 = pos as usize;
                        let frac = pos - i0 as f32;
                        let s0 = *mono.get(i0).unwrap_or(&0.0);
                        let s1 = *mono.get(i0 + 1).unwrap_or(&s0);
                        s0 + (s1 - s0) * frac
                    })
                    .collect()
            };
            let _ = tx.try_send(resampled);
        },
        |e| error!("cpal error: {e}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

// ── ydotool typing ──────────────────────────────────────────────────

fn type_via_ydotool(text: &str) -> Result<()> {
    // Spawning the CLI per delta is ~1 ms overhead; well below the
    // chunk cadence (160 ms+). Keeping the CLI invocation means we
    // don't have to vendor ydotoold's wire format.
    let status = std::process::Command::new("ydotool")
        .env("YDOTOOL_SOCKET", "/run/ydotoold/socket")
        .args(["type", "--key-delay", "0", "--", text])
        .status()
        .context("spawn ydotool")?;
    if !status.success() {
        return Err(anyhow!("ydotool exit {status}"));
    }
    Ok(())
}
