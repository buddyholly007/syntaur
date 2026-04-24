//! chromiumoxide wrapper — launch headless Chromium, render a URL,
//! screenshot it, and collect the console log.
//!
//! Phase 1 scope: single-page render + PNG screenshot + console
//! messages. Phase 3 adds viewport sweep (desktop/tablet/mobile) via
//! the CDP Emulation domain — `Browser::launch_with_viewport` sets
//! device metrics + mobile emulation at launch time so every page
//! opened on the instance uses that profile. Phase 4 adds interaction
//! walks (click element, wait, screenshot again). Keep this surface
//! small now so later additions don't require touching the CLI wiring.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::handler::viewport::Viewport as CdpViewport;
use chromiumoxide::page::Page as CdpPage;
use chromiumoxide::{Browser as CdpBrowser, BrowserConfig};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

/// Cross-device viewport target. Phase 3 sweeps all three per module
/// so baselines can catch e.g. a tablet-only layout regression that
/// a desktop-only pass would miss.
///
/// Dimensions match the widely-cited "reference devices" that most
/// design systems QA against:
///   * Desktop: 1440x900 (MacBook Air / common laptop)
///   * Tablet : 768x1024 (iPad portrait)
///   * Mobile : 375x812  (iPhone 13/14 portrait)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Viewport {
    Desktop,
    Tablet,
    Mobile,
}

impl Viewport {
    /// (width, height) in CSS pixels.
    pub fn dims(&self) -> (u32, u32) {
        match self {
            Viewport::Desktop => (1440, 900),
            Viewport::Tablet => (768, 1024),
            Viewport::Mobile => (375, 812),
        }
    }

    /// Filename-friendly slug, also used as the baseline subdirectory
    /// key. Stable across versions — baselines are keyed on this
    /// string, renaming it invalidates every saved image.
    pub fn slug(&self) -> &'static str {
        match self {
            Viewport::Desktop => "desktop",
            Viewport::Tablet => "tablet",
            Viewport::Mobile => "mobile",
        }
    }

    /// Device-pixel ratio for retina / mobile screens. Mobile bumps
    /// to 2.0 to mirror iPhone behaviour; desktop + tablet stay at 1.0.
    pub fn device_scale_factor(&self) -> f64 {
        match self {
            Viewport::Desktop | Viewport::Tablet => 1.0,
            Viewport::Mobile => 2.0,
        }
    }

    /// Whether Chromium should treat this as a mobile device (touch
    /// events, meta viewport respected, mobile UA string).
    pub fn is_mobile(&self) -> bool {
        matches!(self, Viewport::Mobile)
    }

    /// User-Agent override. Desktop leaves it alone (None → Chromium's
    /// native UA), tablet + mobile present as the matching Apple
    /// device so sites with UA-sniffed mobile shells render in the
    /// shape we're auditing.
    pub fn user_agent(&self) -> Option<&'static str> {
        match self {
            Viewport::Desktop => None,
            Viewport::Tablet => Some(
                "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) \
                 AppleWebKit/605.1.15 (KHTML, like Gecko) \
                 Version/17.0 Mobile/15E148 Safari/604.1",
            ),
            Viewport::Mobile => Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) \
                 AppleWebKit/605.1.15 (KHTML, like Gecko) \
                 Version/17.0 Mobile/15E148 Safari/604.1",
            ),
        }
    }

    /// Translate to chromiumoxide's launch-time viewport struct.
    pub(crate) fn to_cdp(self) -> CdpViewport {
        let (w, h) = self.dims();
        CdpViewport {
            width: w,
            height: h,
            device_scale_factor: Some(self.device_scale_factor()),
            emulating_mobile: self.is_mobile(),
            is_landscape: false,
            has_touch: self.is_mobile(),
        }
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport::Desktop
    }
}

/// One browser instance. Drop closes Chromium.
pub struct Browser {
    inner: CdpBrowser,
    viewport: Viewport,
    /// Optional `Authorization: Bearer <token>` injected into every
    /// outgoing request (via CDP Network.setExtraHTTPHeaders) plus a
    /// `syntaur_token` sessionStorage seed (via
    /// Page.addScriptToEvaluateOnNewDocument) so widget `sdFetch()`
    /// calls also carry the bearer. Set via `with_auth_token`.
    auth_token: Option<String>,
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
    /// Viewport the capture was taken under — baselines are keyed on
    /// (module_slug, viewport), so carrying this forward avoids having
    /// to re-thread the parameter through every call site.
    #[serde(default)]
    pub viewport: Viewport,
}

impl Browser {
    /// Back-compat — launches a desktop-viewport browser. Phase 1 + 2
    /// callers (fix.rs auto-fix loop, tests) use this exact signature
    /// and must keep compiling unchanged.
    pub async fn launch() -> Result<Self> {
        Self::launch_with_viewport(Viewport::Desktop).await
    }

