//! Audio pipeline — how we get Chromium's decrypted audio out to the user
//! and how we attenuate it for ducking.
//!
//! Two operating modes:
//!
//!   1. "direct"  — Chromium plays to system default output. Ducking is
//!                  implemented via per-application volume controls
//!                  (pactl on Linux, WASAPI per-session on Windows,
//!                  provider.set_volume fallback on macOS). Default mode;
//!                  requires no virtual-device setup.
//!
//!   2. "capture" — Chromium plays to a dedicated virtual sink; we
//!                  capture the sink's monitor via cpal and re-play
//!                  through default output with our own volume factor.
//!                  Required if the user wants DSP (EQ, crossfade) or
//!                  tight audio isolation. More setup cost.
//!
//! Phase 3 ships direct mode on all three platforms. Capture mode is
//! Linux-first and opt-in.

use anyhow::Result;
use std::sync::Arc;

use crate::state::BridgeState;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Cheap clone handle to the running audio pipeline.
#[derive(Clone)]
pub struct AudioPipeline {
    inner: Arc<dyn PipelineImpl + Send + Sync>,
}

#[async_trait::async_trait]
trait PipelineImpl: Send + Sync {
    async fn set_volume(&self, level: f32);
    fn backend_name(&self) -> &str;
}

impl AudioPipeline {
    pub async fn start(
        backend: Option<&str>,
        state: Arc<BridgeState>,
    ) -> Result<Self> {
        let chosen = backend.unwrap_or(default_backend_name());
        let inner: Arc<dyn PipelineImpl + Send + Sync> = match chosen {
            #[cfg(target_os = "linux")]
            "linux-direct" | "linux-pipewire" | "linux-pulse" => {
                Arc::new(linux::DirectPipeline::start(state).await?)
            }
            #[cfg(target_os = "linux")]
            "linux-capture" => Arc::new(linux::CapturePipeline::start(state).await?),
            #[cfg(target_os = "windows")]
            "windows-direct" | "windows-wasapi" => {
                Arc::new(windows::DirectPipeline::start(state).await?)
            }
            #[cfg(target_os = "macos")]
            "macos-direct" => Arc::new(macos::DirectPipeline::start(state).await?),
            #[cfg(target_os = "macos")]
            "macos-screencapturekit" | "macos-capture" => {
                Arc::new(macos::CapturePipeline::start(state).await?)
            }
            other => {
                return Err(anyhow::anyhow!(
                    "unsupported audio backend for this platform: {other}"
                ))
            }
        };
        log::info!("audio pipeline up: {}", inner.backend_name());
        Ok(Self { inner })
    }

    pub async fn set_volume(&self, level: f32) {
        self.inner.set_volume(level.clamp(0.0, 1.0)).await
    }

    pub fn backend_name(&self) -> &str {
        self.inner.backend_name()
    }
}

pub fn default_backend_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux-direct"
    }
    #[cfg(target_os = "windows")]
    {
        "windows-direct"
    }
    #[cfg(target_os = "macos")]
    {
        "macos-direct"
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        "unsupported"
    }
}
