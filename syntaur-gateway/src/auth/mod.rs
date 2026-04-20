//! Per-user authentication (v5 Item 3).
//!
//! Introduces a real user model on top of the v6-and-earlier global API
//! token. Key design decisions (from the v5 plan):
//!
//! * **API tokens only, no browser UI.** Each user gets one or more
//!   long-lived bearer tokens (think GitHub PATs). No cookies, no sessions,
//!   no OAuth login providers.
//! * **Token hashes, not tokens.** We store `SHA256(raw_token)` in
//!   `user_api_tokens.token_hash`. Raw tokens are shown exactly once when
//!   they're minted; there's no "reveal token" API.
//! * **Passwords are argon2id-hashed per user.** No global shared password.
//!   The pre-v0.5.0 `gateway.auth.token` / `gateway.auth.password` fallback
//!   was removed in favor of the `/setup/register` bootstrap + the per-user
//!   password column.
//!
//! This module is entirely local. Nothing here talks to an external
//! identity provider; that's Item 4's job.

pub mod principal;
pub mod users;

pub use principal::{Principal, ADMIN_USER_ID};
pub use users::UserStore;
