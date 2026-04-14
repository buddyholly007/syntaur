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
    use gtk4::{Application, ApplicationWindow, Paned, Orientation, Button, Box as GtkBox, Label};
    use webkit6::prelude::*;
    use webkit6::{WebView, NavigationPolicyDecision, PolicyDecisionType};
    use std::cell::RefCell;
    use std::rc::Rc;

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

        // Intercept external links — show in companion panel
        let origin = url_owned.clone();
        let companion_box_nav = companion_box.clone();
        let companion_wv_nav = companion_wv.clone();
        let companion_title_nav = companion_title.clone();
        let paned_nav = paned.clone();
        let vis_nav = companion_visible.clone();
        webview.connect_decide_policy(move |_wv, decision, decision_type| {
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
