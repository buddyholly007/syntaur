//! macOS audio backends.
//!
//! ## direct
//! macOS has no supported "per-application volume" API in the public
//! CoreAudio headers. The closest thing is using the AudioUnit graph
//! with a tap — which requires private APIs or user-installed helpers
//! (e.g. BackgroundMusic, BlackHole-based routing). In direct mode we
//! fall through to provider.set_volume (MusicKit.volume on the AM web
//! player, `<audio>.volume` elsewhere), which is sufficient for the
//! ducking UX.
//!
//! ## capture (opt-in)
//! Requires BlackHole installed. We set BlackHole as the default system
//! output, capture from it via cpal, and re-play through the user's
//! *preferred* output (stored in bridge config). This lets us apply real
//! attenuation at the pipeline level and is a future work item — the
//! stub here validates the backend name and returns gracefully.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::PipelineImpl;
use crate::state::BridgeState;

pub struct DirectPipeline {
    volume: RwLock<f32>,
}

impl DirectPipeline {
    pub async fn start(_state: Arc<BridgeState>) -> Result<Self> {
        Ok(Self {
            volume: RwLock::new(1.0),
        })
    }
}

#[async_trait::async_trait]
impl PipelineImpl for DirectPipeline {
    fn backend_name(&self) -> &str {
        "macos-direct"
    }

    async fn set_volume(&self, level: f32) {
        *self.volume.write().await = level;
        // On macOS provider.set_volume is the real ducking path.
        // Nothing to do here at the pipeline layer.
    }
}

pub struct CapturePipeline {
    volume: RwLock<f32>,
}

impl CapturePipeline {
    pub async fn start(_state: Arc<BridgeState>) -> Result<Self> {
        log::warn!(
            "macOS capture backend is a stub. Requires BlackHole; full capture/playback \
             implementation pending."
        );
        Ok(Self {
            volume: RwLock::new(1.0),
        })
    }
}

#[async_trait::async_trait]
impl PipelineImpl for CapturePipeline {
    fn backend_name(&self) -> &str {
        "macos-screencapturekit"
    }

    async fn set_volume(&self, level: f32) {
        *self.volume.write().await = level;
    }
}
