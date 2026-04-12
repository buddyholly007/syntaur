//! Per-user authentication (v5 Item 3).
//!
//! Introduces a real user model on top of the v6-and-earlier global API
//! token. Key design decisions (from the v5 plan):
//!
//! * **API tokens only, no browser UI.** Each user gets one or more
//!   long-lived bearer tokens (think GitHub PATs). No cookies, no sessions,
//!   no OAuth login providers.
//! * **Legacy admin fallback.** If the `users` table is empty and the
//!   caller presents the pre-existing `gateway.auth.token` value, the
//!   request resolves to a synthetic admin user (`user_id = 0`). A fresh
//!   install keeps working without any bootstrap step.
//! * **Token hashes, not tokens.** We store `SHA256(raw_token)` in
//!   `user_api_tokens.token_hash`. Raw tokens are shown exactly once when
//!   they're minted; there's no "reveal token" API.
//! * **No password storage.** There are no passwords to store.
//!
//! This module is entirely local. Nothing here talks to an external
//! identity provider; that's Item 4's job.

pub mod principal;
pub mod users;

pub use principal::{legacy_admin_enabled, Principal, ADMIN_USER_ID};
pub use users::UserStore;
