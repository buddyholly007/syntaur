//! /landing — public marketing page served by the gateway. Static
//! content: hero, features, how-it-works, LLM options, modules,
//! pricing, CTA, footer.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Your Personal AI Platform",
        authed: false,
        extra_style: Some(".glow { box-shadow: 0 0 60px rgba(14,165,233,0.15); }"),
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    Html(shell(page, body()).into_string())
}

fn body() -> Markup {
    html! {
        (nav())
        (hero())
        (features())
        (how_it_works())
        (llm_options())
        (modules())
        (pricing())
        (cta())
        (footer())
    }
}

fn nav() -> Markup {
    html! {
        nav class="border-b border-gray-800/50 bg-gray-950/80 backdrop-blur sticky top-0 z-50" {
            div class="max-w-5xl mx-auto px-4 py-4 flex items-center justify-between" {
                div class="flex items-center gap-2" {
                    img src="/icon.svg" class="w-6 h-6" alt="";
                    span class="font-bold text-lg" { "Syntaur" }
                }
                div class="flex items-center gap-6 text-sm" {
                    a href="#features" class="text-gray-400 hover:text-white" { "Features" }
                    a href="#modules" class="text-gray-400 hover:text-white" { "Modules" }
                    a href="#pricing" class="text-gray-400 hover:text-white" { "Pricing" }
                    a href="https://github.com/buddyholly007/syntaur" class="text-gray-400 hover:text-white" {
                        "GitHub"
                    }
                    a href="#download"
                        class="bg-oc-600 hover:bg-oc-700 text-white px-4 py-2 rounded-lg font-medium transition-colors" {
                        "Download"
                    }
                }
            }
        }
    }
}

fn hero() -> Markup {
    html! {
        section class="max-w-5xl mx-auto px-4 pt-24 pb-16 text-center" {
            // VERSION-BADGE markers are matched verbatim by
            // syntaur-ship's post-deploy audit (stages/version_audit.rs
            // ::landing_badge) — the probe string-slices on these exact
            // comment bytes. Before the maud migration the same shape
            // lived in landing/index.html and scripts/sync-version.sh
            // patched it; now CARGO_PKG_VERSION inherits from the
            // workspace version and stays in sync automatically.
            (PreEscaped(concat!(
                "<!-- VERSION-BADGE -->v",
                env!("CARGO_PKG_VERSION"),
                "<!-- /VERSION-BADGE -->"
            )))
            div class="inline-flex items-center gap-2 bg-gray-800/50 border border-gray-700 rounded-full px-4 py-1.5 text-sm text-gray-400 mb-6" {
                span class="w-2 h-2 rounded-full bg-green-400" {}
                "v"
                (env!("CARGO_PKG_VERSION"))
                " · Free & open source — runs on your hardware"
            }
            h1 class="text-5xl sm:text-6xl font-extrabold leading-tight mb-6" {
                "Your personal" br;
                span class="text-oc-500" { "AI platform" }
            }
            p class="text-xl text-gray-400 max-w-2xl mx-auto mb-10" {
                "One binary. 88 tools. Voice assistant, smart home control, browser automation, and more — all private, all local. No Docker, no Python, no complexity."
            }
            div class="flex flex-col sm:flex-row gap-3 justify-center" id="download" {
                a href="https://github.com/buddyholly007/syntaur/releases/latest"
                    class="bg-oc-600 hover:bg-oc-700 text-white px-8 py-3.5 rounded-xl font-semibold text-lg transition-colors inline-flex items-center justify-center gap-2" {
                    "Download Syntaur"
                    span class="text-sm opacity-70" { "35 MB" }
                }
                div class="bg-gray-800 border border-gray-700 rounded-xl px-6 py-3.5 font-mono text-sm text-gray-300 select-all" {
                    "curl -sSL https://buddyholly007.github.io/syntaur/install.sh | sh"
                }
            }
            p class="text-xs text-gray-600 mt-4" {
                "Linux, macOS, Windows · Free tier included · No account required"
            }
        }
    }
}

