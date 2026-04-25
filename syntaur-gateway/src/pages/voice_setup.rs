//! /voice-setup — Voice Module setup landing page. Pure informational
//! content (6 section cards), no JS, pre-auth accessible.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Voice Module Setup",
        authed: false,
        extra_style: None,
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    Html(shell(page, body()).into_string())
}

fn body() -> Markup {
    html! {
        (top_bar())
        div class="max-w-3xl mx-auto px-4 py-8 space-y-8" {
            (section_voice_input())
            (section_phone_app())
            (section_wake_word())
            (section_journal())
            (section_wearable())
            (section_consent())
        }
    }
}

fn top_bar() -> Markup {
    html! {
        div class="border-b border-gray-800 bg-gray-900/80 backdrop-blur" {
            div class="max-w-4xl mx-auto px-4 py-2.5 flex items-center justify-between" {
                div class="flex items-center gap-3" {
                    a href="/" class="flex items-center gap-2 hover:opacity-80" {
                        img src="/icon.svg" class="w-5 h-5" alt="";
                        span class="font-semibold hidden sm:inline" { "Syntaur" }
                    }
                    span class="text-gray-300 text-sm font-medium" { "Voice Module Setup" }
                }
                div class="flex items-center gap-3 text-sm" {
                    a href="/journal" class="text-gray-500 hover:text-gray-300" { "Journal" }
                    a href="/modules" class="text-gray-500 hover:text-gray-300" { "Modules" }
                    a href="/" class="text-gray-500 hover:text-gray-300" { "Home" }
                }
            }
        }
    }
}

// Inline SVG helpers — these are 15-20 lines each, readable in Rust as raw strings.
const SVG_MIC: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#0ea5e9" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/></svg>"##;
const SVG_PHONE: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#a78bfa" stroke-width="2"><rect x="5" y="2" width="14" height="20" rx="2"/><line x1="12" y1="18" x2="12" y2="18.01"/></svg>"##;
const SVG_STAR: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#f59e0b" stroke-width="2"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26"/></svg>"##;
const SVG_DOC: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#34d399" stroke-width="2"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>"##;
const SVG_GEAR: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#fb7185" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 112.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9c.26.604.852.997 1.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z"/></svg>"##;
const SVG_SHIELD: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#9ca3af" stroke-width="2"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>"##;

fn section_voice_input() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-oc-600/20 flex items-center justify-center" {
                    (PreEscaped(SVG_MIC))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Voice Input" }
                    p class="text-sm text-gray-500" { "Talk to your assistant instead of typing" }
                }
                span class="ml-auto bg-green-900/30 text-green-400 text-xs px-3 py-1 rounded-full" { "Ready" }
            }
            p class="text-sm text-gray-400 leading-relaxed" {
                "Use the microphone button in the "
                a href="/chat" class="text-oc-500 hover:text-oc-400" { "chat page" }
                " to speak instead of typing. Hold the mic icon, speak your message, and release. "
                "Your speech is transcribed locally using Parakeet and sent to your assistant."
            }
        }
    }
}

fn section_phone_app() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-purple-600/20 flex items-center justify-center" {
                    (PreEscaped(SVG_PHONE))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Phone App" }
                    p class="text-sm text-gray-500" {
                        "Record and talk to your assistant from your phone"
                    }
                }
            }
            p class="text-sm text-gray-400 leading-relaxed mb-4" {
                "Use your phone as a mobile microphone. Record journal entries on the go or talk to your assistant from anywhere on your home network. Supports both journal mode (record and transcribe) and assistant mode (two-way conversation)."
            }
            div class="bg-gray-950 rounded-xl p-4 text-center" {
                p class="text-sm text-gray-500 mb-3" { "Scan to open on your phone" }
                div class="inline-block bg-white p-3 rounded-xl" {
                    img src="/voice-setup/qr" class="w-48 h-48" alt="QR Code" id="phone-qr";
                }
                p class="text-xs text-gray-600 mt-3" {
                    "First time? You'll be guided through a quick certificate install for secure mic access."
                }
            }
        }
    }
}

