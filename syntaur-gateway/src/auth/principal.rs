//! Principal extractor + legacy admin fallback.
//!
//! Every HTTP request to syntaur resolves to a `Principal` before
//! its handler runs. Principal is the answer to "who is this?" and is
//! the key we use to scope conversations, pending approvals, and OAuth
//! tokens.
//!
//! ## Resolution order
//!
//! 1. Parse `Authorization: Bearer <token>` (or `?token=<token>` in the
//!    query string — kept for back-compat with curl-style scripts).
//! 2. Try to resolve the raw token against `user_api_tokens` via
//!    `UserStore::resolve_token`. On hit, return `Principal::User`.
//! 3. If the `users` table is *empty* AND the token matches the legacy
//!    `gateway.auth.token` from the config file, return
//!    `Principal::LegacyAdmin`. The legacy fallback only runs when no real
//!    users are configured so a fresh install keeps working.
//! 4. Otherwise, return 401.
//!
//! ## Design constraints
//!
//! * The extractor runs on every request. It must be fast and allocation-
//!   light. We do one DB hit per request (the token resolution) and cache
//!   the `Principal` on the request extensions so downstream code doesn't
//!   re-resolve.
//! * The legacy fallback is **on by default** and **off when any real
//!   user exists**. No config switch: the users table is the source of
//!   truth.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Query};
use axum::http::{request::Parts, StatusCode};

use crate::auth::users::UserStore;

/// Synthetic user_id for the legacy global-token admin. All rows
/// predating Item 3 are stamped with 0 during the v7 migration, so reads
/// from any existing table pre-filter to this id when the admin is the
/// caller.
pub const ADMIN_USER_ID: i64 = 0;

#[derive(Debug, Clone)]
pub enum Principal {
    /// Pre-Item-3 admin — the system is still running in "legacy" mode
    /// (empty users table) and the caller presented the global token.
    /// Treated as super-user: sees all rows, can hit admin endpoints,
    /// can approve anything.
    LegacyAdmin,
    /// Real user row from the `users` table.
    User { id: i64, name: String },
}

impl Principal {
    /// The effective user_id used to stamp writes and filter reads.
    /// Legacy admin uses 0, real users use their row id.
    pub fn user_id(&self) -> i64 {
        match self {
            Self::LegacyAdmin => ADMIN_USER_ID,
            Self::User { id, .. } => *id,
        }
    }

    /// Human-readable label used in logs and audit entries.
    pub fn label(&self) -> &str {
        match self {
            Self::LegacyAdmin => "legacy-admin",
            Self::User { name, .. } => name.as_str(),
        }
    }

    /// True iff the caller can hit admin endpoints (create users, mint
    /// tokens, manage Telegram links, see any user's data).
    ///
    /// Two paths to admin:
    ///   * `LegacyAdmin` — empty users table, legacy global token. Used
    ///     before any real user is bootstrapped.
    ///   * `User { id: 1, .. }` — the first user created via
    ///     `bootstrap-admin` always gets id 1, and we treat that id as
    ///     admin. Cleaner long-term: add an `is_admin INTEGER` column
    ///     to the users table and load the flag from there. v5 polish.
    pub fn is_admin(&self) -> bool {
        match self {
            Self::LegacyAdmin => true,
            Self::User { id, .. } => *id == 1,
        }
    }

    /// Row-level filter key for stores that support per-user scoping.
    ///
    /// `None`  = no filter (admin, see everything).
    /// `Some(uid)` = `WHERE user_id = uid`.
    ///
    /// This is the single function every handler calls when passing a
    /// scope to ConversationManager / PendingActionStore / SessionStore,
    /// so there's exactly one place that decides "does this principal
    /// see everyone's data or only their own?"
    pub fn scope(&self) -> Option<i64> {
        match self {
            Self::LegacyAdmin => None,
            Self::User { id, .. } => Some(*id),
        }
    }
}

/// True if the "legacy admin" token should still be honored. Returns
/// true iff the users table is empty — as soon as the first real user
/// lands, the legacy path is permanently disabled.
pub async fn legacy_admin_enabled(users: &UserStore) -> bool {
    users.is_empty().await.unwrap_or(true)
}

// ── axum extractor ────────────────────────────────────────────────────

/// Pieces the extractor needs from AppState. We can't pass AppState
/// directly because the extractor runs inside the router layer and the
/// type is `Arc<crate::AppState>`. Instead, both shared stores get
/// promoted to request extensions during the router `with_state` call.
#[derive(Clone)]
pub struct AuthContext {
    pub users: Arc<UserStore>,
    pub legacy_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

impl<S> FromRequestParts<S> for Principal
where
    S: Send + Sync,
    AuthContext: axum::extract::FromRef<S>,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx = <AuthContext as axum::extract::FromRef<S>>::from_ref(state);

        // Pull the raw token from either Authorization: Bearer or ?token=
        let bearer = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
            .map(|s| s.to_string());

        let (raw_token, via_query_string) = if let Some(t) = bearer {
            (Some(t), false)
        } else {
            // Fall back to query param for curl-style callers.
            let qs = Query::<TokenQuery>::try_from_uri(&parts.uri)
                .ok()
                .and_then(|Query(q)| q.token);
            let is_qs = qs.is_some();
            (qs, is_qs)
        };

        if via_query_string {
            log::warn!(
                "[auth] DEPRECATED: token passed via ?token= query string. \
                 Use Authorization: Bearer <token> header instead. \
                 Query string tokens may leak into browser history, server logs, and referrer headers."
            );
        }

        let Some(raw) = raw_token else {
            return Err((
                StatusCode::UNAUTHORIZED,
                "missing bearer token".to_string(),
            ));
        };

        // 1. Try the user tokens table first.
        match ctx.users.resolve_token(&raw).await {
            Ok(Some(resolved)) => {
                return Ok(Principal::User {
                    id: resolved.user_id,
                    name: resolved.user_name,
                });
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("[auth] token lookup error: {}", e);
                // Fall through to the legacy check so a transient DB
                // error on the users table doesn't lock out the admin.
            }
        }

        // 2. Legacy admin fallback — only when users table is empty.
        if legacy_admin_enabled(&ctx.users).await {
            if let Some(legacy) = ctx.legacy_token.as_deref() {
                // Constant-time comparison to prevent timing side-channels.
                let a = raw.as_bytes();
                let b = legacy.as_bytes();
                let len_match = a.len() == b.len();
                let mut diff: u8 = 0;
                for (x, y) in a.iter().zip(b.iter()) {
                    diff |= x ^ y;
                }
                if len_match && diff == 0 {
                    return Ok(Principal::LegacyAdmin);
                }
            }
        }

        Err((
            StatusCode::UNAUTHORIZED,
            "invalid or revoked token".to_string(),
        ))
    }
}
