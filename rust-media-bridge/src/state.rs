//! Shared runtime state for the bridge. One BridgeState per process,
//! passed as Arc<BridgeState> into every HTTP/WS handler.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::audio::AudioPipeline;
use crate::browser::BrowserSession;
use crate::events::{BridgeEvent, NowPlaying};

pub struct BridgeState {
    pub data_dir: PathBuf,
    pub chromium_path: PathBuf,
    pub no_audio: bool,
    pub audio_backend: Option<String>,

    /// Single browser session. Lazy-initialized on first play command so
    /// the bridge boots fast and doesn't spawn Chromium until needed.
    pub browser: Mutex<Option<BrowserSession>>,

    /// Audio pipeline — virtual device + cpal capture/playback. Lazy-init
    /// same as browser. None if no_audio or unsupported platform.
    pub audio: Mutex<Option<AudioPipeline>>,

    /// Broadcast channel for WebSocket subscribers.
    pub events_tx: broadcast::Sender<BridgeEvent>,

    /// Current now-playing snapshot so /status reflects real state.
    pub now_playing: RwLock<Option<NowPlaying>>,

    /// Ducking state. When true, audio pipeline attenuates output by
    /// ducking_level (default 0.2).
    pub ducking: RwLock<DuckingState>,
}

#[derive(Debug, Clone)]
pub struct DuckingState {
    pub active: bool,
    pub level: f32,
    pub user_volume: f32,
}

impl Default for DuckingState {
    fn default() -> Self {
        Self {
            active: false,
            level: 0.2,
            user_volume: 1.0,
        }
    }
}

impl BridgeState {
    pub fn new(
        data_dir: PathBuf,
        chromium_path: PathBuf,
        no_audio: bool,
        audio_backend: Option<String>,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(256);
        Self {
            data_dir,
            chromium_path,
            no_audio,
            audio_backend,
            browser: Mutex::new(None),
            audio: Mutex::new(None),
            events_tx,
            now_playing: RwLock::new(None),
            ducking: RwLock::new(DuckingState::default()),
        }
    }

    /// Broadcast an event to all WebSocket subscribers. Non-blocking; if
    /// no subscribers, the event is dropped (harmless).
    pub fn emit(&self, ev: BridgeEvent) {
        let _ = self.events_tx.send(ev);
    }

    /// Ensure the browser session is alive. Returns a clone of the handle
    /// (the browser itself lives behind an Arc internally).
    pub async fn ensure_browser(self: &Arc<Self>) -> anyhow::Result<BrowserSession> {
        let mut guard = self.browser.lock().await;
        if let Some(b) = guard.as_ref() {
            if b.is_alive() {
                return Ok(b.clone());
            }
        }
        let new = BrowserSession::launch(&self.chromium_path, &self.data_dir, self.clone()).await?;
        *guard = Some(new.clone());
        Ok(new)
    }

    /// Ensure the audio pipeline is running. Returns the handle, or None
    /// if no_audio is set / platform unsupported.
    pub async fn ensure_audio(self: &Arc<Self>) -> anyhow::Result<Option<AudioPipeline>> {
        if self.no_audio {
            return Ok(None);
        }
        let mut guard = self.audio.lock().await;
        if let Some(a) = guard.as_ref() {
            return Ok(Some(a.clone()));
        }
        let new = AudioPipeline::start(self.audio_backend.as_deref(), self.clone()).await?;
        *guard = Some(new.clone());
        Ok(Some(new))
    }
}
