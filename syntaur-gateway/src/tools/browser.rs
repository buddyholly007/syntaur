use futures_util::{SinkExt, StreamExt};
use log::{info, warn, error, debug};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

/// CDP (Chrome DevTools Protocol) client with persistent WebSocket connection.
/// Launches Chromium, maintains a single WebSocket to the page target,
/// multiplexes commands by id, listens for page lifecycle events.

static CMD_ID: AtomicU64 = AtomicU64::new(1);
static BROWSER: Mutex<Option<BrowserInstance>> = Mutex::const_new(None);

struct BrowserInstance {
    ws: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        tokio_tungstenite::tungstenite::Message,
    >,
    pending: std::sync::Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    child: tokio::process::Child,
    xvfb: Option<tokio::process::Child>,
    display: Option<String>,
    page_loaded: std::sync::Arc<tokio::sync::Notify>,
    _reader_handle: tokio::task::JoinHandle<()>,
    /// Per-launch user-data-dir, e.g. `/tmp/chromium-profile-1775582400123456789`.
    /// A unique dir per launch prevents Chromium's SingletonLock from turning a
    /// second launcher into a "remote command" client (which exits clean with
    /// status 0 and confuses our try_wait-based health check).
    user_data_dir: String,
}

fn chromium_path() -> &'static str {
    if std::path::Path::new("/snap/bin/chromium").exists() { "/snap/bin/chromium" }
    else if std::path::Path::new("/usr/bin/chromium-browser").exists() { "/usr/bin/chromium-browser" }
    else if std::path::Path::new("/usr/bin/chromium").exists() { "/usr/bin/chromium" }
    else { "chromium" }
}

/// Tear down the current browser session: kill the chromium process tree,
/// kill xvfb, abort the WS reader, drop the BROWSER global, and remove the
/// per-launch profile dir. Idempotent — safe to call when no session exists.
///
/// Called at the start of every `browser_open` so each navigation entry point
/// is a fresh, isolated session (no leftover cookies, localStorage, navigation
/// history, dialogs, or zombie chromium processes from previous tool calls).
pub async fn teardown_browser() {
    let mut guard = BROWSER.lock().await;
    let Some(mut inst) = guard.take() else { return };

    info!("[browser] Tearing down session (profile {})", inst.user_data_dir);

    // Stop the WS reader first so it doesn't log spurious read errors when the
    // socket gets torn down by the chromium kill below.
    inst._reader_handle.abort();

    // Kill chromium parent. SIGKILL via tokio::Child::kill is sufficient — the
    // pkill below catches any helper processes (zygote, gpu-process, renderers)
    // that might survive the parent on systems where they didn't share a pgid.
    let _ = inst.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), inst.child.wait()).await;

    // Catch any stragglers tied to this user-data-dir.
    let _ = Command::new("pkill")
        .args(["-9", "-f", &inst.user_data_dir])
        .output().await;

    if let Some(mut xvfb) = inst.xvfb.take() {
        let _ = xvfb.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), xvfb.wait()).await;
    }

    // Remove the profile dir so we don't accumulate orphan dirs over time.
    let _ = tokio::fs::remove_dir_all(&inst.user_data_dir).await;
}

/// Launch or reconnect to the browser. Returns () on success.
async fn ensure_browser() -> Result<(), String> {
    let mut guard = BROWSER.lock().await;

    // Check if existing browser is still alive
    if let Some(ref mut inst) = *guard {
        match inst.child.try_wait() {
            Ok(Some(status)) => {
                warn!("[browser] Chromium exited ({}), relaunching...", status);
                // Kill xvfb too
                if let Some(ref mut xvfb) = inst.xvfb {
                    xvfb.kill().await.ok();
                }
                inst._reader_handle.abort();
                let stale_dir = inst.user_data_dir.clone();
                *guard = None;
                // Best-effort cleanup of the stale per-launch profile dir.
                let _ = tokio::fs::remove_dir_all(&stale_dir).await;
            }
            Ok(None) => return Ok(()), // still running
            Err(e) => {
                warn!("[browser] Cannot check Chromium status: {}, relaunching", e);
                *guard = None;
            }
        }
    }

    // Launch new browser
    let instance = launch_browser().await?;
    *guard = Some(instance);
    Ok(())
}

