//! Tidal driver via listen.tidal.com web player.
//!
//! `track_id` is Tidal's numeric track id.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::time::Duration;

use crate::browser::BrowserSession;
use crate::events::NowPlaying;

use super::Provider;

pub struct Tidal;

const LOAD_TIMEOUT: Duration = Duration::from_secs(25);

#[async_trait]
impl Provider for Tidal {
    fn id(&self) -> &'static str {
        "tidal"
    }

    async fn play(&self, browser: &BrowserSession, track_id: &str) -> Result<()> {
        let id = sanitize(track_id);
        let url = format!("https://listen.tidal.com/track/{}", id);
        log::info!("[tidal] navigate {url}");
        browser.navigate(&url).await?;

        browser
            .wait_for_js(
                "!!document.querySelector('[data-test=\"play\"], [data-test=\"pause\"], button[data-test^=\"play\"]')",
                LOAD_TIMEOUT,
            )
            .await
            .map_err(|e| anyhow!("Tidal player didn't load: {e}"))?;

        browser
            .exec(
                r#"
                const btn = document.querySelector('[data-test="play"]')
                         || document.querySelector('button[data-test^="play"]');
                if (btn) btn.click();
                "#,
            )
            .await?;
        Ok(())
    }

    async fn pause(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "document.querySelector('[data-test=\"pause\"]')?.click();",
            )
            .await
    }

    async fn resume(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "document.querySelector('[data-test=\"play\"]')?.click();",
            )
            .await
    }

    async fn next(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('[data-test=\"next\"]')?.click();")
            .await
    }

    async fn prev(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('[data-test=\"previous\"]')?.click();")
            .await
    }

    async fn seek(&self, browser: &BrowserSession, position_ms: u64) -> Result<()> {
        let secs = (position_ms as f64) / 1000.0;
        browser
            .exec(&format!(
                r#"
                const a = document.querySelector('audio, video');
                if (a) {{ try {{ a.currentTime = {secs}; }} catch(e){{}} }}
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
            const a = document.querySelector('audio, video');
            const name = document.querySelector('[data-test="footer-track-title"]')?.textContent?.trim() || '';
            const artist = document.querySelector('[data-test="grid-item-detail-text-title-artist"]')?.textContent?.trim() || '';
            const link = document.querySelector('a[data-test="footer-track-title"]');
            let track_id = '';
            if (link) { const m = link.href.match(/\/track\/(\d+)/); if (m) track_id = m[1]; }
            if (!name) return null;
            return {
              provider:'tidal', track_id, name, artist,
              position_ms: a ? Math.round(a.currentTime * 1000) : 0,
              duration_ms: a && isFinite(a.duration) ? Math.round(a.duration * 1000) : null,
              playing: a && !a.paused && !a.ended
            };
          } catch (e) { return null; }
        })()
        "#;
        let v: serde_json::Value = browser.eval(js).await.unwrap_or(serde_json::Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(NowPlaying {
            provider: "tidal".into(),
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
