//! Server-side page rendering via `maud`. Every UI route returns an
//! `Html<String>` produced by one of these modules.
//!
//! **Why this module exists** (see feedback/rust_first_includes_ui.md):
//! the original UI lived in hand-written `.html` files loaded via
//! `include_str!`. That accumulated 644 KB of HTML that GitHub
//! classified as non-Rust, drifting the project away from the stated
//! "everything Rust" direction. Pages are migrating here incrementally
//! — each migration deletes a `static/*.html` file and adds
//! `pages/<name>.rs`.
//!
//! ## DO NOT migrate a page that's under active parallel work.
//! Before touching any page, run `git log --oneline -20 -- syntaur-gateway/static/<page>.html`
//! and check the daily vault note for a mention. If the file was
//! modified in the last few hours, wait.
//!
//! Migration state (update as you go):
//!   [done] modules.html    → pages::modules
//!   [ok]   history.html    — safe, stable
//!   [ok]   journal.html    — safe, stable
//!   [ok]   voice-setup.html — safe, stable
//!   [ok]   landing.html    — safe, stable
//!   [ok]   music.html      — recent but stable (media bridge commits)
//!   [ok]   dashboard.html  — watch for widget changes
//!   [ok]   chat.html       — core, watch for voice-mode changes
//!   [ok]   setup.html      — watch for onboarding changes
//!   [ok]   settings.html   — watch for new tabs being added
//!   [HOLD] tax.html        — ACTIVELY being worked on by parallel sessions
//!                             (extension filing, deduction scanner,
//!                             AI Deep Scan, copy-assist). Do NOT migrate
//!                             until tax.html is untouched for a full day.

pub mod modules;
pub mod shared;
