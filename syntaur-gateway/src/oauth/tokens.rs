//! Persistent OAuth2 authorization_code tokens (schema v8).
//!
//! One row per (user_id, provider) pair. Tracks access_token,
//! refresh_token, expires_at, scope. Reads go via `get()` which
//! automatically refreshes if the access token is within 30s of
//! expiry.
//!
//! Unlike the `OAuthTokenCache` in `tools/openapi.rs` (which is for the
//! server-side client_credentials flow), this cache is **per-user**.
//! The authorization_code flow fundamentally associates tokens with a
//! human, so the caller must always supply a user_id.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct OAuthTokenRow {
    pub user_id: i64,
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scope: String,
    pub updated_at: i64,
}

pub struct AuthCodeTokenCache {
    db: Arc<Mutex<Connection>>,
    http: reqwest::Client,
}

impl AuthCodeTokenCache {
    pub fn open(db_path: PathBuf, http: reqwest::Client) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open oauth_tokens store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[oauth:tokens] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
            http,
        }))
    }

    /// Persist a fresh token pair (called from the OAuth callback handler
    /// after a successful code exchange). Upserts by (user_id, provider).
    pub async fn upsert(
        &self,
        user_id: i64,
        provider: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
        scope: &str,
    ) -> Result<(), String> {
        let db = self.db.lock().await;
        let now = Utc::now().timestamp();
        db.execute(
            "INSERT INTO oauth_tokens \
               (user_id, provider, access_token, refresh_token, expires_at, scope, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, provider) DO UPDATE SET \
               access_token = excluded.access_token, \
               refresh_token = COALESCE(excluded.refresh_token, refresh_token), \
               expires_at = excluded.expires_at, \
               scope = excluded.scope, \
               updated_at = excluded.updated_at",
            params![
                user_id,
                provider,
                access_token,
                refresh_token,
                expires_at,
                scope,
                now,
                now
            ],
        )
        .map_err(|e| format!("upsert oauth_tokens: {}", e))?;
        Ok(())
    }

    /// Fetch the raw row (no refresh attempt). Used by /api/oauth/status.
    pub async fn peek(
        &self,
        user_id: i64,
        provider: &str,
    ) -> Result<Option<OAuthTokenRow>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT user_id, provider, access_token, refresh_token, expires_at, scope, updated_at \
             FROM oauth_tokens WHERE user_id = ? AND provider = ?",
            params![user_id, provider],
            |r| {
                Ok(OAuthTokenRow {
                    user_id: r.get(0)?,
                    provider: r.get(1)?,
                    access_token: r.get(2)?,
                    refresh_token: r.get(3)?,
                    expires_at: r.get(4)?,
                    scope: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("peek oauth_tokens: {}", e))
    }

    /// Delete the row. Used by /api/oauth/disconnect.
    pub async fn delete(&self, user_id: i64, provider: &str) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute(
            "DELETE FROM oauth_tokens WHERE user_id = ? AND provider = ?",
            params![user_id, provider],
        )
        .map_err(|e| format!("delete oauth_tokens: {}", e))?;
        Ok(())
    }

    /// Fetch a ready-to-use access token, refreshing if we're within 30s
    /// of expiry. Returns an error if the user hasn't connected this
    /// provider yet or if the refresh attempt fails.
    ///
    /// Callers supply the refresh-url + client_id + client_secret because
    /// this cache is provider-agnostic — it doesn't know Google's token
    /// endpoint any more than it knows GitHub's.
    pub async fn get(
        &self,
        user_id: i64,
        provider: &str,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, String> {
        // Read current row
        let row = self
            .peek(user_id, provider)
            .await?
            .ok_or_else(|| {
                format!(
                    "user {} has not connected provider '{}'. \
                     Run /connect {} to authorize.",
                    user_id, provider, provider
                )
            })?;

        let now = Utc::now().timestamp();
        let needs_refresh = match row.expires_at {
            None => false, // provider doesn't report expiry, trust it
            Some(exp) => exp - now < 30,
        };

        if !needs_refresh {
            return Ok(row.access_token);
        }

        // Refresh required.
        let refresh = row.refresh_token.as_deref().ok_or_else(|| {
            format!(
                "access token for user {} provider '{}' is expired and no refresh token is available. \
                 Run /connect {} to reauthorize.",
                user_id, provider, provider
            )
        })?;

        let form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ];
        let resp = self
            .http
            .post(token_url)
            .form(&form)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("refresh request failed: {}", e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(
                "[oauth:tokens] refresh failed for user {} provider {}: {} {}",
                user_id, provider, status, body
            );
            return Err(format!("refresh {} returned {}", provider, status));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("refresh response parse: {}", e))?;

        let new_access = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "refresh response missing access_token".to_string())?
            .to_string();
        // Providers may or may not rotate the refresh token on each refresh.
        // When absent, keep the existing one (handled by COALESCE in upsert).
        let new_refresh = body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let expires_in = body.get("expires_in").and_then(|v| v.as_i64());
        let new_expires_at = expires_in.map(|s| now + s);
        let new_scope = body
            .get("scope")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or(row.scope);

        self.upsert(
            user_id,
            provider,
            &new_access,
            new_refresh.as_deref(),
            new_expires_at,
            &new_scope,
        )
        .await?;

        Ok(new_access)
    }
}