    /// Launch a browser pinned to a specific viewport. Every page
    /// opened on this instance uses the same device metrics + UA, so
    /// the Phase 3 CLI launches one per viewport in sequence rather
    /// than toggling at the page level.
    pub async fn launch_with_viewport(viewport: Viewport) -> Result<Self> {
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
        let (w, h) = viewport.dims();

        // Chromium binary selection, in priority order:
        //   1. SYNTAUR_VERIFY_CHROME env var — explicit override
        //   2. ~/.local/chrome/chrome-linux64/chrome — a pinned
        //      chrome-for-testing install. chromiumoxide 0.7 has a CDP
        //      regression against Chromium ≥147 (`page.goto` returns
        //      `oneshot canceled`); pinning to chrome-for-testing 131
        //      avoids it while also dodging the snap wrapper entirely.
        //      Install with:
        //        curl -s -o /tmp/cft.zip https://storage.googleapis.com/chrome-for-testing-public/131.0.6778.264/linux64/chrome-linux64.zip
        //        python3 -m zipfile -e /tmp/cft.zip ~/.local/chrome/
        //        chmod +x ~/.local/chrome/chrome-linux64/{chrome,chrome_crashpad_handler}
        //   3. /snap/chromium/current/usr/lib/chromium-browser/chrome —
        //      bypasses the snap Go-wrapper arg-stripping. Works on
        //      claudevm when chrome-for-testing isn't installed.
        //   4. None → let chromiumoxide autodetect `chromium` /
        //      `chromium-browser` on PATH.
        let home = std::env::var("HOME").unwrap_or_default();
        let cft_path = PathBuf::from(format!("{home}/.local/chrome/chrome-linux64/chrome"));
        let snap_inner = PathBuf::from("/snap/chromium/current/usr/lib/chromium-browser/chrome");
        let chrome_path: Option<PathBuf> = std::env::var("SYNTAUR_VERIFY_CHROME")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .or_else(|| if cft_path.exists() { Some(cft_path) } else { None })
            .or_else(|| if snap_inner.exists() { Some(snap_inner) } else { None });

        // `--headless=new` vs `--headless=old`: chromiumoxide 0.7 + new
        // headless pipeline has an unresolved bug where Target.createTarget
        // (invoked from `browser.new_page("about:blank")`) hangs until the
        // CDP oneshot is dropped → "oneshot canceled" at every capture
        // attempt. Fall back to legacy headless until chromiumoxide
        // ships the fix. Fully supported on Chromium 131+ headed too.
        // Drop `.disable_default_args()` entirely — chromiumoxide 0.7's
        // default arg set includes things the CDP session depends on
        // (pipe handling, DBus suppression, crash handler wiring). On
        // Chromium 131 (chrome-for-testing) all the args the prior
        // "unknown flag" errors flagged are in fact accepted; we were
        // only seeing rejections from the snap Go-wrapper. Let
        // chromiumoxide drive the defaults and just layer our viewport
        // + UA stability flags on top.
        let mut cfg = BrowserConfig::builder()
            .no_sandbox()
            .args([
                "--headless",
                "--force-color-profile=srgb",
                "--lang=en-US",
            ])
            .window_size(w, h)
            .viewport(viewport.to_cdp());
        if let Some(p) = chrome_path {
            cfg = cfg.chrome_executable(p);
        }
        let (inner, mut handler) = CdpBrowser::launch(
            cfg.build()
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

        Ok(Self {
            inner,
            viewport,
            auth_token: None,
            _handler: handle,
        })
    }

    /// Attach a bearer token. Every subsequent `capture_*` call will
    /// inject `Authorization: Bearer <token>` via CDP + seed the
    /// `syntaur_token` sessionStorage key (so `sdFetch` widget calls
    /// carry the header too). Pass `None` to clear.
    pub fn with_auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token;
        self
    }

    /// Which viewport this browser was launched with.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Open a new page, apply the viewport's UA if any, and navigate
    /// to `url`. Returned page is owned by the caller — drop or
    /// `.close()` it when done.
    ///
    /// Phase 4 flow runner uses this to drive multi-step interactions
    /// (click/type/press) against a single page instance without
    /// re-launching Chromium per step.
    pub async fn new_page(&self, url: &str) -> Result<CdpPage> {
        let page = self
            .inner
            .new_page("about:blank")
            .await
            .context("creating new page")?;
        if let Some(ua) = self.viewport.user_agent() {
            if let Err(e) = page.set_user_agent(ua).await {
                log::warn!(
                    "[browser] failed to set user-agent for {}: {e:#} \
                     — continuing with Chromium default",
                    self.viewport.slug()
                );
            }
        }

        // Apply auth token if present — mirrors the capture_inner
        // pathway so flow steps hit the same authenticated surfaces
        // module captures do. Factored out here (not duplicated)
        // because capture_inner is the authoritative impl; we re-use
        // its two-prong (header + sessionStorage) approach inline.
        if let Some(token) = &self.auth_token {
            use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;
            use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
            let mut headers = std::collections::HashMap::new();
            headers.insert(
                "Authorization".to_string(),
                serde_json::Value::String(format!("Bearer {token}")),
            );
            if let Ok(hdrs_json) = serde_json::to_value(headers) {
                if let Err(e) = page
                    .execute(SetExtraHttpHeadersParams {
                        headers: chromiumoxide::cdp::browser_protocol::network::Headers::new(
                            hdrs_json,
                        ),
                    })
                    .await
                {
                    log::warn!("[browser] new_page: failed to set auth header: {e:#}");
                }
            }
            let seed = format!(
                "try {{ sessionStorage.setItem('syntaur_token', {}); }} catch (e) {{}}",
                serde_json::to_string(token).unwrap_or_else(|_| "\"\"".into())
            );
            if let Err(e) = page
                .execute(AddScriptToEvaluateOnNewDocumentParams {
                    source: seed,
                    world_name: None,
                    include_command_line_api: None,
                    run_immediately: None,
                })
                .await
            {
                log::warn!("[browser] new_page: failed to seed sessionStorage token: {e:#}");
            }
        }

        page.goto(url).await.context("page.goto")?;
        page.wait_for_navigation()
            .await
            .context("waiting for navigation")?;
        Ok(page)
    }