async fn launch_browser() -> Result<BrowserInstance, String> {
    let port = 9222;
    let use_xvfb = std::path::Path::new("/usr/bin/Xvfb").exists()
        || std::path::Path::new("/usr/bin/xvfb-run").exists();

    // Aggressive cleanup of any leftover Chromium that could collide with us.
    // `fuser -k 9222/tcp` only kills the process directly bound to the port,
    // but a stale Chromium tree may still hold the user-data-dir SingletonLock
    // even after the parent is gone. pkill the whole tree by user-data-dir
    // pattern. The pattern matches both the legacy `/tmp/chromium-profile*`
    // dirs (snap-private, from earlier fix iterations) and the current
    // `~/.cache/syntaur-browser/profile-*` dirs.
    let _ = Command::new("pkill")
        .args(["-9", "-f", "chromium-profile|syntaur-browser/profile-"])
        .output().await;
    let _ = Command::new("fuser")
        .args(["-k", &format!("{}/tcp", port)])
        .output().await;

    // Wait until port 9222 is actually free (up to ~3s).
    for _ in 0..15 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        let in_use = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok();
        if !in_use {
            break;
        }
    }

    // Unique user-data-dir per launch — defeats Chromium's SingletonLock,
    // which would otherwise turn a second launcher into a "remote command"
    // client (it forwards the URL to the existing instance and exits clean
    // with status 0, breaking our try_wait-based health check).
    //
    // We have to be careful where this lives. Snap chromium silently rewrites
    // `--user-data-dir` to its default `~/snap/chromium/common/chromium` if
    // the requested path is outside the paths its `personal-files` interface
    // grants it (which is a short, exact-path list — not patterns). Tested:
    //   - `/tmp/...`              → snap-private /tmp namespace; host-invisible
    //   - `~/.cache/...`          → silently redirected to snap default (!)
    //   - `~/snap/chromium/common/...` → respected, parent + all child procs
    //                                    actually use the path we asked for
    //
    // So we put profiles under `~/snap/chromium/common/syntaur-browser/`,
    // which is snap chromium's own writable area, host-visible (so we can
    // clean it up), and respected by snap chromium for --user-data-dir.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let parent = format!("{}/snap/chromium/common/syntaur-browser", home);
    // Wipe and recreate the parent dir to clear any orphan profile dirs left
    // by previous runs (e.g. when syntaur was killed before the per-
    // instance cleanup in ensure_browser could run). Safe because pkill above
    // already killed any chromium that could be using these dirs.
    let _ = tokio::fs::remove_dir_all(&parent).await;
    let _ = tokio::fs::create_dir_all(&parent).await;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let user_data_dir = format!("{}/profile-{}", parent, nanos);

    let (xvfb_child, display) = if use_xvfb {
        info!("[browser] Launching Xvfb virtual display :99...");
        let xvfb = Command::new("Xvfb")
            .args([":99", "-screen", "0", "1280x720x24", "-ac", "-nolisten", "tcp"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let has_xvfb = xvfb.is_some();
        if has_xvfb { info!("[browser] Xvfb started on :99"); }
        else { warn!("[browser] Xvfb failed, falling back to headless"); }
        (xvfb, if has_xvfb { Some(":99".to_string()) } else { None })
    } else {
        (None, None)
    };

    let headless_mode = display.is_none();
    info!("[browser] Launching Chromium (headless={})...", headless_mode);

    // Detect actual Chromium version for UA consistency
    let version_output = Command::new(chromium_path())
        .arg("--version")
        .output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let chrome_version = version_output
        .split_whitespace().last()
        .and_then(|v| v.split('.').next())
        .unwrap_or("131");
    let ua = format!(
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        chrome_version
    );

    let mut args = vec![
        "--no-sandbox".to_string(),
        "--disable-gpu".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--disable-software-rasterizer".to_string(),
        "--no-first-run".to_string(),
        "--disable-extensions".to_string(),
        "--disable-blink-features=AutomationControlled".to_string(),
        format!("--user-agent={}", ua),
        "--disable-infobars".to_string(),
        "--enable-webgl".to_string(),
        "--disable-component-extensions-with-background-pages".to_string(),
        format!("--user-data-dir={}", user_data_dir),
        format!("--remote-debugging-port={}", port),
        "--remote-allow-origins=*".to_string(),
        "--window-size=1280,720".to_string(),
        "about:blank".to_string(),
    ];

    if headless_mode {
        args.insert(0, "--headless=new".to_string());
    } else {
        args.push("--kiosk".to_string());
    }

    let mut cmd = Command::new(chromium_path());
    cmd.args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    if let Some(ref disp) = display {
        cmd.env("DISPLAY", disp);
    }

    let mut child = cmd.spawn()
        .map_err(|e| format!("Failed to launch Chromium: {}", e))?;

    // Belt-and-suspenders: catch the singleton-client scenario early. If the
    // spawned process has already exited within the first ~300ms, it became a
    // "remote command" client. CDP might still appear to work because port 9222
    // is held by the OTHER chromium, but our health check (try_wait on this
    // PID) would lie. Fail fast so the caller retries cleanly.
    tokio::time::sleep(Duration::from_millis(300)).await;
    if let Ok(Some(status)) = child.try_wait() {
        // Best-effort cleanup of the dir we just created.
        let _ = tokio::fs::remove_dir_all(&user_data_dir).await;
        return Err(format!(
            "Chromium exited immediately after launch (status {}). \
             A stale instance is likely holding port 9222 or a SingletonLock — \
             check `pgrep -af chromium` and `/tmp/chromium-profile-*`.",
            status
        ));
    }

    // Wait for DevTools to be ready
    let client = reqwest::Client::new();
    let mut page_ws_url = String::new();
    for attempt in 0..20 {
        tokio::time::sleep(Duration::from_millis(if attempt < 4 { 500 } else { 250 })).await;
        match client.get(format!("http://127.0.0.1:{}/json", port))
            .timeout(Duration::from_secs(2))
            .send().await
        {
            Ok(resp) => {
                if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
                    if let Some(url) = targets.iter()
                        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                        .and_then(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()))
                    {
                        page_ws_url = url.to_string();
                        break;
                    }
                }
            }
            Err(_) => continue,
        }
    }

    if page_ws_url.is_empty() {
        return Err("Failed to get DevTools WebSocket URL".to_string());
    }

    info!("[browser] Chromium ready: {}", page_ws_url);

    // Establish persistent WebSocket connection
    let (ws_stream, _) = tokio_tungstenite::connect_async(&page_ws_url)
        .await
        .map_err(|e| format!("WebSocket connect error: {}", e))?;

    let (ws_write, mut ws_read) = ws_stream.split();

    // Shared state for pending command responses
    let pending: std::sync::Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>> =
        std::sync::Arc::new(Mutex::new(HashMap::new()));
    let pending_clone = pending.clone();

    // Notify for page load events
    let page_loaded = std::sync::Arc::new(tokio::sync::Notify::new());
    let page_loaded_clone = page_loaded.clone();

    // Spawn reader task — routes responses to waiting callers, handles events
    let reader_handle = tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&text) {
                        // Command response (has "id")
                        if let Some(id) = resp.get("id").and_then(|v| v.as_u64()) {
                            let mut map = pending_clone.lock().await;
                            if let Some(sender) = map.remove(&id) {
                                sender.send(resp).ok();
                            }
                        }
                        // CDP event (has "method")
                        else if let Some(method) = resp.get("method").and_then(|v| v.as_str()) {
                            match method {
                                "Page.loadEventFired" | "Page.lifecycleEvent" => {
                                    let name = resp.get("params")
                                        .and_then(|p| p.get("name"))
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("load");
                                    if method == "Page.loadEventFired" || name == "networkIdle" || name == "load" {
                                        page_loaded_clone.notify_waiters();
                                    }
                                }
                                _ => {} // ignore other events
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("[browser] WebSocket read error: {}", e);
                    break;
                }
                _ => {}
            }
        }
        warn!("[browser] WebSocket reader ended");
    });

    let mut instance = BrowserInstance {
        ws: ws_write,
        pending,
        child,
        xvfb: xvfb_child,
        display,
        page_loaded,
        _reader_handle: reader_handle,
        user_data_dir,
    };

    // Enable CDP domains we need
    send_cdp_raw(&mut instance, "Page.enable", json!({})).await.ok();
    send_cdp_raw(&mut instance, "Page.setLifecycleEventsEnabled", json!({"enabled": true})).await.ok();
    send_cdp_raw(&mut instance, "Runtime.enable", json!({})).await.ok();

    // Inject stealth script for all future page loads
    let stealth_js = concat!(
        "Object.defineProperty(navigator, 'webdriver', { get: () => undefined });",
        // Proper PluginArray emulation
        "Object.defineProperty(navigator, 'plugins', { get: () => {",
        "  const p = { 0: {name:'Chrome PDF Plugin',description:'Portable Document Format',filename:'internal-pdf-viewer',length:1,0:{description:'PDF',suffixes:'pdf',type:'application/x-google-chrome-pdf'}},",
        "    1: {name:'Chrome PDF Viewer',description:'',filename:'mhjfbmdgcfjbbpaeojofohoefgiehjai',length:1,0:{description:'PDF',suffixes:'pdf',type:'application/pdf'}},",
        "    2: {name:'Native Client',description:'',filename:'internal-nacl-plugin',length:2,0:{description:'',suffixes:'',type:'application/x-nacl'},1:{description:'',suffixes:'',type:'application/x-pnacl'}},",
        "    length: 3, item: function(i){return this[i];}, namedItem: function(n){for(let i=0;i<this.length;i++){if(this[i].name===n)return this[i];}return null;}, refresh: function(){} };",
        "  return p; }});",
        "Object.defineProperty(navigator, 'languages', { get: () => ['en-US','en'] });",
        "window.chrome = { runtime: {}, loadTimes: function(){}, csi: function(){}, app: {} };",
        "const origQuery = window.navigator.permissions.query;",
        "window.navigator.permissions.query = (parameters) => (",
        "  parameters.name === 'notifications' ?",
        "    Promise.resolve({ state: Notification.permission }) :",
        "    origQuery(parameters)",
        ");",
        "const getParameter = WebGLRenderingContext.prototype.getParameter;",
        "WebGLRenderingContext.prototype.getParameter = function(parameter) {",
        "  if (parameter === 37445) return 'Intel Inc.';",
        "  if (parameter === 37446) return 'Intel Iris OpenGL Engine';",
        "  return getParameter.call(this, parameter);",
        "};",
        // Canvas fingerprint noise — add imperceptible pixel modifications
        "const origToDataURL = HTMLCanvasElement.prototype.toDataURL;",
        "HTMLCanvasElement.prototype.toDataURL = function(type) {",
        "  const ctx = this.getContext('2d');",
        "  if (ctx) {",
        "    const s = ctx.getImageData(0, 0, Math.min(this.width, 16), Math.min(this.height, 16));",
        "    for (let i = 0; i < s.data.length; i += 4) {",
        "      s.data[i] = s.data[i] ^ (((i * 1103515245 + 12345) >> 16) & 1);",
        "    }",
        "    ctx.putImageData(s, 0, 0);",
        "  }",
        "  return origToDataURL.apply(this, arguments);",
        "};",
        "const origToBlob = HTMLCanvasElement.prototype.toBlob;",
        "HTMLCanvasElement.prototype.toBlob = function() {",
        "  const ctx = this.getContext('2d');",
        "  if (ctx) {",
        "    const s = ctx.getImageData(0, 0, Math.min(this.width, 16), Math.min(this.height, 16));",
        "    for (let i = 0; i < s.data.length; i += 4) {",
        "      s.data[i] = s.data[i] ^ (((i * 1103515245 + 12345) >> 16) & 1);",
        "    }",
        "    ctx.putImageData(s, 0, 0);",
        "  }",
        "  return origToBlob.apply(this, arguments);",
        "};",
        // AudioContext fingerprint — add small noise to getFloatFrequencyData
        "const origGetFloat = AnalyserNode.prototype.getFloatFrequencyData;",
        "AnalyserNode.prototype.getFloatFrequencyData = function(array) {",
        "  origGetFloat.call(this, array);",
        "  for (let i = 0; i < array.length; i++) { array[i] += 0.0001 * ((i * 7 + 3) % 5 - 2); }",
        "};",
        // Prevent detection of CDP via Runtime.enable side effects
        "delete window.cdc_adoQpoasnfa76pfcZLmcfl_Array;",
        "delete window.cdc_adoQpoasnfa76pfcZLmcfl_Promise;",
        "delete window.cdc_adoQpoasnfa76pfcZLmcfl_Symbol;",
    );

    let resp = send_cdp_raw(&mut instance, "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": stealth_js})).await;
    match resp {
        Ok(r) => {
            if r.get("error").is_some() {
                warn!("[browser] Stealth injection failed: {:?}", r.get("error"));
            } else {
                info!("[browser] Stealth patches injected");
            }
        }
        Err(e) => warn!("[browser] Stealth injection error: {}", e),
    }

    Ok(instance)
}

