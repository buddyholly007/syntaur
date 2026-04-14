//! Apple Music driver via music.apple.com web player.
//!
//! Apple's web player uses MusicKit JS. The stable control surface is the
//! player DOM: a play-button, scrubber, next/prev, etc. We drive by JS
//! synthetic clicks + direct `HTMLMediaElement` access when possible.
//!
//! The URL `https://music.apple.com/us/song/<id>?l=en-US&at=...` takes us
//! directly to a track with an auto-play intent. The player still requires
//! one "play" click which we synthesize after the DOM settles.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::time::Duration;

use crate::browser::BrowserSession;
use crate::events::NowPlaying;

use super::Provider;

pub struct AppleMusic;

const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[async_trait]
impl Provider for AppleMusic {
    fn id(&self) -> &'static str {
        "apple_music"
    }

    async fn play(&self, browser: &BrowserSession, track_id: &str) -> Result<()> {
        let url = format!(
            "https://music.apple.com/us/song/{}?l=en-US",
            urlencode(track_id)
        );
        log::info!("[apple_music] navigate {url}");
        browser.navigate(&url).await?;

        // Wait for the MusicKit controller to attach. The Apple Music web
        // app exposes `MusicKit.getInstance()` once ready.
        browser
            .wait_for_js(
                "(typeof MusicKit !== 'undefined') && !!MusicKit.getInstance",
                LOAD_TIMEOUT,
            )
            .await
            .map_err(|e| anyhow!("MusicKit didn't initialize: {e}"))?;

        // Kick playback. Two approaches in sequence: first ask the
        // controller directly (works when the queue has been set), then
        // synthesize a click on the play button as a fallback.
        let js = r#"
        (async () => {
          try {
            const mk = MusicKit.getInstance();
            if (mk.isAuthorized === false) { return { ok:false, reason:'unauthorized' }; }
            try { await mk.play(); return { ok:true, via:'musickit' }; }
            catch (e) {
              const btn = document.querySelector('button.play-button, [aria-label*="Play" i]');
              if (btn) { btn.click(); return { ok:true, via:'click' }; }
              return { ok:false, reason: String(e) };
            }
          } catch (e) {
            return { ok:false, reason: String(e) };
          }
        })()
        "#;
        let r: serde_json::Value = browser.eval(js).await?;
        log::info!("[apple_music] play result: {r}");
        if r.get("ok") != Some(&serde_json::Value::Bool(true)) {
            let reason = r
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            if reason == "unauthorized" {
                return Err(anyhow!(
                    "Apple Music not authenticated. Run setup: syntaur-media-bridge --auth-setup --auth-provider apple_music"
                ));
            }
            return Err(anyhow!("play failed: {reason}"));
        }

        Ok(())
    }

    async fn pause(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "try { MusicKit.getInstance().pause(); } catch(e){} \
                 const b=document.querySelector('button.play-button, [aria-label*=\"Pause\" i]'); \
                 if (b && /pause/i.test(b.getAttribute('aria-label')||'')) b.click();",
            )
            .await
    }

    async fn resume(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec(
                "try { MusicKit.getInstance().play(); } catch(e){} \
                 const b=document.querySelector('button.play-button, [aria-label*=\"Play\" i]'); \
                 if (b && /play/i.test(b.getAttribute('aria-label')||'')) b.click();",
            )
            .await
    }

    async fn next(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("try { MusicKit.getInstance().skipToNextItem(); } catch(e){}")
            .await
    }

    async fn prev(&self, browser: &BrowserSession) -> Result<()> {
        browser
            .exec("try { MusicKit.getInstance().skipToPreviousItem(); } catch(e){}")
            .await
    }

    async fn seek(&self, browser: &BrowserSession, position_ms: u64) -> Result<()> {
        let secs = (position_ms as f64) / 1000.0;
        browser
            .exec(&format!(
                "try {{ MusicKit.getInstance().seekToTime({secs}); }} catch(e){{}}"
            ))
            .await
    }

    async fn set_volume(&self, browser: &BrowserSession, level: f32) -> Result<()> {
        let level = level.clamp(0.0, 1.0);
        browser
            .exec(&format!(
                "try {{ MusicKit.getInstance().volume = {level}; }} catch(e){{}}"
            ))
            .await
    }

    async fn now_playing(&self, browser: &BrowserSession) -> Result<Option<NowPlaying>> {
        let js = r#"
        (() => {
          try {
            const mk = MusicKit.getInstance();
            const np = mk.nowPlayingItem;
            if (!np) return null;
            return {
              provider: 'apple_music',
              track_id: String(np.id || ''),
              name: np.attributes?.name || np.title || '',
              artist: np.attributes?.artistName || np.artistName || '',
              duration_ms: Math.round((np.attributes?.durationInMillis ?? np.playbackDuration * 1000) || 0),
              position_ms: Math.round((mk.currentPlaybackTime || 0) * 1000),
              playing: mk.playbackState === 2 /* PLAYING */
            };
          } catch (e) { return null; }
        })()
        "#;
        let v: serde_json::Value = browser.eval(js).await.unwrap_or(serde_json::Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(NowPlaying {
            provider: "apple_music".into(),
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

fn urlencode(s: &str) -> String {
    // track IDs are numeric, but be safe against path traversal / weirdness
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect()
}
