//! Syntaur Media Bridge — runs on the user's desktop machine, drives a
//! headless Chromium pointed at streaming services (Apple Music, Spotify,
//! Tidal, YouTube Music). Captures decrypted audio at the OS layer and
//! routes it to the user's default output device. Gateway talks to us via
//! local HTTP/WS on port 18790; browser UI reaches us the same way.
//!
//! FairPlay / Widevine DRM decrypts inside Chromium exactly like a normal
//! tab — we don't circumvent anything. The "bridge" just means we host the
//! player out-of-sight and pipe audio through our own UI as if it were a
//! native player.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

mod audio;
mod auth;
mod browser;
mod events;
mod providers;
mod server;
mod state;

use state::BridgeState;

#[derive(Parser, Debug, Clone)]
#[command(name = "syntaur-media-bridge", version, about)]
struct Args {
    /// HTTP/WS bind address (default: 127.0.0.1:18790). Bridge is
    /// intentionally localhost-only — remote machines should not be able
    /// to command audio playback here.
    #[arg(long, default_value = "127.0.0.1:18790")]
    bind: String,

    /// Chromium binary path. If unset, auto-detect (chromium, google-chrome,
    /// google-chrome-stable, chrome, brave-browser in PATH).
    #[arg(long)]
    chromium: Option<PathBuf>,

    /// Data directory for persistent browser profile + config (default:
    /// OS-appropriate XDG dir).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Skip audio pipeline (useful for headless servers / CI where we just
    /// want metadata and CDP control without OS-level audio routing).
    #[arg(long)]
    no_audio: bool,

    /// Force a specific platform audio backend (linux-pipewire, linux-pulse,
    /// windows-wasapi, macos-screencapturekit). Auto-detect if unset.
    #[arg(long)]
    audio_backend: Option<String>,

    /// Open a visible Chromium window for the first-run login flow, then
    /// exit. Used by the setup wizard.
    #[arg(long)]
    auth_setup: bool,

    /// Which provider the auth-setup flow targets (apple_music, spotify,
    /// tidal, youtube_music).
    #[arg(long, default_value = "apple_music")]
    auth_provider: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,chromiumoxide=warn,hyper=warn"),
    )
    .format_timestamp_secs()
    .init();

    let args = Args::parse();

    let data_dir = args.data_dir.clone().unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir)?;
    log::info!("data_dir = {}", data_dir.display());

    // Chromium location is resolved lazily — the bridge still starts and
    // serves /status even without Chromium, so the UI can show a clear
    // "install Chromium" message instead of a dead bridge.
    let chromium = args.chromium.clone().or_else(browser::detect_chromium);
    match chromium.as_ref() {
        Some(p) => log::info!("chromium = {}", p.display()),
        None => log::warn!(
            "No Chromium binary found (checked google-chrome, chromium, brave, edge). \
             Bridge will run but /play requests will fail until one is installed."
        ),
    }

    // Auth wizard mode: open visible window for login, wait until the user
    // signs in, then exit. Cookies persist in data_dir for future headless
    // use.
    if args.auth_setup {
        let c = chromium
            .ok_or_else(|| anyhow::anyhow!("Auth wizard requires Chromium — install Chrome, Chromium, Brave, or Edge first."))?;
        return auth::run_auth_wizard(&args.auth_provider, &data_dir, &c).await;
    }

    let chromium_for_state = chromium.unwrap_or_else(|| std::path::PathBuf::from("chromium-not-installed"));
    let state = Arc::new(BridgeState::new(
        data_dir.clone(),
        chromium_for_state,
        args.no_audio,
        args.audio_backend.clone(),
    ));

    server::serve(&args.bind, state).await?;
    Ok(())
}

fn default_data_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "syntaur", "media-bridge") {
        return dirs.data_dir().to_path_buf();
    }
    std::env::temp_dir().join("syntaur-media-bridge")
}
