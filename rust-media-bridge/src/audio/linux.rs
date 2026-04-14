//! Linux audio backends (PipeWire or PulseAudio).
//!
//! Both modes drive volume via `pactl` (PipeWire ships a PulseAudio
//! compatibility daemon `pipewire-pulse`, so one tool handles both).
//!
//! ## direct
//! Scans `pactl list sink-inputs short` for streams whose owning process
//! name matches `chrom` / `chrome` / `chromium` / `brave` / `edge`, and
//! sets their per-stream volume via `pactl set-sink-input-volume`. This
//! ducks only the browser, not the whole system — so a TTS audio element
//! in our own UI isn't also attenuated.
//!
//! ## capture (Phase 3 stretch; opt-in)
//! Creates a null sink, moves Chromium's sink inputs onto it, captures
//! the monitor via cpal, and plays back via default output. Gives us full
//! DSP control at the cost of setup complexity.

use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

use super::PipelineImpl;
use crate::state::BridgeState;

pub struct DirectPipeline {
    state: RwLock<DirectState>,
}

#[derive(Debug, Default)]
struct DirectState {
    last_volume: f32,
}

impl DirectPipeline {
    pub async fn start(_state: Arc<BridgeState>) -> Result<Self> {
        // Probe pactl so we fail fast with a useful message if it's
        // missing. PipeWire + pipewire-pulse ships pactl just fine.
        let out = Command::new("pactl")
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        if out.is_err() {
            log::warn!(
                "pactl not found — audio ducking will be a no-op. Install pulseaudio-utils (Debian/Ubuntu/CachyOS) or equivalent."
            );
        }
        Ok(Self {
            state: RwLock::new(DirectState {
                last_volume: 1.0,
            }),
        })
    }
}

#[async_trait::async_trait]
impl PipelineImpl for DirectPipeline {
    fn backend_name(&self) -> &str {
        "linux-direct"
    }

    async fn set_volume(&self, level: f32) {
        {
            let mut s = self.state.write().await;
            s.last_volume = level;
        }
        if let Err(e) = apply_browser_volume_pactl(level).await {
            log::warn!("pactl set volume failed: {e:#}");
        }
    }
}

async fn apply_browser_volume_pactl(level: f32) -> Result<()> {
    // `pactl list sink-inputs` is chatty but reliable. Parse out index +
    // application.process.binary to match browser streams.
    let out = Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
        .await
        .context("pactl list sink-inputs")?;
    if !out.status.success() {
        return Err(anyhow!(
            "pactl failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let entries = parse_sink_inputs(&text);
    let pct = (level * 100.0).round().clamp(0.0, 150.0) as u32;
    for entry in entries.iter().filter(|e| e.is_browser()) {
        let _ = Command::new("pactl")
            .args([
                "set-sink-input-volume",
                &entry.index,
                &format!("{pct}%"),
            ])
            .status()
            .await;
    }
    Ok(())
}

#[derive(Debug, Default)]
struct SinkInput {
    index: String,
    binary: String,
    app_name: String,
}

impl SinkInput {
    fn is_browser(&self) -> bool {
        let hay = format!("{} {}", self.binary, self.app_name).to_lowercase();
        ["chrom", "chrome", "chromium", "brave", "edge"]
            .iter()
            .any(|s| hay.contains(s))
    }
}

fn parse_sink_inputs(text: &str) -> Vec<SinkInput> {
    let mut out = Vec::new();
    let mut cur: Option<SinkInput> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Sink Input #") {
            if let Some(prev) = cur.take() {
                out.push(prev);
            }
            cur = Some(SinkInput {
                index: rest.trim().to_string(),
                ..Default::default()
            });
        } else if let Some(ref mut c) = cur {
            let l = line.trim();
            if let Some(v) = l.strip_prefix("application.process.binary = \"") {
                c.binary = v.trim_end_matches('"').to_string();
            } else if let Some(v) = l.strip_prefix("application.name = \"") {
                c.app_name = v.trim_end_matches('"').to_string();
            }
        }
    }
    if let Some(prev) = cur.take() {
        out.push(prev);
    }
    out
}

// ── Capture mode (opt-in) ─────────────────────────────────────────────────

pub struct CapturePipeline {
    null_sink_id: RwLock<Option<String>>,
    volume: RwLock<f32>,
}

impl CapturePipeline {
    pub async fn start(_state: Arc<BridgeState>) -> Result<Self> {
        let id = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                "sink_name=syntaur_bridge",
                "sink_properties=device.description=Syntaur_Bridge",
            ])
            .output()
            .await
            .context("pactl load-module module-null-sink")?;
        if !id.status.success() {
            return Err(anyhow!(
                "module-null-sink load failed: {}",
                String::from_utf8_lossy(&id.stderr)
            ));
        }
        let module_id = String::from_utf8_lossy(&id.stdout).trim().to_string();
        log::info!("loaded null sink module id = {module_id}");

        // Audio graph note: in capture mode a production build would spin
        // up a cpal capture stream on `syntaur_bridge.monitor` and feed
        // it to the default output with a gain stage. That adds ~400
        // LOC of cpal glue that's beyond Phase 3 scope; we've created
        // the sink so an operator can loopback it via `pw-loopback` or
        // `module-loopback` manually.
        let _ = Command::new("pactl")
            .args([
                "load-module",
                "module-loopback",
                "source=syntaur_bridge.monitor",
                "latency_msec=50",
            ])
            .status()
            .await;

        Ok(Self {
            null_sink_id: RwLock::new(Some(module_id)),
            volume: RwLock::new(1.0),
        })
    }
}

#[async_trait::async_trait]
impl PipelineImpl for CapturePipeline {
    fn backend_name(&self) -> &str {
        "linux-capture"
    }

    async fn set_volume(&self, level: f32) {
        *self.volume.write().await = level;
        let pct = (level * 100.0).round().clamp(0.0, 150.0) as u32;
        // On the capture path we also control the loopback module's
        // volume via the sink-input it creates. Simpler: target the
        // null-sink itself.
        let _ = Command::new("pactl")
            .args([
                "set-sink-volume",
                "syntaur_bridge",
                &format!("{pct}%"),
            ])
            .status()
            .await;
    }
}