/// Send a CDP command on the persistent connection (requires mutable BrowserInstance)
async fn send_cdp_raw(inst: &mut BrowserInstance, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let id = CMD_ID.fetch_add(1, Ordering::Relaxed);

    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut map = inst.pending.lock().await;
        map.insert(id, tx);
    }

    let cmd = json!({ "id": id, "method": method, "params": params });
    inst.ws.send(tokio_tungstenite::tungstenite::Message::Text(cmd.to_string()))
        .await
        .map_err(|e| format!("CDP send error: {}", e))?;

    let resp = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .map_err(|_| format!("CDP timeout: {} (id={})", method, id))?
        .map_err(|_| "CDP response channel closed".to_string())?;

    // Check for CDP-level errors
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Err(format!("CDP error ({}): {}", code, msg));
    }

    Ok(resp)
}

/// Send a CDP command — acquires the browser lock, sends, returns response.
/// This is the main entry point for all browser operations.
async fn cdp_command(_ws_url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    ensure_browser().await?;
    let mut guard = BROWSER.lock().await;
    let inst = guard.as_mut().ok_or("Browser not initialized")?;
    send_cdp_raw(inst, method, params).await
}

/// Public CDP command wrapper for use by other modules (e.g., captcha, account)
pub async fn cdp_command_pub(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    cdp_command("", method, params).await
}

/// Extract a JS value from a Runtime.evaluate CDP response
/// Public wrapper for extract_js_value (used by captcha solver)
pub fn extract_js_value_pub(resp: &serde_json::Value) -> Result<String, String> {
    extract_js_value(resp)
}

fn extract_js_value(resp: &serde_json::Value) -> Result<String, String> {
    let result = resp.get("result").ok_or("No result in CDP response")?;

    // Check for JS exceptions
    if let Some(exc) = result.get("exceptionDetails") {
        let text = exc.get("text").and_then(|v| v.as_str()).unwrap_or("unknown exception");
        let desc = exc.get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(format!("JS exception: {} {}", text, desc));
    }

    let val = result.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("(no value)");
    Ok(val.to_string())
}

/// Wait for page load event (with timeout fallback)
async fn wait_for_page_load(timeout_secs: u64) {
    let guard = BROWSER.lock().await;
    if let Some(ref inst) = *guard {
        let notify = inst.page_loaded.clone();
        drop(guard); // release lock while waiting

        let _ = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            notify.notified(),
        ).await;
    } else {
        drop(guard);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Auto-save screenshot on error (returns path in error message)
async fn screenshot_on_error(workspace: &str, error: &str) -> String {
    let result = cdp_command("", "Page.captureScreenshot", json!({"format": "png"})).await;
    if let Ok(resp) = result {
        if let Some(data) = resp.get("result").and_then(|r| r.get("data")).and_then(|v| v.as_str()) {
            if let Ok(bytes) = base64_decode(data) {
                let filename = format!("error-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
                let path = std::path::Path::new(workspace).join(&filename);
                if std::fs::write(&path, &bytes).is_ok() {
                    return format!("{} (screenshot: {})", error, filename);
                }
            }
        }
    }
    error.to_string()
}

// ── Iframe / OOPIF targeting ──────────────────────────────────────────────

/// Get the bounding rect of an iframe matching a URL pattern (e.g., "arkoselabs")
/// Returns (x, y, width, height) in main page viewport coordinates
pub async fn get_iframe_bounds(url_pattern: &str) -> Result<(f64, f64, f64, f64), String> {
    let js = format!(
        concat!(
            "(() => {{ ",
            "const iframes = document.querySelectorAll('iframe'); ",
            "for (const f of iframes) {{ ",
            "  const src = f.src || f.getAttribute('data-src') || ''; ",
            "  const id = f.id || ''; ",
            "  if (src.toLowerCase().includes('{}') || id.toLowerCase().includes('{}')) {{ ",
            "    const r = f.getBoundingClientRect(); ",
            "    return JSON.stringify({{x: r.x, y: r.y, width: r.width, height: r.height, src: src.substring(0,80)}}); ",
            "  }} ",
            "}} ",
            "return 'NOT_FOUND'; ",
            "}})()"
        ),
        url_pattern.to_lowercase(), url_pattern.to_lowercase()
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    let val = extract_js_value(&result)?;

    if val == "NOT_FOUND" {
        return Err(format!("No iframe matching '{}' found", url_pattern));
    }

    let parsed: serde_json::Value = serde_json::from_str(&val)
        .map_err(|e| format!("Parse iframe bounds: {}", e))?;

    let x = parsed.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = parsed.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let w = parsed.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let h = parsed.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

    info!("[browser] Iframe '{}' bounds: ({}, {}) {}x{}", url_pattern, x, y, w, h);
    Ok((x, y, w, h))
}

/// Attach to a cross-origin iframe target and execute JS inside it.
/// Uses Target.attachToTarget + Page.createIsolatedWorld with grantUniversalAccess.
pub async fn eval_in_iframe(url_pattern: &str, js: &str) -> Result<String, String> {
    ensure_browser().await?;

    // Step 1: Get frame tree to find the iframe's frameId
    let frame_tree_resp = cdp_command("", "Page.getFrameTree", json!({})).await?;
    let frame_tree = frame_tree_resp.get("result").and_then(|r| r.get("frameTree"))
        .ok_or("No frameTree in response")?;

    let target_frame_id = find_frame_id(frame_tree, url_pattern);
    if let Some(frame_id) = target_frame_id {
        info!("[browser] Found iframe frameId={} for '{}'", frame_id, url_pattern);

        // Step 2: Create isolated world with universal access
        let world_resp = cdp_command("", "Page.createIsolatedWorld", json!({
            "frameId": frame_id,
            "worldName": "captcha-solver",
            "grantUniversalAccess": true
        })).await?;

        let context_id = world_resp.get("result")
            .and_then(|r| r.get("executionContextId"))
            .and_then(|v| v.as_u64())
            .ok_or("No executionContextId returned")?;

        info!("[browser] Created isolated world contextId={}", context_id);

        // Step 3: Evaluate JS in that context
        let eval_resp = cdp_command("", "Runtime.evaluate", json!({
            "expression": js,
            "contextId": context_id
        })).await?;

        return extract_js_value(&eval_resp);
    }

    // Fallback: connect directly to the iframe's own WebSocket URL from /json list
    info!("[browser] Frame not in tree, trying direct iframe WebSocket connection");
    let client = reqwest::Client::new();
    let targets_resp = client.get("http://127.0.0.1:9222/json")
        .timeout(Duration::from_secs(5))
        .send().await
        .map_err(|e| format!("Target list: {}", e))?;
    let targets: Vec<serde_json::Value> = targets_resp.json().await
        .map_err(|e| format!("Target parse: {}", e))?;

    let iframe_target = targets.iter().find(|t| {
        let ttype = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let url = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
        ttype == "iframe" && url.to_lowercase().contains(&url_pattern.to_lowercase())
    });

    if let Some(target) = iframe_target {
        let ws_url = target.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
            .ok_or("No webSocketDebuggerUrl for iframe")?;
        let iframe_url = target.get("url").and_then(|v| v.as_str()).unwrap_or("?");
        info!("[browser] Connecting directly to iframe WS: {} (url: {})", ws_url, &iframe_url[..iframe_url.len().min(80)]);

        // Open a temporary WebSocket to the iframe target
        let (mut iframe_ws, _) = tokio_tungstenite::connect_async(ws_url).await
            .map_err(|e| format!("Iframe WS connect: {}", e))?;

        let cmd_id = CMD_ID.fetch_add(1, Ordering::Relaxed);
        let cmd = json!({
            "id": cmd_id,
            "method": "Runtime.evaluate",
            "params": {"expression": js}
        });

        iframe_ws.send(tokio_tungstenite::tungstenite::Message::Text(cmd.to_string())).await
            .map_err(|e| format!("Iframe WS send: {}", e))?;

        // Read response
        let resp = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(msg) = iframe_ws.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                        if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&text) {
                            if resp.get("id").and_then(|v| v.as_u64()) == Some(cmd_id) {
                                return Ok(resp);
                            }
                        }
                    }
                    Err(e) => return Err(format!("Iframe WS error: {}", e)),
                    _ => continue,
                }
            }
            Err("Iframe WS closed".to_string())
        }).await.map_err(|_| "Iframe eval timeout".to_string())??;

        iframe_ws.close(None).await.ok();
        return extract_js_value(&resp);
    }

    // Last resort: try Target.getTargets OOPIF attachment
    info!("[browser] No direct WS target, trying Target.getTargets");
    let oopif_resp = cdp_command("", "Target.getTargets", json!({})).await?;
    let oopif_targets = oopif_resp.get("result")
        .and_then(|r| r.get("targetInfos"))
        .and_then(|v| v.as_array())
        .ok_or("No targetInfos")?;

    let oopif_target = oopif_targets.iter().find(|t| {
        let url = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
        url.to_lowercase().contains(&url_pattern.to_lowercase())
    });

    if let Some(target) = oopif_target {
        let target_id = target.get("targetId").and_then(|v| v.as_str())
            .ok_or("No targetId")?;
        info!("[browser] Attaching to OOPIF: {}", target_id);

        let attach_resp = cdp_command("", "Target.attachToTarget", json!({
            "targetId": target_id, "flatten": true
        })).await?;

        let session_id = attach_resp.get("result")
            .and_then(|r| r.get("sessionId"))
            .and_then(|v| v.as_str())
            .ok_or("No sessionId")?;

        let eval_result = cdp_command_in_session(session_id, "Runtime.evaluate", json!({
            "expression": js
        })).await?;

        let _ = cdp_command("", "Target.detachFromTarget", json!({"sessionId": session_id})).await;
        return extract_js_value(&eval_result);
    }

    Err(format!("No iframe target matching '{}' found via any method", url_pattern))
}