struct Feature { icon: &'static str, title: &'static str, body: &'static str, glow: bool }

fn features() -> Markup {
    let items = [
        Feature { icon: "💬", title: "AI Chat", body: "Full-featured chat with markdown, code blocks, tool visualization. Connect any LLM — local, network, or cloud.", glow: true },
        Feature { icon: "🎤", title: "Voice Assistant", body: "Talk to your AI with wake word, speech-to-text, and natural text-to-speech. Works hands-free with a room speaker.", glow: false },
        Feature { icon: "🏠", title: "Smart Home", body: "Control lights, thermostats, locks through conversation. Integrates with Home Assistant for 1000+ device types.", glow: false },
        Feature { icon: "🛠", title: "88 Built-in Tools", body: "Web search, email, file management, browser automation, office documents, CAPTCHA solving, social media, and more.", glow: false },
        Feature { icon: "🔒", title: "Private by Default", body: "Runs 100% on your hardware. Your conversations never leave your network. Use a local LLM for zero cloud dependency.", glow: false },
        Feature { icon: "🚀", title: "Zero Dependencies", body: "One 35 MB binary. No Docker, no Python, no Node.js, no YAML configs. Download, run, done.", glow: false },
    ];
    html! {
        section class="max-w-5xl mx-auto px-4 py-16" id="features" {
            h2 class="text-3xl font-bold text-center mb-12" { "Everything in one binary" }
            div class="grid grid-cols-1 md:grid-cols-3 gap-6" {
                @for f in &items {
                    div class=(format!("bg-gray-800/50 border border-gray-700/50 rounded-2xl p-6{}",
                        if f.glow { " glow" } else { "" })) {
                        div class="text-3xl mb-3" { (f.icon) }
                        h3 class="font-semibold text-lg mb-2" { (f.title) }
                        p class="text-gray-400 text-sm" { (f.body) }
                    }
                }
            }
        }
    }
}

fn how_it_works() -> Markup {
    let steps = [
        (1, "Download", "One binary for your platform. 35 MB."),
        (2, "Run", "Browser opens automatically to the setup wizard."),
        (3, "Configure", "Pick your LLM, name your AI, enable modules."),
        (4, "Chat", "Talk to your AI from browser, phone, or voice."),
    ];
    html! {
        section class="max-w-5xl mx-auto px-4 py-16" {
            h2 class="text-3xl font-bold text-center mb-12" { "Up and running in 5 minutes" }
            div class="grid grid-cols-1 md:grid-cols-4 gap-4" {
                @for (n, t, d) in &steps {
                    div class="text-center" {
                        div class="w-10 h-10 rounded-full bg-oc-600 text-white font-bold flex items-center justify-center mx-auto mb-3" {
                            (n)
                        }
                        h3 class="font-medium mb-1" { (t) }
                        p class="text-sm text-gray-500" { (d) }
                    }
                }
            }
        }
    }
}

fn llm_options() -> Markup {
    let opts = [
        ("Local GPU", "Ollama + your NVIDIA/AMD/Apple GPU", "Free · Private", "text-green-400"),
        ("Network LLM", "Auto-discovers Ollama on your LAN", "Free · LAN-only", "text-blue-400"),
        ("OpenRouter", "Free tier with tool-capable models", "Free · No card", "text-yellow-400"),
        ("Groq", "Llama 3.3 70B at ~250 tok/s", "Free · No card", "text-yellow-400"),
        ("Cerebras", "Qwen 3 235B at ~2000 tok/s (fastest)", "Free · No card", "text-yellow-400"),
        ("OpenAI / Anthropic", "GPT-4o, Claude Sonnet", "Pay-per-use", "text-gray-400"),
    ];
    html! {
        section class="max-w-5xl mx-auto px-4 py-16" {
            h2 class="text-3xl font-bold text-center mb-4" { "Bring your own brain" }
            p class="text-gray-400 text-center mb-12 max-w-xl mx-auto" {
                "Use any LLM — local or cloud. Syntaur auto-detects your hardware and recommends the best setup with automatic fallbacks across three free cloud tiers."
            }
            div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4" {
                @for (name, desc, badge, badge_color) in &opts {
                    div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-4 text-center" {
                        p class="font-medium" { (name) }
                        p class="text-xs text-gray-500 mt-1" { (desc) }
                        p class=(format!("text-xs {} mt-2", badge_color)) { (badge) }
                    }
                }
            }
        }
    }
}

fn modules() -> Markup {
    let free = ["Files & Memory", "Shell & Code", "Web Search", "Telegram"];
    let pro = ["Voice Assistant", "Smart Home", "Email & SMS", "Office Docs",
               "Browser Automation", "Social Media", "Finance", "Security Cameras"];
    html! {
        section class="max-w-5xl mx-auto px-4 py-16" id="modules" {
            h2 class="text-3xl font-bold text-center mb-4" { "Modular by design" }
            p class="text-gray-400 text-center mb-12 max-w-xl mx-auto" {
                "Enable only what you need. Extend with community modules."
            }
            div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-3" {
                @for name in &free {
                    div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-3 text-center text-sm" {
                        span class="text-green-400 text-xs block mb-1" { "Free" }
                        (name)
                    }
                }
                @for name in &pro {
                    div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-3 text-center text-sm" {
                        span class="text-oc-500 text-xs block mb-1" { "Pro" }
                        (name)
                    }
                }
            }
        }
    }
}

