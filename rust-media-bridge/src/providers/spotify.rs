//! Spotify driver via open.spotify.com web player.
//!
//! Spotify Premium users can use the official Web Playback SDK — but that
//! requires our UI to host the SDK. Here we drive the web player at
//! open.spotify.com directly, which works for both Premium and Free
//! (Free has ads + skip restrictions, same as the Spotify desktop app).
//!
//! `track_id` accepts either a bare Spotify ID or `spotify:track:<id>` URI.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::time::Duration;

use crate::browser::BrowserSession;
use crate::events::NowPlaying;

use super::Provider;

pub struct Spotify;

const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[async_trait]
impl Provider for Spotify {
    fn id(&self) -> &'static str {
        "spotify"
    }

    async fn play(&self, browser: &BrowserSession, track_id: &str) -> Result<()> {
        let id = track_id
            .strip_prefix("spotify:track:")
            .unwrap_or(track_id);
        let url = format!("https://open.spotify.com/track/{}", sanitize(id));
        log::info!("[spotify] navigate {url}");
        browser.navigate(&url).await?;

        // Wait for the primary play button — its data-testid is stable.
        browser
            .wait_for_js(
                "!!document.querySelector('[data-testid=\"play-button\"], [data-testid=\"control-button-playpause\"]')",
                LOAD_TIMEOUT,
            )
            .await
            .map_err(|e| anyhow!("Spotify player didn't load: {e}"))?;

        // Click the big play button on the track page.
        browser
            .exec(
                r#"
                const btn = document.querySelector('[data-testid="play-button"]')
                         || document.querySelector('[data-testid="control-button-playpause"]');
                if (btn) btn.click();
                "#,
            )
            .await?;
        Ok(())
    }

    async fn pause(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                r#"
                const b = document.querySelector('[data-testid="control-button-playpause"]');
                const lbl = b?.getAttribute('aria-label') || '';
                if (b && /pause/i.test(lbl)) b.click();
                "#,
            )
            .await
    }

    async fn resume(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                r#"
                const b = document.querySelector('[data-testid="control-button-playpause"]');
                const lbl = b?.getAttribute('aria-label') || '';
                if (b && /play/i.test(lbl)) b.click();
                "#,
            )
            .await
    }

    async fn next(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "document.querySelector('[data-testid=\"control-button-skip-forward\"]')?.click();",
            )
            .await
    }

    async fn prev(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "document.querySelector('[data-testid=\"control-button-skip-back\"]')?.click();",
            )
            .await
    }

    async fn seek(&self, browser: &BrowserSession, position_ms: u64) -> Result<()> {
        // No stable public JS API in the web player; seeking via the
        // progress bar requires geometry math. For now we only support
        // seek-to-start/end by best-effort.
        let frac = (position_ms as f64) / 1000.0;
        browser
            .exec(&format!(
                r#"
                const a = document.querySelector('audio, video');
                if (a && isFinite({frac})) {{ try {{ a.currentTime = {frac}; }} catch(e){{}} }}
                "#
            ))
            .await
    }

    async fn set_volume(&self, browser: &BrowserSession, level: f32) -> Result<()> {
        let level = level.clamp(0.0, 1.0);
        browser
            .exec(&format!(
                r#"
                document.querySelectorAll('audio, video').forEach(a => {{ try {{ a.volume = {level}; }} catch(e){{}} }});
                "#
            ))
            .await
    }

    async fn now_playing(&self, browser: &BrowserSession) -> Result<Option<NowPlaying>> {
        let js = r#"
        (() => {
          try {
            const name = document.querySelector('[data-testid="context-item-info-title"] a')?.textContent?.trim()
                     || document.querySelector('[data-testid="now-playing-widget"] a')?.textContent?.trim()
                     || '';
            const artist = document.querySelector('[data-testid="context-item-info-artist"] a')?.textContent?.trim()
                     || Array.from(document.querySelectorAll('[data-testid="now-playing-widget"] a')).map(a=>a.textContent).join(', ')
                     || '';
            const a = document.querySelector('audio, video');
            const playing = a && !a.paused && !a.ended;
            const position_ms = a ? Math.round(a.currentTime * 1000) : 0;
            const duration_ms = a && isFinite(a.duration) ? Math.round(a.duration * 1000) : null;
            // Extract track id from URL of the link in the now-playing widget
            const link = document.querySelector('[data-testid="now-playing-widget"] a[href*="/track/"]');
            let track_id = '';
            if (link) { const m = link.href.match(/\/track\/([A-Za-z0-9]+)/); if (m) track_id = m[1]; }
            if (!name && !track_id) return null;
            return { provider:'spotify', track_id, name, artist, position_ms, duration_ms, playing };
          } catch (e) { return null; }
        })()
        "#;
        let v: serde_json::Value = browser.eval(js).await.unwrap_or(serde_json::Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(NowPlaying {
            provider: "spotify".into(),
            track_id: v
                .get("track_id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            name: v
                .get("name")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            artist: v
                .get("artist")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            duration_ms: v.get("duration_ms").and_then(|n| n.as_u64()),
            position_ms: v
                .get("position_ms")
                .and_then(|n| n.as_u64())
                .unwrap_or(0),
            playing: v
                .get("playing")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
        }))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect()
}
