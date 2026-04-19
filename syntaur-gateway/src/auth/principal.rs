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
    LegacyAdmin,
    /// Real user row from the `users` table.
    User {
        id: i64,
        name: String,
        role: String,
        /// Empty = unscoped (full access). Non-empty = token is limited to
        /// these scopes; endpoints that don't accept one of them 401.
        scopes: Vec<String>,
    },
}

impl Principal {
    /// The effective user_id used to stamp writes and filter reads.
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

    /// True iff the caller can hit admin endpoints.
    /// Role-based: admin role or LegacyAdmin.
    pub fn is_admin(&self) -> bool {
        match self {
            Self::LegacyAdmin => true,
            Self::User { role, .. } => role == "admin",
        }
    }

    /// True iff this principal's token is unscoped (web session, admin CLI,
    /// or legacy global token). Scoped tokens (MACE sessions etc.) return
    /// false. The mint endpoint uses this to refuse scoped tokens from
    /// minting further scoped tokens.
    pub fn is_unscoped(&self) -> bool {
        match self {
            Self::LegacyAdmin => true,
            Self::User { scopes, .. } => scopes.is_empty(),
        }
    }

    /// Does this principal's token grant the named scope? Unscoped tokens
    /// always return true (backward compat — every existing web session
    /// keeps working untouched). Scoped tokens match the literal scope name
    /// or the wildcard `*`.
    pub fn has_scope(&self, scope: &str) -> bool {
        match self {
            Self::LegacyAdmin => true,
            Self::User { scopes, .. } => {
                scopes.is_empty()
                    || scopes.iter().any(|s| s == scope || s == "*")
            }
        }
    }

    /// `has_scope` as a `Result<_, StatusCode>` so handlers can `?`-propagate.
    pub fn require_scope(&self, scope: &str) -> Result<(), axum::http::StatusCode> {
        if self.has_scope(scope) { Ok(()) } else { Err(axum::http::StatusCode::UNAUTHORIZED) }
    }

    /// Row-level filter key for stores that support per-user scoping.
    ///
    /// `None`  = no filter (admin, see everything).
    /// `Some(uid)` = `WHERE user_id = uid`.
    pub fn scope(&self) -> Option<i64> {
        match self {
            Self::LegacyAdmin => None,
            Self::User { role, .. } if role == "admin" => None,
            Self::User { id, .. } => Some(*id),
        }
    }

    /// Sharing-mode-aware scope. In "shared" mode, all users see all data.
    /// In "isolated" mode, non-admin users see only their own data.
    pub fn scope_with_sharing(&self, sharing_mode: &str) -> Option<i64> {
        match self {
            Self::LegacyAdmin => None,
            Self::User { role, .. } if role == "admin" => None,
            _ if sharing_mode == "shared" => None,
            Self::User { id, .. } => Some(*id),
        }
    }

    /// The user's role string.
    pub fn role(&self) -> &str {
        match self {
            Self::LegacyAdmin => "admin",
            Self::User { role, .. } => role.as_str(),
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

/// Pieces the extractor needs from AppState.
#[derive(Clone)]
pub struct AuthContext {
    pub users: Arc<UserStore>,
    pub legacy_token: Option<String>,
    pub allow_query_string_tokens: bool,
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
            let qs = Query::<TokenQuery>::try_from_uri(&parts.uri)
                .ok()
                .and_then(|Query(q)| q.token);
            let is_qs = qs.is_some();
            (qs, is_qs)
        };

        if via_query_string {
            if !ctx.allow_query_string_tokens {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "Query string tokens are disabled. Use Authorization: Bearer header.".to_string(),
                ));
            }
            log::warn!(
                "[auth] DEPRECATED: token passed via ?token= query string. \
                 Use Authorization: Bearer <token> header instead."
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
                    role: resolved.user_role,
                    scopes: resolved.scopes
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                });
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("[auth] token lookup error: {}", e);
            }
        }

        // 2. Legacy admin fallback — only when users table is empty.
        if legacy_admin_enabled(&ctx.users).await {
            if let Some(legacy) = ctx.legacy_token.as_deref() {
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