/// Send a CDP command with a sessionId (for OOPIF targets)
async fn cdp_command_in_session(session_id: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    ensure_browser().await?;
    let id = CMD_ID.fetch_add(1, Ordering::Relaxed);

    let mut guard = BROWSER.lock().await;
    let inst = guard.as_mut().ok_or("Browser not initialized")?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut map = inst.pending.lock().await;
        map.insert(id, tx);
    }

    // Include sessionId in the command for OOPIF targeting
    let cmd = json!({
        "id": id,
        "method": method,
        "params": params,
        "sessionId": session_id
    });

    inst.ws.send(tokio_tungstenite::tungstenite::Message::Text(cmd.to_string()))
        .await
        .map_err(|e| format!("CDP send error: {}", e))?;

    drop(guard);

    let resp = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .map_err(|_| format!("CDP timeout: {} (session)", method))?
        .map_err(|_| "CDP channel closed".to_string())?;

    if let Some(err) = resp.get("error") {
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Err(format!("CDP error: {}", msg));
    }

    Ok(resp)
}

/// Recursively find a frameId in the frame tree by URL pattern
fn find_frame_id(node: &serde_json::Value, url_pattern: &str) -> Option<String> {
    // Check this frame
    if let Some(frame) = node.get("frame") {
        let url = frame.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.to_lowercase().contains(&url_pattern.to_lowercase()) {
            return frame.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
        }
    }

    // Check child frames
    if let Some(children) = node.get("childFrames").and_then(|v| v.as_array()) {
        for child in children {
            if let Some(id) = find_frame_id(child, url_pattern) {
                return Some(id);
            }
        }
    }

    None
}

// ── Public browser tools ──────────────────────────────────────────────────

/// Navigate to URL and return page content. Waits for page load event.
pub async fn browser_open(_agent_id: &str, url: &str) -> Result<String, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") && url != "about:blank" {
        return Err("Only http:// and https:// URLs allowed".to_string());
    }

    // SSRF guard parity with tools::web::web_fetch. Agent-triggered
    // browser navigation to loopback / RFC1918 / link-local / cloud-
    // metadata is the same pivot vector. Without this, a prompt-inject
    // can route a headless Chromium to http://169.254.169.254/... and
    // read cloud-metadata the same way web_fetch would. about:blank
    // carries no network request, so it's exempt.
    if url != "about:blank" {
        crate::tools::web::check_url_safe(url)
            .map_err(|e| format!("browser_open blocked: {e}"))?;
    }

    info!("[browser] Opening: {}", url);
    // Force a fresh session for every browser_open: kill the previous chromium,
    // wipe its profile dir, drop the BROWSER global. The follow-up
    // ensure_browser() then runs launch_browser() cleanly without ever entering
    // the "Chromium exited, relaunching" recovery branch.
    teardown_browser().await;
    ensure_browser().await?;

    // Navigate
    let nav_resp = cdp_command("", "Page.navigate", json!({"url": url})).await?;

    // Check for navigation errors (DNS failure, cert errors, etc.)
    if let Some(error_text) = nav_resp.get("result")
        .and_then(|r| r.get("errorText"))
        .and_then(|v| v.as_str())
    {
        if !error_text.is_empty() {
            return Err(format!("Navigation failed: {}", error_text));
        }
    }

    // Wait for page load event (max 15s), not a hardcoded sleep
    wait_for_page_load(15).await;
    // Small extra delay for JS-heavy pages to finish rendering
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get page content
    let result = cdp_command("", "Runtime.evaluate", json!({
        "expression": "document.title + '\\n\\n' + document.body.innerText.substring(0, 8000)"
    })).await?;

    let text = extract_js_value(&result).unwrap_or_else(|_| "(empty page)".to_string());
    Ok(text)
}

/// Explicitly tear down the browser session. Returns OK with the dir that was
/// cleaned, or "no session" if nothing was running. Useful for agents that
/// want to free resources immediately after a workflow completes instead of
/// waiting for the next browser_open.
pub async fn browser_close(_agent_id: &str) -> Result<String, String> {
    let had_session = { BROWSER.lock().await.is_some() };
    if !had_session {
        return Ok("no session".to_string());
    }
    teardown_browser().await;
    Ok("session torn down".to_string())
}

/// Fill a form field — smart selector: tries CSS, name, placeholder, aria-label, label text
pub async fn browser_fill(_agent_id: &str, selector: &str, value: &str) -> Result<String, String> {
    info!("[browser] Fill: {} = {}...", selector, &value[..value.len().min(20)]);
    ensure_browser().await?;

    let sel = selector.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"");
    let val = value.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"").replace('\n', "\\n");

    let js = format!(
        concat!(
            "(() => {{ ",
            "let el = document.querySelector('{sel}'); ",
            "if (!el) el = document.querySelector('input[name=\"{sel}\"]'); ",
            "if (!el) el = document.querySelector('select[name=\"{sel}\"]'); ",
            "if (!el) el = document.querySelector('textarea[name=\"{sel}\"]'); ",
            "if (!el) el = document.querySelector('input[aria-label*=\"{sel}\" i], select[aria-label*=\"{sel}\" i], textarea[aria-label*=\"{sel}\" i]'); ",
            "if (!el) {{ const all = document.querySelectorAll('input,textarea'); for (const i of all) {{ if (i.placeholder && i.placeholder.toLowerCase().includes('{sel}'.toLowerCase())) {{ el = i; break; }} }} }} ",
            "if (!el) {{ const labels = document.querySelectorAll('label'); for (const l of labels) {{ if (l.textContent.toLowerCase().includes('{sel}'.toLowerCase())) {{ el = l.querySelector('input,textarea,select') || document.getElementById(l.htmlFor); if (el) break; }} }} }} ",
            "if (!el) return 'NOT FOUND'; ",
            "el.focus(); ",
            "if (el.tagName === 'SELECT') {{ ",
            "  let found = false; ",
            "  for (const opt of el.options) {{ ",
            "    if (opt.value === '{val}' || opt.textContent.trim().toLowerCase() === '{val}'.toLowerCase()) {{ ",
            "      el.value = opt.value; found = true; break; ",
            "    }} ",
            "  }} ",
            "  if (!found) {{ ",
            "    for (const opt of el.options) {{ ",
            "      if (opt.textContent.trim().toLowerCase().includes('{val}'.toLowerCase())) {{ ",
            "        el.value = opt.value; found = true; break; ",
            "      }} ",
            "    }} ",
            "  }} ",
            "  if (!found) return 'VALUE NOT FOUND in select options'; ",
            "  const ev = new Event('change', {{bubbles: true}}); ",
            "  el.dispatchEvent(ev); ",
            "  const reactKey = Object.keys(el).find(k => k.startsWith('__reactProps$') || k.startsWith('__reactEvents$')); ",
            "  if (reactKey && el[reactKey] && el[reactKey].onChange) {{ el[reactKey].onChange({{target: el}}); }} ",
            "  return 'OK (select): ' + (el.name || el.id) + ' = ' + el.value; ",
            "}} ",
            "try {{ const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; s.call(el, '{val}'); }} catch(e) {{ el.value = '{val}'; }} ",
            "el.dispatchEvent(new Event('input', {{bubbles: true}})); ",
            "el.dispatchEvent(new Event('change', {{bubbles: true}})); ",
            "const reactKey = Object.keys(el).find(k => k.startsWith('__reactProps$') || k.startsWith('__reactEvents$')); ",
            "if (reactKey && el[reactKey] && el[reactKey].onChange) {{ el[reactKey].onChange({{target: el}}); }} ",
            "return 'OK: ' + (el.name || el.placeholder || el.id || el.tagName); ",
            "}})()"
        ),
        sel = sel, val = val
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    let r = extract_js_value(&result)?;

    if r == "NOT FOUND" {
        Err(format!("Element '{}' not found — try browser_find_inputs to see available fields", selector))
    } else if r.starts_with("VALUE NOT FOUND") {
        Err(format!("Value '{}' not found in select '{}' — try browser_find_inputs to see options", value, selector))
    } else {
        Ok(r)
    }
}

