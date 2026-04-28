//! Syntaur Dashboard Viewer — native webview, no browser needed.
//!
//! Linux: GTK4 + WebKitGTK 6.0 — native Wayland & X11, fractional scaling.
//! macOS: WKWebView via wry+tao.
//! Windows: WebView2 via wry+tao.

const DEFAULT_URL: &str = "http://localhost:18789";
const WINDOW_TITLE: &str = "Syntaur";
const DEFAULT_W: i32 = 1200;
const DEFAULT_H: i32 = 800;

/// Saved window geometry persisted across launches. Position is `Option`
/// because Wayland (Linux default) doesn't let clients restore their own
/// position — only the compositor can decide. On macOS/Windows the tao
/// path honors it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
struct WindowState {
    width: i32,
    height: i32,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { width: DEFAULT_W, height: DEFAULT_H, x: None, y: None, maximized: false }
    }
}

impl WindowState {
    /// Clamp obviously broken values (someone hand-edited the file, or
    /// monitors got unplugged since last run). Keeps width/height in a
    /// sensible range; positions stay opaque to us — the OS handles
    /// off-screen recovery well enough on macOS/Windows.
    fn sanitized(self) -> Self {
        let clamp = |v: i32, min: i32, max: i32| v.max(min).min(max);
        Self {
            width: clamp(self.width, 480, 10_000),
            height: clamp(self.height, 320, 10_000),
            x: self.x,
            y: self.y,
            maximized: self.maximized,
        }
    }
}

/// Resolve the per-user state file path. Linux honors XDG_CONFIG_HOME,
/// macOS uses ~/Library/Application Support, Windows uses %APPDATA%.
/// Deliberately kept without the `dirs` crate to avoid pulling in extra
/// deps for what is 20 lines of path logic.
fn state_file_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".config")))?;
        Some(base.join("syntaur-viewer").join("window.json"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(std::path::PathBuf::from(home)
            .join("Library").join("Application Support").join("syntaur-viewer").join("window.json"))
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var("USERPROFILE").ok().map(|h| std::path::PathBuf::from(h).join("AppData").join("Roaming")))?;
        Some(base.join("syntaur-viewer").join("window.json"))
    }
}

fn load_window_state() -> WindowState {
    let Some(path) = state_file_path() else { return WindowState::default(); };
    let Ok(text) = std::fs::read_to_string(&path) else { return WindowState::default(); };
    serde_json::from_str::<WindowState>(&text)
        .map(|s| s.sanitized())
        .unwrap_or_default()
}

fn save_window_state(state: &WindowState) {
    let Some(path) = state_file_path() else { return; };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&state.sanitized()) {
        // Atomic write: tmp + rename. Prevents half-written state if the
        // app crashes mid-save.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn main() {
    // URL priority: CLI arg > env var > saved server config > default localhost
    let url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SYNTAUR_URL").ok())
        .or_else(|| read_saved_url())
        .unwrap_or_else(|| DEFAULT_URL.to_string());

    // Tier 2 remote-access onboarding. A laptop user on the road hasn't
    // joined the household tailnet yet — the target URL resolves (via
    // MagicDNS or a public domain) but TCP connect fails because the
    // device isn't a tailnet member. Instead of dropping her at a blank
    // "server unreachable" screen, load a local onboarding HTML that
    // explains Tailscale + polls reachability in the background, so the
    // dashboard loads automatically as soon as the connection comes up.
    //
    // LAN-local URLs skip the probe — the operator intentionally chose
    // local-only mode and Tailscale isn't the right guidance there.
    let load_url = if is_local_gateway(&url)
        || probe_reachable(&url, std::time::Duration::from_secs(3))
    {
        url.clone()
    } else {
        match write_onboarding_html(&url) {
            Ok(path) => format!("file://{}", path.display()),
            Err(e) => {
                eprintln!("[syntaur-viewer] onboarding page write failed: {e}; falling back to direct load");
                url.clone()
            }
        }
    };

    if let Err(e) = run_viewer(&load_url) {
        eprintln!("[syntaur-viewer] Failed: {}", e);
        std::process::exit(1);
    }
}