fn pricing() -> Markup {
    html! {
        section class="max-w-5xl mx-auto px-4 py-16" id="pricing" {
            h2 class="text-3xl font-bold text-center mb-12" { "Simple pricing" }
            div class="grid grid-cols-1 md:grid-cols-2 gap-6 max-w-3xl mx-auto" {
                div class="bg-gray-800/50 border border-gray-700/50 rounded-2xl p-8" {
                    h3 class="font-bold text-xl mb-2" { "Free" }
                    p class="text-4xl font-extrabold mb-1" { "$0" }
                    p class="text-gray-500 text-sm mb-6" { "Forever" }
                    ul class="space-y-2 text-sm text-gray-300" {
                        (pricing_item("text-green-400", "AI chat with any LLM"))
                        (pricing_item("text-green-400", "Web search & file management"))
                        (pricing_item("text-green-400", "Shell & code execution"))
                        (pricing_item("text-green-400", "Telegram integration"))
                        (pricing_item("text-green-400", "Unlimited conversations"))
                        (pricing_item("text-green-400", "Community modules"))
                    }
                    a href="https://github.com/buddyholly007/syntaur/releases/latest"
                       class="block mt-6 text-center bg-gray-700 hover:bg-gray-600 text-white px-6 py-2.5 rounded-lg font-medium transition-colors" {
                        "Download Free"
                    }
                }
                div class="bg-gray-800/50 border-2 border-oc-600 rounded-2xl p-8 relative" {
                    span class="absolute -top-3 left-1/2 -translate-x-1/2 bg-oc-600 text-white text-xs font-bold px-3 py-1 rounded-full" {
                        "Most Popular"
                    }
                    h3 class="font-bold text-xl mb-2" { "Pro" }
                    p class="text-4xl font-extrabold mb-1" { "$49" }
                    p class="text-gray-500 text-sm mb-6" { "One-time purchase" }
                    ul class="space-y-2 text-sm text-gray-300" {
                        (pricing_item("text-oc-500", "Everything in Free"))
                        (pricing_item("text-oc-500", "Voice assistant (wake word, TTS, STT)"))
                        (pricing_item("text-oc-500", "Smart home control"))
                        (pricing_item("text-oc-500", "Email, SMS, browser automation"))
                        (pricing_item("text-oc-500", "Office documents, social media"))
                        (pricing_item("text-oc-500", "Finance & security cameras"))
                        (pricing_item("text-oc-500", "Perpetual license — pay once, use forever"))
                    }
                    a href="/checkout"
                       class="block mt-6 text-center bg-oc-600 hover:bg-oc-700 text-white px-6 py-2.5 rounded-lg font-semibold transition-colors" {
                        "Buy Pro — $49"
                    }
                }
            }
        }
    }
}

fn pricing_item(check_color: &'static str, text: &'static str) -> Markup {
    html! {
        li class="flex items-center gap-2" {
            span class=(check_color) { (PreEscaped("&#10003;")) }
            (text)
        }
    }
}

fn cta() -> Markup {
    html! {
        section class="max-w-5xl mx-auto px-4 py-20 text-center" {
            h2 class="text-3xl font-bold mb-4" { "Your AI. Your hardware. Your rules." }
            p class="text-gray-400 mb-8" { "Download Syntaur and start chatting in 5 minutes." }
            a href="https://github.com/buddyholly007/syntaur/releases/latest"
               class="bg-oc-600 hover:bg-oc-700 text-white px-8 py-3.5 rounded-xl font-semibold text-lg transition-colors inline-block" {
                "Download Syntaur"
            }
        }
    }
}

fn footer() -> Markup {
    html! {
        footer class="border-t border-gray-800 py-8" {
            div class="max-w-5xl mx-auto px-4 flex flex-col sm:flex-row items-center justify-between gap-4 text-sm text-gray-600" {
                div class="flex items-center gap-2" {
                    img src="/icon.svg" class="w-4 h-4 opacity-50" alt="";
                    span { "Syntaur" }
                }
                div class="flex items-center gap-6" {
                    a href="https://github.com/buddyholly007/syntaur" class="hover:text-gray-400" { "GitHub" }
                    a href="https://github.com/buddyholly007/syntaur/issues" class="hover:text-gray-400" { "Support" }
                    a href="/setup" class="hover:text-gray-400" { "Documentation" }
                }
            }
        }
    }
}
