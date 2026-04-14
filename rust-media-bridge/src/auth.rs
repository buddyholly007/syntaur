//! Auth wizard — opens a visible Chromium window for first-run login on
//! each provider, then persists cookies to the bridge's profile dir so
//! future headless runs inherit the session.
//!
//! We detect "authenticated" by checking for the provider-specific cookie
//! jar entries that survive a logged-in session (e.g., music.apple.com
//! sets `media-user-token`, Spotify sets `sp_dc`, etc.).

use anyhow::{anyhow, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub async fn run_auth_wizard(
    provider: &str,
    data_dir: &Path,
    chromium: &Path,
) -> Result<()> {
    let profile_dir = data_dir.join("chromium");
    std::fs::create_dir_all(&profile_dir)?;

    let url = provider_login_url(provider)?;
    log::info!("Opening visible Chromium for {provider} login: {url}");

    let cfg = BrowserConfig::builder()
        .chrome_executable(chromium)
        .user_data_dir(&profile_dir)
        .with_head() // visible
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-infobars")
        .build()
        .map_err(|e| anyhow!("BrowserConfig build failed: {e}"))?;

    let (browser, mut handler) = Browser::launch(cfg).await?;
    let handler_task = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if let Err(e) = h {
                log::warn!("chromium handler error: {e}");
            }
        }
    });

    let page = browser.new_page(CreateTargetParams::new(url)).await?;
    println!();
    println!("╭─────────────────────────────────────────────────────────╮");
    println!("│  Syntaur Media Bridge — first-run login                 │");
    println!("├─────────────────────────────────────────────────────────┤");
    println!("│  Sign in to {provider:<44}│");
    println!("│  in the Chromium window that just opened.               │");
    println!("│                                                         │");
    println!("│  Once signed in and playing, close the window.          │");
    println!("│  Cookies will be saved so the bridge can play headless. │");
    println!("╰─────────────────────────────────────────────────────────╯");
    println!();

    // Poll until the user closes the browser window (target closed)
    // OR we detect an authed cookie. Whichever happens first.
    let cookie_check_interval = Duration::from_secs(3);
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(cookie_check_interval).await;
        if start.elapsed() > Duration::from_secs(60 * 15) {
            log::warn!("auth wizard timeout (15 min) — closing");
            break;
        }
        // Page closed? (chromiumoxide doesn't expose a clean is_alive, so
        // we attempt a lightweight navigation check)
        let still_alive = page.evaluate("1+1").await.is_ok();
        if !still_alive {
            log::info!("auth wizard: page closed, wrapping up");
            break;
        }
        // Authed?
        if let Ok(authed) = check_authed(&page, provider).await {
            if authed {
                log::info!("auth wizard: detected {provider} auth cookies");
                mark_authed(data_dir, provider);
                println!("✓ {provider} authenticated. You can close the window now.");
            }
        }
    }

    let _ = page.close().await;
    drop(browser);
    handler_task.abort();
    Ok(())
}

fn provider_login_url(provider: &str) -> Result<&'static str> {
    match provider {
        "apple_music" => Ok("https://music.apple.com/us/login"),
        "spotify" => Ok("https://accounts.spotify.com/en/login"),
        "tidal" => Ok("https://listen.tidal.com/login"),
        "youtube_music" => Ok("https://accounts.google.com/ServiceLogin?service=youtube"),
        other => Err(anyhow!("unknown auth provider: {other}")),
    }
}

async fn check_authed(page: &chromiumoxide::Page, provider: &str) -> Result<bool> {
    let expr = match provider {
        "apple_music" => "document.cookie.includes('media-user-token') || !!localStorage.getItem('music.kitAuthToken') || !!localStorage.getItem('music.metadata.user')",
        "spotify" => "document.cookie.includes('sp_dc') || document.cookie.includes('sp_t')",
        "tidal" => "!!localStorage.getItem('_TIDAL_activeSession') || document.cookie.includes('tidal')",
        "youtube_music" => "document.cookie.includes('SAPISID') || document.cookie.includes('__Secure-1PSID')",
        _ => return Ok(false),
    };
    let v: serde_json::Value = page.evaluate(expr).await?.into_value()?;
    Ok(v.as_bool().unwrap_or(false))
}

/// Scan the profile dir for provider cookie jars and return the list of
/// providers that look authenticated (without having to launch Chromium).
/// This is a lightweight check for /status.
pub fn detect_authed_providers(data_dir: &Path) -> Result<Vec<String>> {
    let profile = data_dir.join("chromium");
    if !profile.exists() {
        return Ok(vec![]);
    }

    // Chromium's Cookies SQLite is locked while the browser runs. We use
    // a heuristic — check the Local Storage / IndexedDB dirs have
    // relevant entries. For a definitive check, call /status with the
    // browser running and it'll query via CDP.
    let mut authed: Vec<String> = vec![];

    // Simpler marker file: we write `<provider>.authed` into the data
    // dir when the wizard completes successfully. That way auth state
    // is cheap to read without touching the profile.
    for name in ["apple_music", "spotify", "tidal", "youtube_music"] {
        let marker = data_dir.join(format!("{name}.authed"));
        if marker.exists() {
            authed.push(name.to_string());
        }
    }
    Ok(authed)
}

/// Called by the auth wizard after successful sign-in to mark the
/// provider as authed. Non-fatal if it fails.
pub fn mark_authed(data_dir: &Path, provider: &str) {
    let marker = data_dir.join(format!("{provider}.authed"));
    if let Err(e) = std::fs::write(&marker, chrono::Utc::now().to_rfc3339()) {
        log::warn!("failed to write auth marker {}: {e}", marker.display());
    }
}

#[allow(dead_code)]
pub fn clear_auth(data_dir: &Path, provider: &str) -> Result<()> {
    let marker = data_dir.join(format!("{provider}.authed"));
    if marker.exists() {
        std::fs::remove_file(marker)?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn profile_path(data_dir: &Path) -> PathBuf {
    data_dir.join("chromium")
}