/// Set a <select> dropdown by value or visible text
pub async fn browser_select(_agent_id: &str, selector: &str, value: &str) -> Result<String, String> {
    info!("[browser] Select: {} = {}", selector, value);
    ensure_browser().await?;

    let sel = selector.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"");
    let val = value.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"");

    let js = format!(
        concat!(
            "(() => {{ ",
            "let el = document.querySelector('{sel}'); ",
            "if (!el) el = document.querySelector('select[name=\"{sel}\"]'); ",
            "if (!el) el = document.querySelector('select[id=\"{sel}\"]'); ",
            "if (!el) el = document.querySelector('select[aria-label*=\"{sel}\" i]'); ",
            "if (!el) return 'SELECT NOT FOUND'; ",
            "if (el.tagName !== 'SELECT') return 'NOT A SELECT: ' + el.tagName; ",
            "const opts = Array.from(el.options).map(o => o.value + '=' + o.textContent.trim()); ",
            "let found = false; ",
            "for (const opt of el.options) {{ ",
            "  if (opt.value === '{val}' || opt.textContent.trim() === '{val}') {{ ",
            "    el.value = opt.value; found = true; break; ",
            "  }} ",
            "}} ",
            "if (!found) {{ for (const opt of el.options) {{ ",
            "  if (opt.value.toLowerCase() === '{val}'.toLowerCase() || opt.textContent.trim().toLowerCase() === '{val}'.toLowerCase()) {{ ",
            "    el.value = opt.value; found = true; break; ",
            "  }} ",
            "}} }} ",
            "if (!found) {{ for (const opt of el.options) {{ ",
            "  if (opt.textContent.trim().toLowerCase().includes('{val}'.toLowerCase())) {{ ",
            "    el.value = opt.value; found = true; break; ",
            "  }} ",
            "}} }} ",
            "if (!found) {{ for (const opt of el.options) {{ ",
            "  if (opt.value === '{val}' || opt.value === String(parseInt('{val}'))) {{ ",
            "    el.value = opt.value; found = true; break; ",
            "  }} ",
            "}} }} ",
            "if (!found) return 'VALUE NOT FOUND. Options: ' + opts.join(', '); ",
            "el.dispatchEvent(new Event('change', {{bubbles: true}})); ",
            "el.dispatchEvent(new Event('input', {{bubbles: true}})); ",
            "const nativeSet = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value').set; ",
            "if (nativeSet) {{ try {{ nativeSet.call(el, el.value); }} catch(e) {{}} }} ",
            "const reactKey = Object.keys(el).find(k => k.startsWith('__reactProps$') || k.startsWith('__reactEvents$')); ",
            "if (reactKey && el[reactKey] && el[reactKey].onChange) {{ el[reactKey].onChange({{target: el}}); }} ",
            "return 'OK: ' + (el.name || el.id) + ' = ' + el.value + ' (' + el.options[el.selectedIndex].textContent.trim() + ')'; ",
            "}})()"
        ),
        sel = sel, val = val
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    let r = extract_js_value(&result)?;

    if r.starts_with("SELECT NOT FOUND") || r.starts_with("NOT A SELECT") || r.starts_with("VALUE NOT FOUND") {
        Err(r)
    } else {
        Ok(r)
    }
}

/// List all form inputs on the page
pub async fn browser_find_inputs(_agent_id: &str) -> Result<String, String> {
    ensure_browser().await?;
    let js = concat!(
        "(() => { const inputs = document.querySelectorAll('input,textarea,select,button[type=submit]'); ",
        "const r = []; for (const el of inputs) { ",
        "if (el.type === 'hidden') continue; ",
        "const entry = { tag: el.tagName, type: el.type||'', name: el.name||'', id: el.id||'', ",
        "placeholder: el.placeholder||'', ariaLabel: el.getAttribute('aria-label')||'', ",
        "visible: el.offsetParent !== null }; ",
        "if (el.tagName === 'SELECT') { ",
        "  entry.options = Array.from(el.options).slice(0, 30).map(o => ({ value: o.value, text: o.textContent.trim() })); ",
        "  entry.selectedValue = el.value; ",
        "} ",
        "r.push(entry); } ",
        "return JSON.stringify(r); })()"
    );
    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    extract_js_value(&result)
}

