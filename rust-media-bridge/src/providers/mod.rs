//! Provider dispatch. Each streaming service has its own module that
//! knows how to load + drive its web player via CDP.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;

use crate::browser::BrowserSession;
use crate::events::{BridgeEvent, NowPlaying, PlayRequest};
use crate::state::BridgeState;

pub mod apple_music;
pub mod spotify;
pub mod tidal;
pub mod youtube_music;

/// A provider knows how to talk to one streaming service's web player.
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    /// Navigate the browser to the track and start playback.
    async fn play(&self, browser: &BrowserSession, track_id: &str) -> Result<()>;

    /// Pause current track. OK if there isn't one.
    async fn pause(&self, browser: &BrowserSession) -> Result<()>;

    /// Resume after pause.
    async fn resume(&self, browser: &BrowserSession) -> Result<()>;

    /// Skip to next in whatever queue the provider's player has.
    async fn next(&self, browser: &BrowserSession) -> Result<()>;

    /// Go back to previous track.
    async fn prev(&self, browser: &BrowserSession) -> Result<()>;

    /// Seek within the current track.
    async fn seek(&self, browser: &BrowserSession, position_ms: u64) -> Result<()>;

    /// Set volume 0.0 - 1.0 inside the provider's player. Best-effort — if
    /// the provider doesn't expose JS volume control, this is a no-op.
    async fn set_volume(&self, browser: &BrowserSession, level: f32) -> Result<()>;

    /// Extract a now-playing snapshot from the current DOM.
    async fn now_playing(&self, browser: &BrowserSession) -> Result<Option<NowPlaying>>;
}

pub fn dispatch(id: &str) -> Result<Box<dyn Provider>> {
    match id {
        "apple_music" => Ok(Box::new(apple_music::AppleMusic)),
        "spotify" => Ok(Box::new(spotify::Spotify)),
        "tidal" => Ok(Box::new(tidal::Tidal)),
        "youtube_music" => Ok(Box::new(youtube_music::YouTubeMusic)),
        other => Err(anyhow!("unknown provider: {other}")),
    }
}

/// State-tracked current provider. Stored on BridgeState via a side
/// channel (RwLock<Option<String>>) so pause/resume/next/prev know which
/// provider is active without the gateway having to remember.
pub async fn current_provider(state: &Arc<BridgeState>) -> Option<String> {
    state
        .now_playing
        .read()
        .await
        .as_ref()
        .map(|np| np.provider.clone())
}

pub async fn play(state: &Arc<BridgeState>, req: &PlayRequest) -> Result<()> {
    let provider = dispatch(&req.provider)?;
    let browser = state.ensure_browser().await?;

    // Start audio pipeline in parallel so it's warm by the time audio
    // actually emerges. If it fails, we log but continue — user still
    // gets playback via default system routing.
    let state_for_audio = state.clone();
    tokio::spawn(async move {
        if let Err(e) = state_for_audio.ensure_audio().await {
            log::warn!("audio pipeline init failed: {e:#}");
        }
    });

    provider.play(&browser, &req.track_id).await?;

    // Mark now-playing immediately with caller-provided metadata; a later
    // metadata-poll loop will enrich it with artwork/duration.
    let np = NowPlaying {
        provider: req.provider.clone(),
        track_id: req.track_id.clone(),
        name: req.name.clone().unwrap_or_default(),
        artist: req.artist.clone().unwrap_or_default(),
        position_ms: 0,
        duration_ms: None,
        playing: true,
    };
    *state.now_playing.write().await = Some(np.clone());
    state.emit(BridgeEvent::NowPlaying {
        provider: np.provider,
        track_id: np.track_id,
        name: np.name,
        artist: np.artist,
        album: None,
        artwork_url: None,
        duration_ms: np.duration_ms,
    });

    // Kick off (or keep alive) the metadata poller for this session.
    crate::providers::start_metadata_poller(state.clone());

    Ok(())
}

pub async fn pause(state: &Arc<BridgeState>) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Ok(());
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.pause(&browser).await
}

pub async fn resume(state: &Arc<BridgeState>) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Ok(());
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.resume(&browser).await
}

pub async fn stop(state: &Arc<BridgeState>) -> Result<()> {
    // "Stop" = pause + clear now-playing. We intentionally don't navigate
    // away so the next play command is faster.
    if let Some(id) = current_provider(state).await {
        let provider = dispatch(&id)?;
        let browser = state.ensure_browser().await?;
        let _ = provider.pause(&browser).await;
    }
    *state.now_playing.write().await = None;
    Ok(())
}

pub async fn next_track(state: &Arc<BridgeState>) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Err(anyhow!("no active provider"));
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.next(&browser).await
}

pub async fn prev_track(state: &Arc<BridgeState>) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Err(anyhow!("no active provider"));
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.prev(&browser).await
}

pub async fn seek(state: &Arc<BridgeState>, position_ms: u64) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Err(anyhow!("no active provider"));
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.seek(&browser, position_ms).await
}

pub async fn set_provider_volume(state: &Arc<BridgeState>, level: f32) -> Result<()> {
    let Some(id) = current_provider(state).await else {
        return Ok(());
    };
    let provider = dispatch(&id)?;
    let browser = state.ensure_browser().await?;
    provider.set_volume(&browser, level).await
}

use std::sync::OnceLock;
use tokio::sync::Mutex as TokioMutex;

fn poller_running() -> Arc<TokioMutex<bool>> {
    static POLLER_RUNNING: OnceLock<Arc<TokioMutex<bool>>> = OnceLock::new();
    POLLER_RUNNING
        .get_or_init(|| Arc::new(TokioMutex::new(false)))
        .clone()
}

/// Ensure a metadata-polling task is running. Idempotent — a second call
/// while one is already live is a no-op.
fn start_metadata_poller(state: Arc<BridgeState>) {
    let mu = poller_running();
    tokio::spawn(async move {
        {
            let mut g = mu.lock().await;
            if *g {
                return;
            }
            *g = true;
        }
        let _ = run_metadata_poller(state).await;
        let mut g = mu.lock().await;
        *g = false;
    });
}

async fn run_metadata_poller(state: Arc<BridgeState>) -> Result<()> {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let Some(id) = current_provider(&state).await else {
            break;
        };
        let provider = match dispatch(&id) {
            Ok(p) => p,
            Err(_) => break,
        };
        let browser = match state.ensure_browser().await {
            Ok(b) => b,
            Err(_) => break,
        };
        if let Ok(Some(np)) = provider.now_playing(&browser).await {
            let mut guard = state.now_playing.write().await;
            let changed_track = guard
                .as_ref()
                .map(|old| old.track_id != np.track_id)
                .unwrap_or(true);
            *guard = Some(np.clone());
            drop(guard);
            if changed_track {
                state.emit(BridgeEvent::NowPlaying {
                    provider: np.provider.clone(),
                    track_id: np.track_id.clone(),
                    name: np.name.clone(),
                    artist: np.artist.clone(),
                    album: None,
                    artwork_url: None,
                    duration_ms: np.duration_ms,
                });
            }
            state.emit(BridgeEvent::Position {
                position_ms: np.position_ms,
                duration_ms: np.duration_ms,
                playing: np.playing,
            });
        }
    }
    Ok(())
}
