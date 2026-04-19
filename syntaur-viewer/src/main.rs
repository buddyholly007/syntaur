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

    if let Err(e) = run_viewer(&url) {
        eprintln!("[syntaur-viewer] Failed: {}", e);
        std::process::exit(1);
    }
}

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
    if url.starts_with(gateway_origin) { return false; }
    if url.starts_with("about:") || url.starts_with("data:") || url.starts_with("blob:") { return false; }
    if url.starts_with("javascript:") { return false; }
    url.starts_with("http://") || url.starts_with("https://")
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

        // Left: main Syntaur webview
        let webview = WebView::new();
        webview.set_vexpand(true);
        webview.set_hexpand(true);
        paned.set_start_child(Some(&webview));

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