/// Fill an entire form in one call
pub async fn browser_fill_form(_agent_id: &str, fields: &serde_json::Value) -> Result<String, String> {
    info!("[browser] Fill form: {} fields", fields.as_object().map_or(0, |o| o.len()));
    ensure_browser().await?;

    let fields_json = serde_json::to_string(fields).unwrap_or_default()
        .replace('\\', "\\\\")
        .replace('\'', "\\'");

    let js = format!(
        concat!(
            "(() => {{ ",
            "const fields = JSON.parse('{fields_json}'); ",
            "const allInputs = document.querySelectorAll('input,textarea,select'); ",
            "const results = []; ",
            "const index = []; ",
            "for (const el of allInputs) {{ ",
            "  if (el.type === 'hidden') continue; ",
            "  const label = el.closest('label')?.textContent?.trim() || ''; ",
            "  const forLabel = el.id ? document.querySelector('label[for=\"' + el.id + '\"]')?.textContent?.trim() || '' : ''; ",
            "  let nearbyText = ''; ",
            "  let p = el.parentElement; ",
            "  for (let i = 0; i < 5 && p; i++) {{ ",
            "    const spans = p.querySelectorAll('span, label, div, h1, h2, h3, p'); ",
            "    for (const s of spans) {{ ",
            "      const t = s.textContent.trim(); ",
            "      if (t.length > 0 && t.length < 40 && !s.querySelector('input,textarea,select')) {{ ",
            "        nearbyText += ' ' + t; ",
            "      }} ",
            "    }} ",
            "    if (nearbyText.trim().length > 0) break; ",
            "    p = p.parentElement; ",
            "  }} ",
            "  index.push({{ el, name: (el.name||'').toLowerCase(), id: (el.id||'').toLowerCase(), ",
            "    type: (el.type||'').toLowerCase(), placeholder: (el.placeholder||'').toLowerCase(), ",
            "    ariaLabel: (el.getAttribute('aria-label')||'').toLowerCase(), ",
            "    label: (label + ' ' + forLabel).toLowerCase(), ",
            "    nearby: nearbyText.toLowerCase(), tag: el.tagName }}); ",
            "}} ",
            "for (const [key, val] of Object.entries(fields)) {{ ",
            "  const k = key.toLowerCase(); ",
            "  let match = null; ",
            "  let matchScore = 0; ",
            "  for (const f of index) {{ ",
            "    if (f.el._used) continue; ",
            "    let score = 0; ",
            "    if (f.name === k) score = 100; ",
            "    else if (k === 'email' && f.type === 'email') score = 90; ",
            "    else if (k === 'password' && f.type === 'password') score = 90; ",
            "    else if (f.name.includes(k)) score = 80; ",
            "    else if (k.includes(f.name) && f.name.length > 1) score = 75; ",
            "    else if (f.placeholder.includes(k)) score = 70; ",
            "    else if (f.ariaLabel.includes(k)) score = 70; ",
            "    else if (f.label.includes(k)) score = 65; ",
            "    else if (f.nearby.includes(k)) score = 60; ",
            "    else {{ ",
            "      const words = k.split(/[\\s_-]+/); ",
            "      const all = f.name + ' ' + f.placeholder + ' ' + f.ariaLabel + ' ' + f.label + ' ' + f.id + ' ' + f.nearby; ",
            "      let hits = 0; ",
            "      for (const w of words) {{ if (w.length > 1 && all.includes(w)) hits++; }} ",
            "      if (hits > 0) score = 40 + (hits / words.length) * 30; ",
            "    }} ",
            "    if (score > matchScore) {{ match = f; matchScore = score; }} ",
            "  }} ",
            "  if (!match || matchScore < 30) {{ ",
            "    results.push('MISS: ' + key + ' (no matching field found)'); ",
            "    continue; ",
            "  }} ",
            "  match.el._used = true; ",
            "  const el = match.el; ",
            "  const strVal = String(val); ",
            "  if (el.tagName === 'SELECT') {{ ",
            "    let found = false; ",
            "    for (const opt of el.options) {{ ",
            "      if (opt.value === strVal || opt.textContent.trim().toLowerCase() === strVal.toLowerCase() ",
            "          || opt.value === String(parseInt(strVal)) ",
            "          || opt.textContent.trim().toLowerCase().includes(strVal.toLowerCase())) {{ ",
            "        el.value = opt.value; found = true; break; ",
            "      }} ",
            "    }} ",
            "    if (!found) {{ ",
            "      const opts = Array.from(el.options).slice(0,10).map(o => o.value + '=' + o.textContent.trim()).join(', '); ",
            "      results.push('MISS: ' + key + ' → select has no option matching \"' + strVal + '\". Options: ' + opts); ",
            "      continue; ",
            "    }} ",
            "    el.dispatchEvent(new Event('change', {{bubbles: true}})); ",
            "    try {{ ",
            "      const rk = Object.keys(el).find(k => k.startsWith('__reactProps$')); ",
            "      if (rk && el[rk] && el[rk].onChange) el[rk].onChange({{target: el}}); ",
            "    }} catch(e) {{}} ",
            "    results.push('OK: ' + key + ' → ' + (el.name||el.id) + ' = ' + el.options[el.selectedIndex].textContent.trim()); ",
            "  }} else {{ ",
            "    el.focus(); ",
            "    try {{ ",
            "      const setter = el.tagName === 'TEXTAREA' ",
            "        ? Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set ",
            "        : Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; ",
            "      setter.call(el, strVal); ",
            "    }} catch(e) {{ el.value = strVal; }} ",
            "    el.dispatchEvent(new Event('input', {{bubbles: true}})); ",
            "    el.dispatchEvent(new Event('change', {{bubbles: true}})); ",
            "    try {{ ",
            "      const rk = Object.keys(el).find(k => k.startsWith('__reactProps$')); ",
            "      if (rk && el[rk] && el[rk].onChange) el[rk].onChange({{target: el}}); ",
            "    }} catch(e) {{}} ",
            "    results.push('OK: ' + key + ' → ' + (el.name||el.placeholder||el.id||el.tagName)); ",
            "  }} ",
            "}} ",
            "return results.join('\\n'); ",
            "}})()"
        ),
        fields_json = fields_json
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    extract_js_value(&result)
}

/// Set a custom (non-native) dropdown
pub async fn browser_set_dropdown(_agent_id: &str, label: &str, value: &str) -> Result<String, String> {
    info!("[browser] Set dropdown: {} = {}", label, value);
    ensure_browser().await?;

    let lbl = label.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"");
    let val = value.replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"");

    // Phase 1: Find and click the trigger
    let trigger_js = format!(
        concat!(
            "(() => {{ try {{ ",
            "const label = '{lbl}'; ",
            "let trigger = null; ",
            "const comboboxes = document.querySelectorAll('[role=\"combobox\"]'); ",
            "for (const cb of comboboxes) {{ ",
            "  const al = (cb.getAttribute('aria-label') || '').toLowerCase(); ",
            "  const txt = cb.textContent.trim().toLowerCase(); ",
            "  if (al.includes(label.toLowerCase()) || txt === label.toLowerCase() || txt.includes(label.toLowerCase())) {{ ",
            "    trigger = cb; break; ",
            "  }} ",
            "}} ",
            "if (!trigger) {{ ",
            "  const divs = document.querySelectorAll('div, span'); ",
            "  for (const el of divs) {{ ",
            "    if (el.textContent.trim().toLowerCase() === label.toLowerCase() && el.childElementCount <= 2) {{ ",
            "      if (!el.closest('[role=\"listbox\"]')) {{ trigger = el; break; }} ",
            "    }} ",
            "  }} ",
            "}} ",
            "if (!trigger) {{ ",
            "  const selects = document.querySelectorAll('select'); ",
            "  for (const s of selects) {{ ",
            "    const al = (s.getAttribute('aria-label') || '').toLowerCase(); ",
            "    const nm = (s.name || '').toLowerCase(); ",
            "    if (al.includes(label.toLowerCase()) || nm.includes(label.toLowerCase())) {{ ",
            "      return 'NATIVE_SELECT:' + Array.from(s.options).findIndex(o => o.textContent.trim().toLowerCase() === '{val}'.toLowerCase()); ",
            "    }} ",
            "  }} ",
            "}} ",
            "if (!trigger) return 'TRIGGER NOT FOUND for: ' + label; ",
            "trigger.scrollIntoView({{block: 'center'}}); ",
            "const rect = trigger.getBoundingClientRect(); ",
            "const controls = trigger.getAttribute('aria-controls') || ''; ",
            "return 'TRIGGER_AT:' + Math.round(rect.x + rect.width/2) + ',' + Math.round(rect.y + rect.height/2) + ':' + controls; ",
            "}} catch(e) {{ return 'JS ERROR: ' + e.message; }} ",
            "}})()"
        ),
        lbl = lbl, val = val
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": trigger_js})).await?;
    let r = extract_js_value(&result)?;

    if r.starts_with("TRIGGER NOT FOUND") || r.starts_with("JS ERROR") {
        return Err(r);
    }

    let mut controls_id = String::new();

    if r.starts_with("TRIGGER_AT:") {
        let parts: Vec<&str> = r.trim_start_matches("TRIGGER_AT:").splitn(2, ':').collect();
        let coords: Vec<f64> = parts[0].split(',').filter_map(|s| s.parse().ok()).collect();
        if parts.len() > 1 { controls_id = parts[1].to_string(); }

        if coords.len() == 2 && (coords[0] > 0.0 || coords[1] > 0.0) {
            cdp_command("", "Input.dispatchMouseEvent", json!({
                "type": "mousePressed", "x": coords[0], "y": coords[1],
                "button": "left", "clickCount": 1
            })).await.ok();
            tokio::time::sleep(Duration::from_millis(50)).await;
            cdp_command("", "Input.dispatchMouseEvent", json!({
                "type": "mouseReleased", "x": coords[0], "y": coords[1],
                "button": "left", "clickCount": 1
            })).await.ok();
        }
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Phase 2: Find and click the option
    let controls_escaped = controls_id.replace('\'', "\\'");
    let option_js = format!(
        concat!(
            "(() => {{ try {{ ",
            "const target = '{val}'; ",
            "const controlsId = '{controls}'; ",
            "const listboxes = document.querySelectorAll('[role=\"listbox\"]'); ",
            "let targetLb = null; ",
            "if (controlsId) targetLb = document.getElementById(controlsId); ",
            "if (!targetLb) {{ ",
            "  const comboboxes = document.querySelectorAll('[role=\"combobox\"]'); ",
            "  const triggerCb = Array.from(comboboxes).find(cb => {{ ",
            "    const txt = cb.textContent.trim().toLowerCase(); ",
            "    const al = (cb.getAttribute('aria-label') || '').toLowerCase(); ",
            "    return al.includes(target.substring(0,3).toLowerCase()) || txt.includes(target.substring(0,3).toLowerCase()); ",
            "  }}); ",
            "  if (triggerCb) {{ ",
            "    let sib = triggerCb.nextElementSibling; ",
            "    while (sib) {{ ",
            "      if (sib.getAttribute && sib.getAttribute('role') === 'listbox') {{ targetLb = sib; break; }} ",
            "      sib = sib.nextElementSibling; ",
            "    }} ",
            "    if (!targetLb) {{ ",
            "      const parent = triggerCb.parentElement; ",
            "      if (parent) {{ ",
            "        const lb = parent.querySelector('[role=\"listbox\"]'); ",
            "        if (lb) targetLb = lb; ",
            "      }} ",
            "    }} ",
            "  }} ",
            "}} ",
            "const searchList = targetLb ? [targetLb] : Array.from(listboxes); ",
            "let match = null; ",
            "for (const lb of searchList) {{ ",
            "  const opts = lb.querySelectorAll('[role=\"option\"]'); ",
            "  const allOpts = opts.length > 0 ? opts : lb.children; ",
            "  for (const opt of allOpts) {{ ",
            "    const t = opt.textContent.trim(); ",
            "    if (t === target || t.toLowerCase() === target.toLowerCase()) {{ ",
            "      match = opt; break; ",
            "    }} ",
            "  }} ",
            "  if (match) break; ",
            "  const numTarget = parseInt(target); ",
            "  if (!isNaN(numTarget)) {{ ",
            "    for (const opt of allOpts) {{ ",
            "      if (opt.textContent.trim() === String(numTarget)) {{ match = opt; break; }} ",
            "    }} ",
            "  }} ",
            "  if (match) break; ",
            "  if (target.length >= 3) {{ ",
            "    for (const opt of allOpts) {{ ",
            "      if (opt.textContent.trim().toLowerCase().includes(target.toLowerCase())) {{ ",
            "        match = opt; break; ",
            "      }} ",
            "    }} ",
            "  }} ",
            "  if (match) break; ",
            "}} ",
            "if (!match) {{ ",
            "  const avail = targetLb ? Array.from(targetLb.children).slice(0,15).map(c=>c.textContent.trim()) : ['no target listbox']; ",
            "  return 'OPTION NOT FOUND: \"' + target + '\". Options: ' + avail.join(', '); ",
            "}} ",
            "match.scrollIntoView({{block: 'nearest'}}); ",
            "const rect = match.getBoundingClientRect(); ",
            "return 'OPTION_AT:' + Math.round(rect.x + rect.width/2) + ',' + Math.round(rect.y + rect.height/2) + ':' + match.textContent.trim(); ",
            "}} catch(e) {{ return 'JS ERROR: ' + e.message; }} ",
            "}})()"
        ),
        val = val, controls = controls_escaped
    );

    let result2 = cdp_command("", "Runtime.evaluate", json!({"expression": option_js})).await?;
    let r2 = extract_js_value(&result2)?;

    if r2.starts_with("OPTION NOT FOUND") || r2.starts_with("JS ERROR") {
        return Err(r2);
    }

    if r2.starts_with("OPTION_AT:") {
        let rest = r2.trim_start_matches("OPTION_AT:");
        let colon_pos = rest.find(':').unwrap_or(rest.len());
        let coord_part = &rest[..colon_pos];
        let option_text = &rest[colon_pos+1..];

        let coords: Vec<f64> = coord_part.split(',').filter_map(|s| s.parse().ok()).collect();
        if coords.len() == 2 {
            cdp_command("", "Input.dispatchMouseEvent", json!({
                "type": "mousePressed", "x": coords[0], "y": coords[1],
                "button": "left", "clickCount": 1
            })).await.ok();
            tokio::time::sleep(Duration::from_millis(50)).await;
            cdp_command("", "Input.dispatchMouseEvent", json!({
                "type": "mouseReleased", "x": coords[0], "y": coords[1],
                "button": "left", "clickCount": 1
            })).await.ok();
            tokio::time::sleep(Duration::from_millis(500)).await;
            return Ok(format!("OK: {} = {}", label, option_text));
        }
    }

    Err(format!("Unexpected dropdown result: {}", r2))
}

/// Compound: navigate + fill + dropdowns + submit in ONE call
pub async fn browser_open_and_fill(
    _agent_id: &str,
    url: &str,
    fields: &serde_json::Value,
    dropdowns: &serde_json::Value,
    submit: bool,
) -> Result<String, String> {
    // Same SSRF guard as browser_open. This compound entrypoint is
    // reachable by any agent with the browser skill and would otherwise
    // bypass the check.
    if url != "about:blank" {
        crate::tools::web::check_url_safe(url)
            .map_err(|e| format!("browser_open_and_fill blocked: {e}"))?;
    }

    info!("[browser] Compound: open {} + fill {} fields + {} dropdowns, submit={}",
        url, fields.as_object().map_or(0, |o| o.len()),
        dropdowns.as_object().map_or(0, |o| o.len()), submit);

    let mut report = Vec::new();
    let mut errors = 0;

    // Step 1: Navigate
    browser_open("", url).await?;
    report.push(format!("Opened: {}", url));

    // Step 2: Fill text fields
    if let Some(obj) = fields.as_object() {
        if !obj.is_empty() {
            let result = browser_fill_form("", fields).await?;
            // Count MISS entries
            for line in result.lines() {
                if line.starts_with("MISS:") { errors += 1; }
            }
            report.push(format!("Form: {}", result));
        }
    }

    // Step 3: Set dropdowns
    if let Some(obj) = dropdowns.as_object() {
        for (label, value) in obj {
            let val = value.as_str().unwrap_or("");
            match browser_set_dropdown("", label, val).await {
                Ok(r) => report.push(format!("Dropdown {}: {}", label, r)),
                Err(e) => {
                    errors += 1;
                    report.push(format!("Dropdown {} FAILED: {}", label, e));
                }
            }
        }
    }

    // Step 4: Submit
    if submit {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let submit_js = concat!(
            "(() => { ",
            "const btns = document.querySelectorAll('button, [role=button], input[type=submit]'); ",
            "for (const b of btns) { ",
            "  const t = b.textContent.trim().toLowerCase(); ",
            "  if (t.includes('sign up') || t.includes('submit') || t.includes('next') ",
            "    || t.includes('continue') || t.includes('create') || b.type === 'submit' || b.name === 'websubmit') { ",
            "    b.scrollIntoView({block:'center'}); ",
            "    const r = b.getBoundingClientRect(); ",
            "    return 'BTN:' + Math.round(r.x + r.width/2) + ',' + Math.round(r.y + r.height/2) + ':' + b.textContent.trim(); ",
            "  } ",
            "} return 'NO SUBMIT BUTTON'; })()"
        );
        let btn_result = cdp_command("", "Runtime.evaluate", json!({"expression": submit_js})).await
            .ok().and_then(|r| extract_js_value(&r).ok())
            .unwrap_or_default();

        if btn_result.starts_with("BTN:") {
            let rest = btn_result.trim_start_matches("BTN:");
            let colon_pos = rest.find(':').unwrap_or(rest.len());
            let coords: Vec<f64> = rest[..colon_pos].split(',').filter_map(|s| s.parse().ok()).collect();
            if coords.len() == 2 {
                browser_click_at("", coords[0], coords[1]).await?;
                let btn_name = &rest[colon_pos+1..];
                report.push(format!("Submitted: clicked '{}'", btn_name));
            }
        } else {
            errors += 1;
            report.push(format!("Submit: {}", btn_result));
        }

        // Wait for navigation after submit
        wait_for_page_load(10).await;
    }

    // Step 5: Page summary
    let brief = browser_read_brief("").await.unwrap_or_default();
    report.push(format!("Page after: {}", brief));

    let full_report = report.join("\n");

    // Return Err if critical fields failed
    if errors > 0 {
        Err(format!("Completed with {} error(s):\n{}", errors, full_report))
    } else {
        Ok(full_report)
    }
}

/// Click an element
pub async fn browser_click(_agent_id: &str, selector: &str) -> Result<String, String> {
    info!("[browser] Click: {}", selector);
    ensure_browser().await?;

    let js = format!(
        "(() => {{ const el = document.querySelector('{}'); if (!el) return 'NOT FOUND'; el.click(); return 'OK'; }})()",
        selector.replace('\'', "\\'")
    );

    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    let val = extract_js_value(&result)?;

    tokio::time::sleep(Duration::from_millis(500)).await;

    if val == "NOT FOUND" {
        Err(format!("Element '{}' not found", selector))
    } else {
        Ok(format!("Clicked '{}'", selector))
    }
}

/// Take screenshot
pub async fn browser_screenshot(_agent_id: &str, workspace: &std::path::Path) -> Result<String, String> {
    info!("[browser] Screenshot");
    ensure_browser().await?;

    let result = cdp_command("", "Page.captureScreenshot", json!({"format": "png"})).await?;
    let data = result.get("result").and_then(|r| r.get("data"))
        .and_then(|v| v.as_str())
        .ok_or("No screenshot data")?;

    let bytes = base64_decode(data)?;
    let filename = format!("screenshot-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    let path = workspace.join(&filename);
    std::fs::write(&path, &bytes).map_err(|e| format!("Save error: {}", e))?;

    Ok(format!("Screenshot saved: {}", filename))
}

/// Read current page text
pub async fn browser_read(_agent_id: &str) -> Result<String, String> {
    ensure_browser().await?;
    let result = cdp_command("", "Runtime.evaluate", json!({
        "expression": "document.body.innerText.substring(0, 2000)"
    })).await?;
    extract_js_value(&result)
}

/// Quick page summary
pub async fn browser_read_brief(_agent_id: &str) -> Result<String, String> {
    ensure_browser().await?;
    let js = concat!(
        "(() => { ",
        "const t = document.title; ",
        "const u = location.href.substring(0, 80); ",
        "const h = Array.from(document.querySelectorAll('h1,h2,h3')).slice(0,3).map(e=>e.textContent.trim()).join(', '); ",
        "const inputs = document.querySelectorAll('input,textarea,select').length; ",
        "const btns = Array.from(document.querySelectorAll('button,[role=button]')).filter(b=>b.offsetParent!==null).slice(0,5).map(b=>b.textContent.trim().substring(0,20)).join(', '); ",
        "const err = document.querySelector('[role=alert],[class*=error],[class*=Error]')?.textContent?.trim()?.substring(0,80) || ''; ",
        "return 'Title: ' + t + '\\nURL: ' + u + '\\nHeadings: ' + (h||'none') + '\\nInputs: ' + inputs + '\\nButtons: ' + (btns||'none') + (err ? '\\nError: ' + err : ''); ",
        "})()"
    );
    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    extract_js_value(&result)
}

/// Execute JavaScript
pub async fn browser_execute_js(_agent_id: &str, js: &str) -> Result<String, String> {
    info!("[browser] JS: {}...", &js[..js.len().min(50)]);
    ensure_browser().await?;
    let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
    let val = result.get("result").and_then(|r| r.get("result")).unwrap_or(&json!(null));
    // Return the value as a string if it's a string, otherwise JSON
    if let Some(s) = val.get("value").and_then(|v| v.as_str()) {
        Ok(s.to_string())
    } else {
        Ok(serde_json::to_string_pretty(val).unwrap_or_default())
    }
}

/// Wait for element
pub async fn browser_wait(_agent_id: &str, selector: &str, timeout_secs: u64) -> Result<String, String> {
    ensure_browser().await?;
    let timeout = Duration::from_secs(timeout_secs.max(1).min(30));
    let start = std::time::Instant::now();

    loop {
        let js = format!(
            "document.querySelector('{}') !== null",
            selector.replace('\'', "\\'")
        );
        let result = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await?;
        let found = result.get("result").and_then(|r| r.get("result")).and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool()).unwrap_or(false);

        if found {
            return Ok(format!("Element '{}' found", selector));
        }
        if start.elapsed() > timeout {
            return Err(format!("Element '{}' not found within {}s", selector, timeout_secs));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Click at exact screen coordinates
pub async fn browser_click_at(_agent_id: &str, x: f64, y: f64) -> Result<String, String> {
    ensure_browser().await?;

    if let Some(display) = xdotool_available().await {
        let (off_x, off_y) = get_viewport_offset().await;
        let ix = (x + off_x) as i32;
        let iy = (y + off_y) as i32;
        info!("[browser] xdotool click at ({}, {}) [offset +{},+{}]", ix, iy, off_x, off_y);
        xdotool_cmd(&display, &["mousemove", "--", &ix.to_string(), &iy.to_string()]).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        xdotool_cmd(&display, &["click", "1"]).await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        return Ok(format!("xdotool clicked at ({}, {})", ix, iy));
    }

    info!("[browser] CDP click at ({}, {})", x, y);
    cdp_command("", "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
    })).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    cdp_command("", "Input.dispatchMouseEvent", json!({
        "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
    })).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(format!("Clicked at ({}, {})", x, y))
}

async fn xdotool_available() -> Option<String> {
    let guard = BROWSER.lock().await;
    guard.as_ref().and_then(|inst| inst.display.clone())
}

async fn get_viewport_offset() -> (f64, f64) {
    let js = "JSON.stringify({offsetX: window.screenX || 0, offsetY: (window.outerHeight - window.innerHeight) + (window.screenY || 0)})";
    if let Ok(result) = cdp_command("", "Runtime.evaluate", json!({"expression": js})).await {
        if let Ok(val) = extract_js_value(&result) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&val) {
                let x = parsed.get("offsetX").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = parsed.get("offsetY").and_then(|v| v.as_f64()).unwrap_or(0.0);
                return (x, y);
            }
        }
    }
    (0.0, 0.0)
}

