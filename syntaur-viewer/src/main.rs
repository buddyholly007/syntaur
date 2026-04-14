//! Syntaur Dashboard Viewer — native webview, no browser needed.
//!
//! Linux: GTK4 + WebKitGTK 6.0 — native Wayland & X11, fractional scaling.
//! macOS: WKWebView via wry+tao.
//! Windows: WebView2 via wry+tao.

const DEFAULT_URL: &str = "http://localhost:18789";
const WINDOW_TITLE: &str = "Syntaur";

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
    use gtk4::{Application, ApplicationWindow};
    use webkit6::prelude::*;
    use webkit6::{WebView, NavigationPolicyDecision, PolicyDecisionType};

    let app = Application::builder()
        .application_id("dev.syntaur.viewer")
        .build();

    let url_owned = url.to_string();

    app.connect_activate(move |app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .title(WINDOW_TITLE)
            .default_width(1200)
            .default_height(800)
            .build();

        let webview = WebView::new();
        webview.set_vexpand(true);
        webview.set_hexpand(true);

        // Intercept external links — open in system browser
        let origin = url_owned.clone();
        webview.connect_decide_policy(move |_wv, decision, decision_type| {
            if decision_type == PolicyDecisionType::NavigationAction {
                if let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() {
                    if let Some(mut action) = nav.navigation_action() {
                        if let Some(request) = action.request() {
                            if let Some(uri) = request.uri() {
                                let uri_str = uri.to_string();
                                if is_external_url(&uri_str, &origin) {
                                    open_in_system_browser(&uri_str);
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
        window.set_child(Some(&webview));
        window.present();
    });

    app.run_with_args::<&str>(&[]);
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_viewer(url: &str) -> Result<(), String> {
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;

    let gateway_origin = url.to_string();
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .build(&event_loop)
        .map_err(|e| format!("window: {}", e))?;

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

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "windows")]
fn run_viewer(url: &str) -> Result<(), String> {
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;

    let gateway_origin = url.to_string();
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .build(&event_loop)
        .map_err(|e| format!("window: {}", e))?;

    let _webview = WebViewBuilder::new()
        .with_url(url)
        .with_additional_browser_args("--disable-gpu --disable-software-rasterizer")
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

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *control_flow = ControlFlow::Exit;
        }
    });
}