    /// Back-compat capture method. Screenshot filename is
    /// `<page_slug>.png` to keep Phase 1/2 behaviour for callers that
    /// don't care about viewport. New callers should prefer
    /// `capture_with_viewport`, which adds the viewport suffix.
    pub async fn capture(
        &self,
        url: &str,
        page_slug: &str,
        out_dir: &Path,
    ) -> Result<PageCapture> {
        self.capture_inner(url, page_slug, out_dir, None).await
    }

    /// Render `url`, wait for load, capture a PNG to `out_dir` +
    /// console messages. Filename: `<page_slug>_<viewport>.png`.
    ///
    /// `page_slug` becomes the first half of the PNG filename stem.
    /// The viewport suffix is appended automatically so a single run
    /// directory can hold desktop/tablet/mobile shots side-by-side.
    pub async fn capture_with_viewport(
        &self,
        url: &str,
        page_slug: &str,
        out_dir: &Path,
    ) -> Result<PageCapture> {
        self.capture_inner(url, page_slug, out_dir, Some(self.viewport))
            .await
    }

    async fn capture_inner(
        &self,
        url: &str,
        page_slug: &str,
        out_dir: &Path,
        suffix_viewport: Option<Viewport>,
    ) -> Result<PageCapture> {
        std::fs::create_dir_all(out_dir).ok();
        let start = std::time::Instant::now();

        let page = self
            .inner
            .new_page("about:blank")
            .await
            .context("creating new page")?;

        // Per-page UA override for tablet/mobile — the launch-time
        // viewport handles device metrics + `emulating_mobile`, but
        // not UA, so sites with UA-sniffed layouts still need this.
        if let Some(ua) = self.viewport.user_agent() {
            if let Err(e) = page.set_user_agent(ua).await {
                log::warn!(
                    "[browser] failed to set user-agent for {}: {e:#} \
                     — continuing with Chromium default",
                    self.viewport.slug()
                );
            }
        }

        // Auth injection — two paths needed:
        //   1. extraHTTPHeaders so the main doc fetch + any widget
        //      XHR/fetch without our sdFetch wrapper carry the bearer
        //   2. sessionStorage seed on every new document so Syntaur's
        //      `sdFetch` widget helper reads `syntaur_token` and adds
        //      the Authorization header on its own fetch calls
        if let Some(token) = &self.auth_token {
            use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;
            use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

            let mut headers = std::collections::HashMap::new();
            headers.insert(
                "Authorization".to_string(),
                serde_json::Value::String(format!("Bearer {token}")),
            );
            let hdrs_json = serde_json::to_value(headers)
                .context("serializing auth header map")?;
            if let Err(e) = page
                .execute(SetExtraHttpHeadersParams {
                    headers: chromiumoxide::cdp::browser_protocol::network::Headers::new(
                        hdrs_json,
                    ),
                })
                .await
            {
                log::warn!("[browser] failed to set Authorization header: {e:#}");
            }
            // Seed sessionStorage for sdFetch. `addScriptToEvaluateOnNewDocument`
            // runs before any page JS on every new document (including
            // iframes), so the key is there by the time widget init fires.
            let seed = format!(
                "try {{ sessionStorage.setItem('syntaur_token', {}); }} catch (e) {{}}",
                serde_json::to_string(token).unwrap_or_else(|_| "\"\"".into())
            );
            if let Err(e) = page
                .execute(AddScriptToEvaluateOnNewDocumentParams {
                    source: seed,
                    world_name: None,
                    include_command_line_api: None,
                    run_immediately: None,
                })
                .await
            {
                log::warn!("[browser] failed to seed sessionStorage token: {e:#}");
            }
        }

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

        let filename = match suffix_viewport {
            Some(v) => format!("{}_{}.png", page_slug, v.slug()),
            None => format!("{}.png", page_slug),
        };
        let screenshot_path = out_dir.join(filename);
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
            viewport: self.viewport,
        })
    }
}
