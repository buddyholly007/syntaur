//! Windows audio backend — per-session WASAPI volume control.
//!
//! Windows exposes per-application audio sessions through
//! `IAudioSessionManager2` and `ISimpleAudioVolume`. We can walk all
//! sessions on the default render endpoint, match ones owned by a
//! browser PID, and set their SetMasterVolume independently of other
//! audio — meaning our TTS on the same machine isn't attenuated while
//! the browser is.
//!
//! Phase 3 ships a minimal implementation that shells out to
//! `powershell`'s `Set-AppAudioVolume`-style helpers when available
//! and otherwise degrades to provider.set_volume (MusicKit.volume etc).
//! A native WASAPI implementation can replace this later.

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
        "windows-direct"
    }

    async fn set_volume(&self, level: f32) {
        *self.volume.write().await = level;

        // Best-effort via PowerShell. AudioDeviceCmdlets is a common
        // third-party module; native audio sessions need COM which we'll
        // wire in a follow-up with windows-rs. Without either installed,
        // this falls through silently and provider.set_volume (called
        // alongside) attenuates inside the player itself.
        let script = format!(
            r#"
            try {{
              $vol = {vol}
              if (Get-Module -ListAvailable -Name AudioDeviceCmdlets) {{
                Get-AudioSession | Where-Object {{ $_.Process -match 'chrome|chromium|msedge|brave' }} | ForEach-Object {{
                  $_ | Set-AudioSessionVolume -Volume $vol
                }}
              }}
            }} catch {{ }}
            "#,
            vol = level
        );
        let _ = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .await;
    }
}