async fn xdotool_cmd(display: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("xdotool")
        .args(args)
        .env("DISPLAY", display)
        .output()
        .await
        .map_err(|e| format!("xdotool error: {}", e))?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Press and hold at coordinates
pub async fn browser_hold_at(_agent_id: &str, x: f64, y: f64, duration_secs: u64) -> Result<String, String> {
    let duration = duration_secs.max(1).min(15);
    ensure_browser().await?;

    if let Some(display) = xdotool_available().await {
        return xdotool_hold(&display, x, y, duration).await;
    }

    info!("[browser] CDP press-and-hold at ({}, {}) for {}s", x, y, duration);

    // Realistic mouse movement to target
    let start_x = x - 200.0 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as f64 % 150.0);
    let start_y = y - 100.0 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as f64 % 80.0);

    let steps = 12;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let t_smooth = t * t * (3.0 - 2.0 * t);
        let mx = start_x + (x - start_x) * t_smooth;
        let my = start_y + (y - start_y) * t_smooth;
        cdp_command("", "Input.dispatchMouseEvent", json!({
            "type": "mouseMoved", "x": mx, "y": my
        })).await.ok();
        tokio::time::sleep(Duration::from_millis(15 + (i as u64 * 3))).await;
    }

    tokio::time::sleep(Duration::from_millis(80)).await;

    cdp_command("", "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
    })).await?;

    let hold_ms = duration * 1000;
    let micro_steps = (hold_ms / 200) as usize;
    for _ in 0..micro_steps {
        tokio::time::sleep(Duration::from_millis(180 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as u64 % 40))).await;
        let jitter_x = x + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as f64 % 3.0) - 1.5;
        let jitter_y = y + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as f64 % 3.0) - 1.5;
        cdp_command("", "Input.dispatchMouseEvent", json!({
            "type": "mouseMoved", "x": jitter_x, "y": jitter_y
        })).await.ok();
    }

    cdp_command("", "Input.dispatchMouseEvent", json!({
        "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
    })).await?;

    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(format!("Held at ({}, {}) for {}s with realistic movement", x, y, duration))
}

