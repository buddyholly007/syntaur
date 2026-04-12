//! Syntaur Dashboard Viewer — lightweight native webview window.
//!
//! Opens the Syntaur dashboard in a minimal OS-native window using the
//! platform's built-in web engine (WebKit on Linux, WKWebView on macOS,
//! WebView2/Edge on Windows). No full browser needed.
//!
//! Uses ~20-30 MB RAM with zero GPU usage — ideal when the GPU and RAM
//! are needed for LLM inference.

use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Icon, WindowBuilder},
};
use wry::WebViewBuilder;

const DEFAULT_URL: &str = "http://localhost:18789";
const WINDOW_TITLE: &str = "Syntaur";
const DEFAULT_WIDTH: f64 = 1200.0;
const DEFAULT_HEIGHT: f64 = 800.0;

fn main() {
    // Allow overriding the URL via CLI arg or env var
    let url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SYNTAUR_URL").ok())
        .unwrap_or_else(|| DEFAULT_URL.to_string());

    // Disable GPU compositing on Linux (WebKit)
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
        .with_window_icon(load_icon())
        .build(&event_loop)
        .expect("Failed to create window");

    let builder = WebViewBuilder::new()
        .with_url(&url)
        .with_devtools(cfg!(debug_assertions));

    // Disable GPU on Windows (WebView2/Chromium flags)
    #[cfg(target_os = "windows")]
    let builder = builder
        .with_additional_browser_args("--disable-gpu --disable-software-rasterizer");

    let _webview = builder
        .build(&window)
        .expect("Failed to create webview");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// Try to load a window icon from the embedded PNG data.
/// Returns None on failure (window just won't have an icon — not fatal).
fn load_icon() -> Option<Icon> {
    // Embedded 32x32 RGBA icon (Syntaur centaur silhouette on dark bg).
    // Generated from the SVG at build time would be ideal, but for now
    // we use a simple solid-color placeholder that works everywhere.
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Simple rounded rectangle with gradient-ish fill
            let in_rect = x >= 2 && x < 30 && y >= 2 && y < 30;
            if in_rect {
                // Sky blue gradient (#0ea5e9 -> #0369a1)
                let t = y as f32 / size as f32;
                let r = (14.0 + t * (3.0 - 14.0)) as u8;
                let g = (165.0 + t * (105.0 - 165.0)) as u8;
                let b = (233.0 + t * (161.0 - 233.0)) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Icon::from_rgba(rgba, size, size).ok()
}
