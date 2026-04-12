//! OAuth2 authorization_code flow with PKCE (v5 Item 4).
//!
//! Builds on Item 3's per-user auth: each oauth_tokens row is stamped
//! with a `user_id`, so Alice's Google Calendar token never gets served
//! to Bob.
//!
//! ## Module layout
//!
//! * [`state`] — in-memory state cache for in-flight authorization
//!   requests. Key: opaque random `state` query param. Value: the
//!   originating user_id, provider, and PKCE code_verifier. 10 minute
//!   TTL; if the user doesn't click through in time, the row disappears.
//! * [`tokens`] — write-through cache on top of the `oauth_tokens`
//!   SQLite table. Handles refresh (30s before expiry) and per-user +
//!   per-provider lookups.
//! * [`pkce`] — PKCE S256 challenge/verifier pair generation.
//!
//! ## Flow
//!
//! 1. User runs `/connect <provider>` in Telegram (or POST /api/oauth/start).
//! 2. Gateway creates a state entry, returns an authorization URL with
//!    state + code_challenge + scopes.
//! 3. User opens the URL in their browser, approves.
//! 4. Provider redirects to `/api/oauth/callback?code=...&state=...`.
//! 5. Gateway pops the state entry, exchanges code → access/refresh
//!    tokens, persists to `oauth_tokens`, tells the user "connected".
//! 6. Later, when a tool call needs the token, `AuthCodeTokenCache::get`
//!    returns the current token (refreshing if needed) or an error if
//!    the user hasn't connected yet.

pub mod pkce;
pub mod state;
pub mod tokens;

pub use pkce::PkcePair;
pub use state::{OAuthStateCache, PendingAuthEntry};
pub use tokens::{AuthCodeTokenCache, OAuthTokenRow};