/// Find the Chromium window ID in Xvfb for --window targeting
async fn find_chromium_window(display: &str) -> Option<String> {
    let output = Command::new("xdotool")
        .args(["search", "--class", "chromium"])
        .env("DISPLAY", display)
        .output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().last().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn xdotool_hold(display: &str, x: f64, y: f64, duration_secs: u64) -> Result<String, String> {
    let (off_x, off_y) = get_viewport_offset().await;
    let ix = (x + off_x) as i32;
    let iy = (y + off_y) as i32;
    let dur = duration_secs.max(1).min(15);

    // Find and focus the Chromium window for reliable event delivery
    let win_id = find_chromium_window(display).await;
    let win_args: Vec<String> = match &win_id {
        Some(id) => {
            // Focus the window first
            xdotool_cmd(display, &["windowfocus", id]).await.ok();
            tokio::time::sleep(Duration::from_millis(100)).await;
            vec!["--window".to_string(), id.clone()]
        }
        None => vec![],
    };

    info!("[browser] xdotool hold at ({}, {}) for {}s on {} (window={:?})", ix, iy, dur, display, win_id);

    // Natural pre-movement: start from a random nearby position
    let start_x = (ix - 150 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as i32 % 100)).max(10);
    let start_y = (iy - 60 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as i32 % 40)).max(10);

    let mut move_args = vec!["mousemove"];
    let win_refs: Vec<&str> = win_args.iter().map(|s| s.as_str()).collect();
    move_args.extend_from_slice(&win_refs);
    move_args.extend_from_slice(&["--", &start_x.to_string(), &start_y.to_string()]);
    // Can't borrow temporary strings, build differently
    let sx = start_x.to_string();
    let sy = start_y.to_string();
    let mut args1 = vec!["mousemove"];
    for w in &win_refs { args1.push(w); }
    args1.extend_from_slice(&["--", &sx, &sy]);
    xdotool_cmd(display, &args1).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Move to target in steps (smooth approach)
    let steps = 5;
    for s in 1..=steps {
        let t = s as f64 / steps as f64;
        let mx = start_x as f64 + (ix as f64 - start_x as f64) * t;
        let my = start_y as f64 + (iy as f64 - start_y as f64) * t;
        let mxs = (mx as i32).to_string();
        let mys = (my as i32).to_string();
        let mut args = vec!["mousemove"];
        for w in &win_refs { args.push(w); }
        args.extend_from_slice(&["--", &mxs, &mys]);
        xdotool_cmd(display, &args).await.ok();
        tokio::time::sleep(Duration::from_millis(40 + (s as u64 * 10))).await;
    }
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Press with window targeting
    let mut down_args = vec!["mousedown"];
    for w in &win_refs { down_args.push(w); }
    down_args.push("1");
    xdotool_cmd(display, &down_args).await?;

    // Hold with micro-jitter
    let jitter_steps = dur * 5;
    for _ in 0..jitter_steps {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let jx = ix + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as i32 % 3) - 1;
        let jy = iy + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis() as i32 % 3) - 1;
        let jxs = jx.to_string();
        let jys = jy.to_string();
        let mut args = vec!["mousemove"];
        for w in &win_refs { args.push(w); }
        args.extend_from_slice(&["--", &jxs, &jys]);
        xdotool_cmd(display, &args).await.ok();
    }

    // Release
    let mut up_args = vec!["mouseup"];
    for w in &win_refs { up_args.push(w); }
    up_args.push("1");
    xdotool_cmd(display, &up_args).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(format!("xdotool hold at ({}, {}) for {}s — window targeted", ix, iy, dur))
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    let input = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let mut buf = [0u8; 4];
        let mut count = 0;
        while count < 4 && i < input.len() {
            let b = input[i];
            i += 1;
            if b == b'=' || b == b'\n' || b == b'\r' { continue; }
            buf[count] = CHARS.iter().position(|&c| c == b).unwrap_or(0) as u8;
            count += 1;
        }
        if count >= 2 { result.push((buf[0] << 2) | (buf[1] >> 4)); }
        if count >= 3 { result.push((buf[1] << 4) | (buf[2] >> 2)); }
        if count >= 4 { result.push((buf[2] << 6) | buf[3]); }
    }
    Ok(result)
}
