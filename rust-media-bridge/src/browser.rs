//! Headless Chromium session management via CDP. The bridge owns a single
//! Chromium process with a persistent user-data-dir (so cookies / FairPlay
//! / Widevine state persist across runs). We expose a cheap Clone handle
//! (BrowserSession) that all providers share.
//!
//! Chromium is launched with:
//!   --user-data-dir=<data_dir>/chromium
//!   --autoplay-policy=no-user-gesture-required
//!   --disable-blink-features=AutomationControlled
//!   --disable-features=IsolateOrigins,site-per-process (needed for some iframe players)
//!
//! We prefer visible-but-minimized over true `--headless=new` because
//! Apple Music's player is flakier in headless mode. The window is moved
//! off-screen and given size 1x1 after launch — effectively invisible to
//! the user while keeping the renderer happy.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::Page;
use futures_util::StreamExt;
use tokio::sync::RwLock;

use crate::state::BridgeState;

/// Cheap clone handle over an Arc<Inner>.
#[derive(Clone)]
pub struct BrowserSession {
    inner: Arc<Inner>,
}

struct Inner {
    browser: RwLock<Option<Browser>>,
    /// The single page we use for playback. Providers navigate this page.
    page: RwLock<Option<Page>>,
    data_dir: PathBuf,
}

impl BrowserSession {
    pub async fn launch(
        chromium: &Path,
        data_dir: &Path,
        _state: Arc<BridgeState>,
    ) -> Result<Self> {
        let profile_dir = data_dir.join("chromium");
        std::fs::create_dir_all(&profile_dir)?;

        let viewport = Viewport {
            width: 1024,
            height: 768,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        };

        // We stay windowed (not true --headless) because FairPlay/Widevine
        // + MusicKit work more reliably with a real graphics context.
        // Chromium is launched off-screen and tiny so it doesn't bother the
        // user visually. On Linux without a display server, fall back to
        // --headless=new.
        let has_display = std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some();

        let mut cfg = BrowserConfig::builder()
            .chrome_executable(chromium)
            .user_data_dir(&profile_dir)
            .viewport(viewport)
            .arg("--autoplay-policy=no-user-gesture-required")
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-infobars")
            .arg("--disable-features=TranslateUI,BlinkGenPropertyTrees")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--window-position=-2000,-2000")
            .arg("--window-size=1,1");

        if !has_display {
            cfg = cfg.arg("--headless=new");
        }

        let cfg = cfg
            .build()
            .map_err(|e| anyhow!("BrowserConfig build failed: {e}"))?;

        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .context("failed to launch Chromium")?;

        // Chromiumoxide requires us to pump the handler stream in the
        // background — otherwise CDP events never flow.
        tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if let Err(e) = h {
                    log::warn!("chromium handler error: {e}");
                }
            }
        });

        // Create a single page we'll reuse. about:blank starts it clean.
        let page = browser
            .new_page(CreateTargetParams::new("about:blank"))
            .await
            .context("failed to create initial page")?;

        // Hide webdriver flag to reduce automation fingerprinting.
        let _ = page
            .evaluate(
                "Object.defineProperty(navigator, 'webdriver', { get: () => undefined });",
            )
            .await;

        Ok(Self {
            inner: Arc::new(Inner {
                browser: RwLock::new(Some(browser)),
                page: RwLock::new(Some(page)),
                data_dir: data_dir.to_path_buf(),
            }),
        })
    }

    pub fn is_alive(&self) -> bool {
        // Synchronous best-effort — if the lock is busy, assume alive.
        self.inner
            .browser
            .try_read()
            .map(|g| g.is_some())
            .unwrap_or(true)
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    pub async fn page(&self) -> Result<Page> {
        let guard = self.inner.page.read().await;
        guard
            .clone()
            .ok_or_else(|| anyhow!("browser page not initialized"))
    }

    /// Navigate the single page to `url` and wait for load.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let page = self.page().await?;
        page.goto(url).await?;
        // Wait for DOMContentLoaded — loose but fast. Providers that need
        // more specific readiness should poll after this.
        page.wait_for_navigation().await?;
        Ok(())
    }

    /// Evaluate JS in the page and return the JSON result.
    pub async fn eval<T: serde::de::DeserializeOwned>(&self, js: &str) -> Result<T> {
        let page = self.page().await?;
        let result = page.evaluate(js).await?;
        Ok(result.into_value()?)
    }

    /// Run JS that doesn't return a useful value.
    pub async fn exec(&self, js: &str) -> Result<()> {
        let page = self.page().await?;
        let _ = page.evaluate(js).await?;
        Ok(())
    }

    /// Poll a JS expression until it returns truthy, or timeout.
    pub async fn wait_for_js(&self, js: &str, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("wait_for_js timeout: {js}"));
            }
            let v: serde_json::Value = self.eval(js).await.unwrap_or(serde_json::Value::Null);
            if truthy(&v) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Shut the browser down cleanly.
    pub async fn close(&self) -> Result<()> {
        let mut page_guard = self.inner.page.write().await;
        if let Some(p) = page_guard.take() {
            let _ = p.close().await;
        }
        let mut br_guard = self.inner.browser.write().await;
        if let Some(mut b) = br_guard.take() {
            let _ = b.close().await;
            let _ = b.wait().await;
        }
        Ok(())
    }
}

fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

/// Locate a Chromium-family binary on the user's system. Preference order
/// favors channels with Widevine CDM bundled (Chrome, Edge) over pure
/// Chromium builds that may lack it.
pub fn detect_chromium() -> Option<PathBuf> {
    let candidates: &[&str] = &[
        "google-chrome-stable",
        "google-chrome",
        "chrome",
        "microsoft-edge-stable",
        "microsoft-edge",
        "brave-browser",
        "chromium",
        "chromium-browser",
    ];
    for name in candidates {
        if let Ok(path) = which_like(name) {
            return Some(path);
        }
    }
    // macOS app-bundle paths
    #[cfg(target_os = "macos")]
    for p in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ] {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // Windows typical install paths
    #[cfg(target_os = "windows")]
    for p in [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ] {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Minimal `which` implementation so we don't need another dep.
fn which_like(name: &str) -> Result<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path.split(sep) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        #[cfg(windows)]
        {
            let exe = candidate.with_extension("exe");
            if exe.is_file() {
                return Ok(exe);
            }
        }
    }
    Err(anyhow!("{name} not found in PATH"))
}
