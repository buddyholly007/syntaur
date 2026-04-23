//! chromiumoxide wrapper — launch headless Chromium, render a URL,
//! screenshot it, and collect the console log.
//!
//! Phase 1 scope: single-page render + PNG screenshot + console
//! messages. Phase 3 adds viewport sweep (desktop/tablet/mobile).
//! Phase 4 adds interaction walks (click element, wait, screenshot
//! again). Keep this surface small now so the Phase 3+4 additions
//! don't require touching the CLI wiring.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::{Browser as CdpBrowser, BrowserConfig};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

/// One browser instance. Drop closes Chromium.
pub struct Browser {
    inner: CdpBrowser,
    _handler: tokio::task::JoinHandle<()>,
}

/// One per-URL capture result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageCapture {
    pub url: String,
    pub screenshot_path: PathBuf,
    pub console_messages: Vec<String>,
    pub http_status: Option<u16>,
    pub elapsed_ms: u64,
}

impl Browser {
    pub async fn launch() -> Result<Self> {
        // Chromium 147 (snap on claudevm, Arch chromium on gaming PC)
        // refuses several of chromiumoxide's built-in DEFAULT_ARGS —
        // most visibly `--disable-background-networking`, which makes
        // the browser exit with "unrecognized command line flag"
        // before CDP can connect. Fix: opt out of the entire legacy
        // DEFAULT_ARGS list and pass a minimal, Chromium-147-friendly
        // set ourselves. Also switch to `--headless=new` (the modern
        // headless pipeline) — the legacy `--headless` mode is on
        // its way out and has diverging CDP semantics from headed
        // Chromium, which is what we'll eventually want to match
        // against baselines.
        let (inner, mut handler) = CdpBrowser::launch(
            BrowserConfig::builder()
                .no_sandbox()
                .new_headless_mode()
                .disable_default_args()
                .args([
                    // Essential stability flags for headless/VM/container runs.
                    "--disable-dev-shm-usage",
                    "--disable-gpu",
                    "--no-first-run",
                    "--no-default-browser-check",
                    // Suppress prompts that would block the CDP session.
                    "--disable-popup-blocking",
                    "--disable-prompt-on-repost",
                    // Keep keyring/password prompts from freezing startup.
                    "--password-store=basic",
                    "--use-mock-keychain",
                    // Deterministic color + locale for screenshot diffs.
                    "--force-color-profile=srgb",
                    "--lang=en-US",
                ])
                // Consistent viewport — Phase 3 will parameterize.
                .window_size(1440, 900)
                .build()
                .map_err(|e| anyhow::anyhow!("chromium config: {e}"))?,
        )
        .await
        .context("launching headless chromium (install chromium/chromium-browser if missing)")?;

        // The handler stream must be polled for the browser to work.
        let handle = tokio::task::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        Ok(Self { inner, _handler: handle })
    }

    /// Render `url`, wait for load, capture a PNG to `out_dir` +
    /// console messages. `page_slug` becomes the PNG filename stem.
    pub async fn capture(
        &self,
        url: &str,
        page_slug: &str,
        out_dir: &Path,
    ) -> Result<PageCapture> {
        std::fs::create_dir_all(out_dir).ok();
        let start = std::time::Instant::now();

        let page = self
            .inner
            .new_page("about:blank")
            .await
            .context("creating new page")?;

        // Subscribe to console events BEFORE navigation.
        let mut console_events = page
            .event_listener::<chromiumoxide::cdp::browser_protocol::log::EventEntryAdded>()
            .await
            .context("attaching console listener")?;
        let console_task: tokio::task::JoinHandle<Vec<String>> = tokio::task::spawn(async move {
            let mut buf = Vec::new();
            // Collect for 2.5s post-load; enough to catch FOUC / late errors.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
            loop {
                match tokio::time::timeout_at(deadline, console_events.next()).await {
                    Ok(Some(ev)) => {
                        let entry = &ev.entry;
                        buf.push(format!("{:?}: {}", entry.level, entry.text));
                    }
                    _ => break,
                }
            }
            buf
        });

        page.goto(url).await.context("page.goto")?;
        page.wait_for_navigation().await.context("waiting for navigation")?;

        // Give FOUC + post-load JS a moment to settle. Phase 3 will
        // move to a smarter "domContentLoaded + 2s" signal.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let png = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .full_page(true)
                    .build(),
            )
            .await
            .context("capturing screenshot")?;

        let screenshot_path = out_dir.join(format!("{}.png", page_slug));
        std::fs::write(&screenshot_path, &png)
            .with_context(|| format!("writing {}", screenshot_path.display()))?;

        // Best-effort HTTP status via JS `performance` API. Not all
        // CDP versions surface the main-frame response status
        // cleanly; the Phase 3 pass will wire it via the Network
        // domain directly.
        let http_status: Option<u16> = page
            .evaluate(
                r#"
                (() => {
                    const e = performance.getEntriesByType('navigation')[0];
                    return e && e.responseStatus ? e.responseStatus : null;
                })()
                "#,
            )
            .await
            .ok()
            .and_then(|v| v.into_value::<u16>().ok());

        let console = console_task.await.unwrap_or_default();
        let elapsed_ms = start.elapsed().as_millis() as u64;

        page.close().await.ok();

        Ok(PageCapture {
            url: url.to_string(),
            screenshot_path,
            console_messages: console,
            http_status,
            elapsed_ms,
        })
    }
}