fn section_wake_word() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-amber-600/20 flex items-center justify-center" {
                    (PreEscaped(SVG_STAR))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Wake Word Training" }
                    p class="text-sm text-gray-500" { "Teach your assistant to recognize your voice" }
                }
                span class="ml-auto text-xs px-3 py-1 rounded-full" id="ww-status" { "-" }
            }
            p class="text-sm text-gray-400 leading-relaxed mb-4" {
                "Your assistant listens for a specific phrase — your "
                strong class="text-white" { "wake word" }
                " — before responding. To work reliably, it needs to learn what "
                em { "your" }
                " voice sounds like saying that phrase. Recording takes under a minute: you'll say your wake word 5 times."
            }
            p class="text-sm text-gray-400 mb-4" {
                "This also helps with "
                strong class="text-white" { "speaker identification" }
                " — so your assistant knows it's you speaking, not someone else in the room."
            }
            div class="flex items-center gap-3" {
                input type="text" id="wake-word-input" placeholder="Enter your wake word (e.g. Hey Atlas)"
                    class="flex-1 bg-gray-950 border border-gray-800 focus:border-oc-500 rounded-xl px-4 py-2.5 text-sm text-white placeholder-gray-600 outline-none";
                a href="/voice-setup/wake-word"
                    class="bg-amber-600 hover:bg-amber-700 text-white text-sm font-semibold px-5 py-2.5 rounded-xl transition-colors"
                    id="train-btn" { "Train" }
            }
        }
    }
}

fn section_journal() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-emerald-600/20 flex items-center justify-center" {
                    (PreEscaped(SVG_DOC))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Audio Journal" }
                    p class="text-sm text-gray-500" { "Searchable transcripts of your recorded conversations" }
                }
            }
            p class="text-sm text-gray-400 leading-relaxed mb-4" {
                "Every recording is transcribed and saved as a daily journal. Search by keyword, date, or topic. Your assistant can also search your journal — ask \"what did I talk about yesterday?\" and it will find the answer."
            }
            a href="/journal"
                class="inline-block bg-emerald-600 hover:bg-emerald-700 text-white text-sm font-semibold px-5 py-2.5 rounded-xl transition-colors" {
                "Open Journal"
            }
        }
    }
}

fn section_wearable() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-rose-600/20 flex items-center justify-center" {
                    (PreEscaped(SVG_GEAR))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Wearable Devices" }
                    p class="text-sm text-gray-500" { "Connect a BLE pendant for hands-free recording" }
                }
                span class="ml-auto text-xs text-gray-600" { "Optional" }
            }
            p class="text-sm text-gray-400 leading-relaxed mb-3" {
                "Pair a Bluetooth wearable (Limitless pendant, Omi necklace) for always-available recording. The pendant records to flash when you're away from home and syncs automatically when you return."
            }
            p class="text-sm text-gray-500 leading-relaxed" {
                strong class="text-gray-400" { "Requirements:" }
                " A BLE-capable host on your network running the "
                code class="text-xs bg-gray-950 px-1.5 py-0.5 rounded" { "ble-relay" }
                " adapter. See the "
                a href="https://github.com/syntaur/docs/voice-wearable" class="text-oc-500 hover:text-oc-400" {
                    "setup guide"
                }
                " for instructions."
            }
        }
    }
}

fn section_consent() -> Markup {
    html! {
        section class="bg-gray-900 border border-gray-800 rounded-2xl p-6" {
            div class="flex items-center gap-3 mb-4" {
                div class="w-10 h-10 rounded-xl bg-gray-700/30 flex items-center justify-center" {
                    (PreEscaped(SVG_SHIELD))
                }
                div {
                    h2 class="text-lg font-semibold text-white" { "Privacy & Consent" }
                    p class="text-sm text-gray-500" { "Your voice never leaves your network" }
                }
            }
            p class="text-sm text-gray-400 leading-relaxed mb-4" {
                "All audio processing happens locally on your machine. Recordings, transcripts, and training data are stored in your Syntaur data directory and never sent to any cloud service."
            }
            div class="bg-gray-950 rounded-xl p-4" {
                label class="flex items-center gap-3 cursor-pointer" {
                    span class="text-sm text-gray-300" { "Recording consent mode" }
                    select id="consent-mode"
                        class="ml-auto bg-gray-900 border border-gray-800 rounded-lg px-3 py-1.5 text-sm text-white outline-none" {
                        option value="all_party" { "All-party consent (recommended)" }
                        option value="one_party" { "One-party consent" }
                    }
                }
                p class="text-xs text-gray-600 mt-2" {
                    "All-party consent requires all participants to be aware of recording. This is the legal default in California, Florida, and other two-party consent states."
                }
            }
        }
    }
}
