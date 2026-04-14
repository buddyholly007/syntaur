//! Server-side page rendering via `maud`. Every UI route returns an
//! `Html<String>` produced by one of these modules.
//!
//! **Why this module exists** (see feedback/rust_first_includes_ui.md):
//! the original UI lived in hand-written `.html` files loaded via
//! `include_str!`. That accumulated 644 KB of HTML that GitHub
//! classified as non-Rust. Pages migrate here incrementally.
//!
//! ## DO NOT migrate a page under active parallel work.
//! Before touching a page, `git log --oneline -20 -- <path>` and check
//! the daily vault note. See `feedback/check_before_deleting.md`.
//!
//! Migration state:
//!   [done] modules.html     → pages::modules
//!   [done] history.html     → pages::history
//!   [done] journal.html     → pages::journal
//!   [done] voice-setup.html → pages::voice_setup
//!   [done] landing.html     → pages::landing
//!   [done] music.html      → pages::music
//!   [done] dashboard.html  → pages::dashboard
//!   [done] chat.html       → pages::chat
//!   [done] setup.html      → pages::setup
//!   [done] settings.html   → pages::settings
//!   [HOLD] tax.html — parallel tax-module work, do NOT migrate

pub mod history;
pub mod journal;
pub mod landing;
pub mod modules;
pub mod chat;
pub mod dashboard;
pub mod music;
pub mod settings;
pub mod setup;
pub mod shared;
pub mod voice_setup;
