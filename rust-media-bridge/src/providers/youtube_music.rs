//! YouTube Music driver via music.youtube.com.
//!
//! `track_id` is YouTube's videoId (11-char base64url).

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::time::Duration;

use crate::browser::BrowserSession;
use crate::events::NowPlaying;

use super::Provider;

pub struct YouTubeMusic;

const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[async_trait]
impl Provider for YouTubeMusic {
    fn id(&self) -> &'static str {
        "youtube_music"
    }

    async fn play(&self, browser: &BrowserSession, track_id: &str) -> Result<()> {
        let id = sanitize(track_id);
        let url = format!("https://music.youtube.com/watch?v={}", id);
        log::info!("[yt_music] navigate {url}");
        browser.navigate(&url).await?;

        browser
            .wait_for_js(
                "!!document.querySelector('video.video-stream, .ytmusic-player-bar')",
                LOAD_TIMEOUT,
            )
            .await
            .map_err(|e| anyhow!("YT Music player didn't load: {e}"))?;

        // YT auto-plays when `?autoplay=1` isn't blocked; also try clicking
        // the player play button if it's paused.
        browser
            .exec(
                r#"
                const v = document.querySelector('video.video-stream');
                if (v && v.paused) { v.play().catch(()=>{}); }
                const btn = document.querySelector('#play-pause-button');
                if (v && v.paused && btn) btn.click();
                "#,
            )
            .await?;
        Ok(())
    }

    async fn pause(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('video.video-stream')?.pause();")
            .await
    }

    async fn resume(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('video.video-stream')?.play().catch(()=>{});")
            .await
    }

    async fn next(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('.next-button')?.click();")
            .await
    }

    async fn prev(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("document.querySelector('.previous-button')?.click();")
            .await
    }

    async fn seek(&self, browser: &BrowserSession, position_ms: u64) -> Result<()> {
        let secs = (position_ms as f64) / 1000.0;
        browser
            .exec(&format!(
                "const v=document.querySelector('video.video-stream'); if (v) v.currentTime={secs};"
            ))
            .await
    }

    async fn set_volume(&self, browser: &BrowserSession, level: f32) -> Result<()> {
        let level = level.clamp(0.0, 1.0);
        browser
            .exec(&format!(
                "const v=document.querySelector('video.video-stream'); if (v) v.volume={level};"
            ))
            .await
    }

    async fn now_playing(&self, browser: &BrowserSession) -> Result<Option<NowPlaying>> {
        let js = r#"
        (() => {
          try {
            const v = document.querySelector('video.video-stream');
            const name = document.querySelector('.ytmusic-player-bar .title')?.textContent?.trim() || '';
            const artist = document.querySelector('.ytmusic-player-bar .byline')?.textContent?.trim() || '';
            const u = new URL(location.href);
            const track_id = u.searchParams.get('v') || '';
            if (!name && !track_id) return null;
            return {
              provider:'youtube_music', track_id, name, artist,
              position_ms: v ? Math.round(v.currentTime * 1000) : 0,
              duration_ms: v && isFinite(v.duration) ? Math.round(v.duration * 1000) : null,
              playing: v && !v.paused && !v.ended
            };
          } catch (e) { return null; }
        })()
        "#;
        let v: serde_json::Value = browser.eval(js).await.unwrap_or(serde_json::Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(NowPlaying {
            provider: "youtube_music".into(),
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
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}