/// Loopback / RFC1918 / link-local hosts — operator picked LAN mode
/// deliberately, no Tailscale onboarding relevant.
fn is_local_gateway(url: &str) -> bool {
    let stripped = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split(':').next().unwrap_or("").split('/').next().unwrap_or("");
    if host == "localhost" || host == "127.0.0.1" || host == "0.0.0.0" || host == "::1" {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() { return true; }
        if let std::net::IpAddr::V4(v4) = ip {
            let o = v4.octets();
            return o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254);
        }
    }
    false
}

/// TCP-connect reachability probe. Short timeout so we don't stall first
/// launch on flaky networks; failure is treated as "show onboarding."
fn probe_reachable(url: &str, timeout: std::time::Duration) -> bool {
    let scheme_https = url.starts_with("https://");
    let stripped = url.trim_start_matches("https://").trim_start_matches("http://");
    let host_port = stripped.split('/').next().unwrap_or("");
    let (host, port) = match host_port.rfind(':') {
        Some(idx) if !host_port[..idx].contains(']') || idx > host_port.rfind(']').unwrap_or(0) => {
            let (h, p) = host_port.split_at(idx);
            let port = p[1..].parse::<u16>().ok();
            match port {
                Some(p) => (h.trim_matches(|c| c == '[' || c == ']').to_string(), p),
                None => (host_port.to_string(), if scheme_https { 443 } else { 80 }),
            }
        }
        _ => (host_port.to_string(), if scheme_https { 443 } else { 80 }),
    };
    if host.is_empty() { return false; }
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else { return false; };
    std::net::TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// Render the onboarding HTML to a temp file parameterized with the
/// target URL. Self-contained — no remote assets — so it renders even if
/// the laptop has no internet yet. Returns an absolute path; caller
/// prepends `file://`.
fn write_onboarding_html(target_url: &str) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("syntaur-viewer");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = dir.join("connect.html");
    let ts_download = if cfg!(target_os = "macos") {
        "https://tailscale.com/download/mac"
    } else if cfg!(target_os = "windows") {
        "https://tailscale.com/download/windows"
    } else {
        "https://tailscale.com/download/linux"
    };
    let display_host = target_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(target_url);
    let html = ONBOARDING_TEMPLATE
        .replace("{{TARGET_URL}}", target_url)
        .replace("{{DISPLAY_HOST}}", display_host)
        .replace("{{TS_DOWNLOAD_URL}}", ts_download);
    std::fs::write(&path, html.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

const ONBOARDING_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect to Home — Syntaur</title>
<style>
  *, *::before, *::after { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh;
    display: flex; align-items: center; justify-content: center;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif;
    background: radial-gradient(ellipse at top, #1f2937, #0b1120 70%);
    color: #e5e7eb;
  }
  .card {
    max-width: 560px; width: 92%;
    background: rgba(17,24,39,0.9);
    border: 1px solid rgba(148,163,184,0.15);
    border-radius: 18px;
    padding: 40px 44px;
    box-shadow: 0 24px 60px rgba(0,0,0,0.35);
  }
  h1 { margin: 0 0 8px; font-size: 26px; font-weight: 600; letter-spacing: -0.01em; }
  .sub { margin: 0 0 28px; font-size: 14px; color: #94a3b8; line-height: 1.55; }
  .host {
    display: inline-block; font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 13px; padding: 4px 10px; border-radius: 6px;
    background: rgba(56,189,248,0.08); color: #7dd3fc;
    border: 1px solid rgba(56,189,248,0.18);
  }
  ol.steps { list-style: none; counter-reset: s; padding: 0; margin: 8px 0 28px; }
  ol.steps li {
    counter-increment: s; position: relative;
    padding: 12px 0 12px 40px; font-size: 14px; color: #cbd5e1; line-height: 1.5;
  }
  ol.steps li::before {
    content: counter(s); position: absolute; left: 0; top: 10px;
    width: 26px; height: 26px; border-radius: 50%;
    background: rgba(148,163,184,0.1); color: #94a3b8;
    display: flex; align-items: center; justify-content: center;
    font-size: 12px; font-weight: 600;
  }
  .btn {
    display: inline-block; padding: 11px 20px; border-radius: 8px;
    background: #0284c7; color: white; text-decoration: none;
    font-weight: 500; font-size: 14px;
    transition: background 120ms;
  }
  .btn:hover { background: #0ea5e9; }
  .btn.ghost {
    background: transparent; color: #94a3b8;
    border: 1px solid rgba(148,163,184,0.25);
  }
  .btn.ghost:hover { background: rgba(148,163,184,0.06); color: #e5e7eb; }
  .row { display: flex; gap: 10px; flex-wrap: wrap; margin-top: 4px; }
  .probe-state {
    margin-top: 22px; padding: 10px 14px; border-radius: 8px;
    font-size: 13px; color: #94a3b8;
    background: rgba(148,163,184,0.06); border: 1px solid rgba(148,163,184,0.12);
    display: flex; align-items: center; gap: 10px;
  }
  .dot {
    width: 8px; height: 8px; border-radius: 50%; background: #f59e0b;
    animation: pulse 1.4s ease-in-out infinite;
  }
  @keyframes pulse { 0%,100%{opacity:0.45} 50%{opacity:1} }
  .probe-state.ok .dot { background: #34d399; animation: none; }
</style>
</head>
<body>
  <main class="card">
    <h1>Connect to Home</h1>
    <p class="sub">Your Syntaur lives on your home network. To reach it from this laptop, this device needs to join your household's Tailscale — a private encrypted connection between your own devices. Free, one-time setup, takes about a minute.</p>

    <ol class="steps">
      <li>Install Tailscale from the official download page. When it asks you to sign in, use whichever account matches the invite your household admin sent you.</li>
      <li>Once Tailscale is running in your menu bar / system tray, this window finishes loading automatically. You don't need to quit or restart.</li>
    </ol>

    <div class="row">
      <a class="btn" href="{{TS_DOWNLOAD_URL}}" target="_blank" rel="noopener">Install Tailscale →</a>
      <a class="btn ghost" href="{{TARGET_URL}}">Check again now</a>
    </div>

    <div class="probe-state" id="probe">
      <span class="dot"></span>
      <span id="probeMsg">Watching for a connection to <span class="host">{{DISPLAY_HOST}}</span>…</span>
    </div>
  </main>

  <script>
    const TARGET = "{{TARGET_URL}}";
    const msg = document.getElementById('probeMsg');
    const state = document.getElementById('probe');

    async function probe() {
      try {
        await fetch(TARGET + "/health", { mode: "no-cors", cache: "no-store" });
        state.classList.add('ok');
        msg.innerHTML = "Connected. Loading your dashboard…";
        setTimeout(() => { window.location.href = TARGET; }, 400);
        return true;
      } catch (e) {
        return false;
      }
    }

    probe();
    setInterval(probe, 4000);
  </script>
</body>
</html>
"##;

/// Read saved server URL from ~/.syntaur/server.json (connect mode)
fn read_saved_url() -> Option<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let data = std::fs::read_to_string(format!("{}/.syntaur/server.json", home)).ok()?;
    // Parse {"url": "http://..."}
    let start = data.find("\"url\"")?.checked_add(5)?;
    let rest = &data[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')? + q1;
    Some(rest[q1..q2].to_string())
}

/// Open a URL in the system's default browser.
fn open_in_system_browser(url: &str) {
    #[cfg(target_os = "linux")]
    { let _ = std::process::Command::new("xdg-open").arg(url).spawn(); }
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(url).spawn(); }
    #[cfg(target_os = "windows")]
    { let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn(); }
}

/// Check if a URL is internal (belongs to the Syntaur gateway) or external.
fn is_external_url(url: &str, gateway_origin: &str) -> bool {
    if url.starts_with("about:") || url.starts_with("data:") || url.starts_with("blob:") { return false; }
    if url.starts_with("javascript:") { return false; }
    if !(url.starts_with("http://") || url.starts_with("https://")) { return false; }
    // Compare by ORIGIN (scheme + host + port), not by full string prefix.
    // Otherwise launching with `?token=…` or any path/query in SYNTAUR_URL
    // would treat every internal click (e.g. `/scheduler`) as external and
    // route it to the companion panel.
    extract_origin(url) != extract_origin(gateway_origin)
}

/// Pull `scheme://host[:port]` off the front of a URL. Manual parse — no
/// `url` crate dep — because we only need the prefix and the input is one
/// of two trusted shapes: our own gateway URL or a click target from the
/// page. Returns the empty string if `url` doesn't look like an absolute
/// http(s) URL, which makes `is_external_url`'s caller treat malformed
/// inputs as external (the safer side for an intercept).
/// True if `url` and `trusted` are same-origin (scheme + host + port match).
/// Used to gate auto-grant of permission-requests in the embedded webview.
/// Empty/malformed URIs default to false — the WebKit signal can fire
/// before navigation is committed (uri() returns "") and we don't want a
/// blank page to inherit the trust that the configured Syntaur URL has.
fn same_origin(url: &str, trusted: &str) -> bool {
    let a = extract_origin(url);
    let b = extract_origin(trusted);
    !a.is_empty() && a == b
}

fn extract_origin(url: &str) -> String {
    let scheme_end = match url.find("://") {
        Some(i) => i,
        None => return String::new(),
    };
    let scheme = &url[..scheme_end];
    if scheme != "http" && scheme != "https" { return String::new(); }
    let after_scheme = &url[scheme_end + 3..];
    let host_end = after_scheme.find(|c: char| c == '/' || c == '?' || c == '#')
        .unwrap_or(after_scheme.len());
    format!("{}://{}", scheme, &after_scheme[..host_end])
}

#[cfg(target_os = "linux")]
fn run_viewer(url: &str) -> Result<(), String> {
    use gtk4::prelude::*;
    use gtk4::{Application, ApplicationWindow, Paned, Orientation, Button, Box as GtkBox, Label};
    use webkit6::prelude::*;
    use webkit6::{WebView, NavigationPolicyDecision, PolicyDecisionType};
    use std::cell::RefCell;
    use std::rc::Rc;

    let app = Application::builder()
        .application_id("dev.syntaur.viewer")
        .build();

    let url_owned = url.to_string();
    let saved = load_window_state();

    app.connect_activate(move |app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .title(WINDOW_TITLE)
            .default_width(saved.width)
            .default_height(saved.height)
            .build();
        if saved.maximized {
            window.maximize();
        }

        // Save size + maximized state on close. Wayland withholds window
        // position from clients, so we only round-trip size. GTK4's
        // default_width/default_height reflect the CURRENT size after the
        // user has resized — the name is legacy from GTK3.
        window.connect_close_request(|win| {
            let s = WindowState {
                width: win.default_width(),
                height: win.default_height(),
                x: None,
                y: None,
                maximized: win.is_maximized(),
            };
            save_window_state(&s);
            gtk4::glib::Propagation::Proceed
        });

        // Main layout: horizontal paned — left=Syntaur, right=companion panel
        let paned = Paned::new(Orientation::Horizontal);

        // Left: main Syntaur webview.
        //
        // Voice mode plays TTS audio after the agent responds — that's after
        // the user-gesture activation window has lapsed, so autoplay would be
        // blocked. The viewer is single-purpose; allow audio playback without
        // a fresh gesture. WebKitGTK 6 needs BOTH levers:
        //   1. settings.media_playback_requires_user_gesture = false
        //   2. WebsitePolicies.autoplay = Allow (default is Deny which
        //      overrides the setting and corks the PulseAudio sink-input).
        let policies = webkit6::WebsitePolicies::builder()
            .autoplay(webkit6::AutoplayPolicy::Allow)
            .build();
        let webview = WebView::builder()
            .website_policies(&policies)
            .build();
        webview.set_vexpand(true);
        webview.set_hexpand(true);
        paned.set_start_child(Some(&webview));

        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
            settings.set_media_playback_requires_user_gesture(false);
        }

        // Optional debug hook: SYNTAUR_INIT_SCRIPT=<path> injects the file's JS
        // at document-start in every frame, and exposes a `window.__syntaurLog`
        // bridge via WebKit's script-message-handler that writes to
        // SYNTAUR_INIT_LOG (stderr if unset). Off by default; only set when
        // diagnosing the voice/audio pipeline against the actual viewer engine.
        if let Ok(path) = std::env::var("SYNTAUR_INIT_SCRIPT") {
            match std::fs::read_to_string(&path) {
                Ok(src) => {
                    let ucm = webview.user_content_manager().expect("ucm");
                    let script = webkit6::UserScript::new(
                        &src,
                        webkit6::UserContentInjectedFrames::AllFrames,
                        webkit6::UserScriptInjectionTime::Start,
                        &[],
                        &[],
                    );
                    ucm.add_script(&script);

                    let log_path = std::env::var("SYNTAUR_INIT_LOG").ok();
                    ucm.register_script_message_handler("syntaurLog", None);
                    ucm.connect_script_message_received(
                        Some("syntaurLog"),
                        move |_mgr, val| {
                            let s = val.to_string();
                            let line = format!("{}\n", s);
                            match &log_path {
                                Some(p) => {
                                    use std::io::Write;
                                    if let Ok(mut f) = std::fs::OpenOptions::new()
                                        .create(true).append(true).open(p)
                                    {
                                        let _ = f.write_all(line.as_bytes());
                                    }
                                }
                                None => eprint!("{}", line),
                            }
                        },
                    );
                    eprintln!("syntaur-viewer: injected init script from {}", path);
                }
                Err(e) => eprintln!("syntaur-viewer: SYNTAUR_INIT_SCRIPT read failed: {}", e),
            }
        }

        // Permission-request handler. WebKitGTK 6's default behaviour
        // for an UNHANDLED permission-request is to DENY. That breaks
        // any Syntaur feature that needs getUserMedia (chat mic, voice
        // journal), Notifications, or Geolocation — Sean reported a
        // bare `NotAllowedError: request is not allowed by the user
        // agent or the platform` from the chat-mic button after we
        // moved the gateway behind tailnet HTTPS, even though the
        // origin was now a secure context. The fix is to handle the
        // signal explicitly: auto-allow on our own origin (the viewer
        // is purpose-built for Syntaur — same-origin permissions are
        // by definition trusted), default-deny everything else so the
        // companion panel for external links keeps the safe default.
        {
            // PermissionRequestExt + connect_permission_request are
            // re-exported via webkit6::prelude::*, already in scope.
            let trusted_origin = url_owned.clone();
            webview.connect_permission_request(move |wv, req| {
                let page_uri = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                let same_origin = same_origin(&page_uri, &trusted_origin);
                if same_origin {
                    req.allow();
                } else {
                    req.deny();
                }
                true
            });
        }

        // Right: companion panel (starts hidden)
        let companion_box = GtkBox::new(Orientation::Vertical, 0);
        companion_box.set_visible(false);

        // Companion header bar with title + close button
        let header = GtkBox::new(Orientation::Horizontal, 4);
        header.set_margin_start(8);
        header.set_margin_end(4);
        header.set_margin_top(4);
        header.set_margin_bottom(4);
        let companion_title = Label::new(Some(""));
        companion_title.set_hexpand(true);
        companion_title.set_xalign(0.0);
        companion_title.add_css_class("caption");
        let close_btn = Button::with_label("Close");
        close_btn.add_css_class("flat");
        header.append(&companion_title);
        header.append(&close_btn);
        companion_box.append(&header);

        // Companion webview
        let companion_wv = WebView::new();
        companion_wv.set_vexpand(true);
        companion_wv.set_hexpand(true);
        companion_box.append(&companion_wv);

        paned.set_end_child(Some(&companion_box));
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        // State for companion panel
        let companion_visible = Rc::new(RefCell::new(false));

        // Close button hides the companion panel
        {
            let companion_box_c = companion_box.clone();
            let paned_c = paned.clone();
            let vis = companion_visible.clone();
            close_btn.connect_clicked(move |_| {
                companion_box_c.set_visible(false);
                *vis.borrow_mut() = false;
                // Give all space to the main webview
                paned_c.set_position(paned_c.allocation().width());
            });
        }

        // Intercept external links.
        //
        // Two decision types matter:
        //   - NewWindowAction fires for target="_blank", window.open(),
        //     middle-click, and Ctrl-click. These represent "the user
        //     explicitly wants this in a real browser" — we launch the
        //     system default (Firefox etc.) via xdg-open. This is what
        //     OAuth flows need, since the user has their sessions +
        //     password manager in the real browser.
        //   - NavigationAction fires for main-frame navigations (user
        //     typed a URL, or clicked a same-window link). For externals
        //     we show them in the companion panel on the right so the
        //     user doesn't lose their place in Syntaur.
        let origin = url_owned.clone();
        let companion_box_nav = companion_box.clone();
        let companion_wv_nav = companion_wv.clone();
        let companion_title_nav = companion_title.clone();
        let paned_nav = paned.clone();
        let vis_nav = companion_visible.clone();
        webview.connect_decide_policy(move |_wv, decision, decision_type| {
            if decision_type == PolicyDecisionType::NewWindowAction {
                if let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() {
                    if let Some(mut action) = nav.navigation_action() {
                        if let Some(request) = action.request() {
                            if let Some(uri) = request.uri() {
                                let uri_str = uri.to_string();
                                let _ = std::process::Command::new("xdg-open")
                                    .arg(&uri_str)
                                    .stdin(std::process::Stdio::null())
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .spawn();
                                decision.ignore();
                                return true;
                            }
                        }
                    }
                }
                return false;
            }

            if decision_type == PolicyDecisionType::NavigationAction {
                if let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() {
                    if let Some(mut action) = nav.navigation_action() {
                        if let Some(request) = action.request() {
                            if let Some(uri) = request.uri() {
                                let uri_str = uri.to_string();
                                if is_external_url(&uri_str, &origin) {
                                    // Show companion panel with the external page
                                    companion_wv_nav.load_uri(&uri_str);
                                    let title = uri_str.split('/').filter(|s| !s.is_empty()).last()
                                        .unwrap_or("Page").replace('-', " ").replace('_', " ");
                                    companion_title_nav.set_text(&title);
                                    companion_box_nav.set_visible(true);
                                    *vis_nav.borrow_mut() = true;
                                    // Split ~40% for companion
                                    let total_w = paned_nav.allocation().width();
                                    if total_w > 400 {
                                        paned_nav.set_position(total_w * 60 / 100);
                                    }
                                    decision.ignore();
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
            false
        });

        webview.load_uri(&url_owned);
        window.set_child(Some(&paned));
        window.present();
    });

    app.run_with_args::<&str>(&[]);
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_viewer(url: &str) -> Result<(), String> {
    use tao::{
        dpi::{LogicalPosition, LogicalSize},
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;

    let gateway_origin = url.to_string();
    let saved = load_window_state();
    let event_loop = EventLoop::new();
    let mut builder = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(saved.width as f64, saved.height as f64))
        .with_maximized(saved.maximized);
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        builder = builder.with_position(LogicalPosition::new(x as f64, y as f64));
    }
    let window = builder.build(&event_loop).map_err(|e| format!("window: {}", e))?;

    let _webview = WebViewBuilder::new()
        .with_url(url)
        .with_navigation_handler(move |uri| {
            if is_external_url(&uri, &gateway_origin) {
                open_in_system_browser(&uri);
                false // block navigation in webview
            } else {
                true // allow internal navigation
            }
        })
        .build(&window)
        .map_err(|e| format!("webview: {}", e))?;

    // Track current geometry as it changes so we can save it on close.
    let mut current = saved;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: ev, .. } = &event {
            match ev {
                WindowEvent::Resized(phys) => {
                    let scale = window.scale_factor();
                    let logical = phys.to_logical::<f64>(scale);
                    current.width = logical.width.round() as i32;
                    current.height = logical.height.round() as i32;
                    current.maximized = window.is_maximized();
                }
                WindowEvent::Moved(phys) => {
                    let scale = window.scale_factor();
                    let logical = phys.to_logical::<f64>(scale);
                    current.x = Some(logical.x.round() as i32);
                    current.y = Some(logical.y.round() as i32);
                }
                WindowEvent::CloseRequested => {
                    save_window_state(&current);
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn run_viewer(url: &str) -> Result<(), String> {
    use tao::{
        dpi::{LogicalPosition, LogicalSize},
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;
    // `with_additional_browser_args` is a Windows-only extension method on
    // WebViewBuilder — the trait must be in scope for the method to
    // resolve. Without this import the whole builder chain fails
    // type-inference (method not found → closure types propagate wrong →
    // nav handler reports `size for str is unknown`).
    use wry::WebViewBuilderExtWindows;

    let gateway_origin = url.to_string();
    let saved = load_window_state();
    let event_loop = EventLoop::new();
    let mut builder = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(saved.width as f64, saved.height as f64))
        .with_maximized(saved.maximized);
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        builder = builder.with_position(LogicalPosition::new(x as f64, y as f64));
    }
    let window = builder.build(&event_loop).map_err(|e| format!("window: {}", e))?;

    // CDP / remote debugging: opt-in via SYNTAUR_VIEWER_DEBUG_PORT env var.
    // WebView2 respects these flags when passed through additional_browser_args.
    // `--remote-allow-origins=*` is required because CDP uses a WebSocket and
    // recent Chromium versions reject cross-origin WS handshakes by default.
    let mut browser_args =
        String::from("--disable-gpu --disable-software-rasterizer");
    if let Ok(port) = std::env::var("SYNTAUR_VIEWER_DEBUG_PORT") {
        browser_args.push_str(&format!(
            " --remote-debugging-port={} --remote-allow-origins=*",
            port
        ));
        eprintln!(
            "[syntaur-viewer] CDP enabled — 127.0.0.1:{}/json",
            port
        );
    }

    let _webview = WebViewBuilder::new()
        .with_url(url)
        .with_additional_browser_args(&browser_args)
        .with_navigation_handler(move |uri| {
            if is_external_url(&uri, &gateway_origin) {
                open_in_system_browser(&uri);
                false
            } else {
                true
            }
        })
        .build(&window)
        .map_err(|e| format!("webview: {}", e))?;

    let mut current = saved;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: ev, .. } = &event {
            match ev {
                WindowEvent::Resized(phys) => {
                    let scale = window.scale_factor();
                    let logical = phys.to_logical::<f64>(scale);
                    current.width = logical.width.round() as i32;
                    current.height = logical.height.round() as i32;
                    current.maximized = window.is_maximized();
                }
                WindowEvent::Moved(phys) => {
                    let scale = window.scale_factor();
                    let logical = phys.to_logical::<f64>(scale);
                    current.x = Some(logical.x.round() as i32);
                    current.y = Some(logical.y.round() as i32);
                }
                WindowEvent::CloseRequested => {
                    save_window_state(&current);
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{extract_origin, is_external_url};

    #[test]
    fn origin_strips_path_query_fragment() {
        assert_eq!(extract_origin("http://192.168.1.239:18789"), "http://192.168.1.239:18789");
        assert_eq!(extract_origin("http://192.168.1.239:18789/"), "http://192.168.1.239:18789");
        assert_eq!(extract_origin("http://192.168.1.239:18789/?token=abc"), "http://192.168.1.239:18789");
        assert_eq!(extract_origin("http://192.168.1.239:18789/scheduler"), "http://192.168.1.239:18789");
        assert_eq!(extract_origin("https://example.com:8443/path?q=1#frag"), "https://example.com:8443");
        assert_eq!(extract_origin("https://example.com/path"), "https://example.com");
    }

    #[test]
    fn origin_rejects_non_http() {
        assert_eq!(extract_origin("javascript:alert(1)"), "");
        assert_eq!(extract_origin("about:blank"), "");
        assert_eq!(extract_origin("data:text/plain,hi"), "");
        assert_eq!(extract_origin("not a url"), "");
    }

    #[test]
    fn launch_url_with_token_does_not_break_internal_nav() {
        let launched = "http://192.168.1.239:18789/?token=ocp_FAKE";
        // Same-origin click should NOT be external.
        assert!(!is_external_url("http://192.168.1.239:18789/scheduler", launched));
        assert!(!is_external_url("http://192.168.1.239:18789/", launched));
        // Different host SHOULD be external.
        assert!(is_external_url("https://www.google.com", launched));
        assert!(is_external_url("http://192.168.1.69:8080/foo", launched));
        // about:/data:/javascript: never external.
        assert!(!is_external_url("about:blank", launched));
        assert!(!is_external_url("data:text/plain,hi", launched));
        assert!(!is_external_url("javascript:void(0)", launched));
    }
}
