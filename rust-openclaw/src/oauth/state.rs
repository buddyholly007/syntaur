//! In-memory state cache for in-flight OAuth2 authorization requests.
//!
//! The state value is a random string we mint in `/api/oauth/start`,
//! embed in the authorization URL as `?state=<value>`, and expect back
//! in the callback. It serves two purposes:
//!
//! 1. **CSRF protection** — the callback must present the same state
//!    we minted, so an attacker can't trigger an unsolicited code
//!    exchange.
//! 2. **Session carrier** — it ties the callback back to the
//!    originating user + provider + PKCE verifier without stuffing any
//!    of those into the URL.
//!
//! Entries TTL out at 10 minutes. If the user doesn't click through in
//! time, the row disappears; a future callback with a stale state
//! returns 400.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const STATE_TTL_SECS: u64 = 600; // 10 minutes

#[derive(Debug, Clone)]
pub struct PendingAuthEntry {
    pub user_id: i64,
    pub provider: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub created_at: Instant,
}

pub struct OAuthStateCache {
    inner: Mutex<HashMap<String, PendingAuthEntry>>,
}

impl OAuthStateCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Insert a new pending authorization. Call from /api/oauth/start.
    pub async fn insert(&self, state_value: String, entry: PendingAuthEntry) {
        let mut g = self.inner.lock().await;
        // Opportunistic GC of stale entries so the map doesn't grow
        // unbounded if a user starts many flows without finishing them.
        g.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(STATE_TTL_SECS));
        g.insert(state_value, entry);
    }

    /// Pop an entry by state value. Returns None if missing or expired.
    /// "Pop" is important — state is single-use; even if the callback
    /// retries, the second attempt must fail.
    pub async fn take(&self, state_value: &str) -> Option<PendingAuthEntry> {
        let mut g = self.inner.lock().await;
        let entry = g.remove(state_value)?;
        if entry.created_at.elapsed() >= Duration::from_secs(STATE_TTL_SECS) {
            return None;
        }
        Some(entry)
    }
}
