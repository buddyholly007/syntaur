//! User + token + Telegram-link persistence.
//!
//! One store for all three: `users`, `user_api_tokens`, `user_telegram_links`.
//! Same `~/.syntaur/index.db` file as everything else, schema v7.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::info;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// How user rows look coming back from the store. No token hash exposed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub disabled: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiTokenMeta {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// What the store returns when a token hash lookup succeeds.
#[derive(Debug, Clone)]
pub struct ResolvedToken {
    pub user_id: i64,
    pub user_name: String,
    pub token_id: i64,
}

pub struct UserStore {
    db: Arc<Mutex<Connection>>,
}

impl UserStore {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open user store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[auth:users] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    // ── user CRUD ─────────────────────────────────────────────────────────

    /// Create a new user. Fails if the name is already taken.
    pub async fn create_user(&self, name: &str) -> Result<User, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO users (name, created_at, disabled) VALUES (?, ?, 0)",
            params![name, now],
        )
        .map_err(|e| format!("create user '{}': {}", name, e))?;
        let id = db.last_insert_rowid();
        Ok(User {
            id,
            name: name.to_string(),
            created_at: now,
            disabled: false,
        })
    }

    /// Return the user with the given id, or None.
    pub async fn get_user(&self, id: i64) -> Result<Option<User>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT id, name, created_at, disabled FROM users WHERE id = ?",
            params![id],
            |r| {
                Ok(User {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    disabled: r.get::<_, i64>(3)? != 0,
                })
            },
        )
        .optional()
        .map_err(|e| format!("get_user: {}", e))
    }

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare("SELECT id, name, created_at, disabled FROM users ORDER BY id")
            .map_err(|e| format!("list_users prepare: {}", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(User {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    disabled: r.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|e| format!("list_users query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list_users row: {}", e))?);
        }
        Ok(out)
    }

    /// Returns true if there are zero users in the table. Used by the
    /// legacy admin fallback: an empty users table means the system is
    /// still running in "pre-Item-3" mode and the legacy global token is
    /// still authoritative.
    pub async fn is_empty(&self) -> Result<bool, String> {
        let db = self.db.lock().await;
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(|e| format!("users count: {}", e))?;
        Ok(n == 0)
    }

    // ── tokens ────────────────────────────────────────────────────────────

    /// Mint a new API token for an existing user. The **raw** token is
    /// returned exactly once; only the hash is persisted.
    ///
    /// Token format: `ocp_` + 32 bytes of base64url-encoded randomness.
    /// ~256 bits of entropy, uniform random, SHA256 of it as the storage
    /// key is indistinguishable from random to any attacker.
    pub async fn mint_token(&self, user_id: i64, name: &str) -> Result<String, String> {
        use rand::RngCore;

        // Verify the user exists first; we'd rather fail loudly than
        // insert an orphaned token that can't be used.
        if self.get_user(user_id).await?.is_none() {
            return Err(format!("user {} does not exist", user_id));
        }

        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let raw = format!(
            "ocp_{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                bytes
            )
        );
        let hash = hash_token(&raw);
        let now = Utc::now().timestamp();

        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_api_tokens (user_id, token_hash, name, created_at) \
             VALUES (?, ?, ?, ?)",
            params![user_id, hash, name, now],
        )
        .map_err(|e| format!("mint_token: {}", e))?;

        Ok(raw)
    }

    /// Look up a raw token against the store. Returns Some(ResolvedToken) on
    /// hit, None on miss. Revoked tokens are treated as misses. Updates
    /// `last_used_at` as a side effect.
    pub async fn resolve_token(&self, raw_token: &str) -> Result<Option<ResolvedToken>, String> {
        let hash = hash_token(raw_token);
        let db = self.db.lock().await;
        let row = db
            .query_row(
                "SELECT t.id, t.user_id, u.name, t.revoked_at \
                 FROM user_api_tokens t \
                 JOIN users u ON u.id = t.user_id \
                 WHERE t.token_hash = ?",
                params![hash],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("resolve_token: {}", e))?;

        let Some((token_id, user_id, user_name, revoked_at)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        let now = Utc::now().timestamp();
        let _ = db.execute(
            "UPDATE user_api_tokens SET last_used_at = ? WHERE id = ?",
            params![now, token_id],
        );
        Ok(Some(ResolvedToken {
            user_id,
            user_name,
            token_id,
        }))
    }

    pub async fn revoke_token(&self, token_id: i64) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "UPDATE user_api_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, token_id],
        )
        .map_err(|e| format!("revoke_token: {}", e))?;
        Ok(())
    }

    pub async fn list_tokens_for_user(&self, user_id: i64) -> Result<Vec<ApiTokenMeta>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT id, user_id, name, created_at, last_used_at, revoked_at \
                 FROM user_api_tokens WHERE user_id = ? ORDER BY id",
            )
            .map_err(|e| format!("list_tokens prepare: {}", e))?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                Ok(ApiTokenMeta {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    name: r.get(2)?,
                    created_at: r.get(3)?,
                    last_used_at: r.get(4)?,
                    revoked_at: r.get(5)?,
                })
            })
            .map_err(|e| format!("list_tokens query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list_tokens row: {}", e))?);
        }
        Ok(out)
    }

    // ── Telegram chat links ───────────────────────────────────────────────

    /// Bind a (bot_token, chat_id) pair to a user. Idempotent: if the
    /// pair is already linked to the same user, this is a no-op; if it's
    /// linked to a different user, this errors out rather than silently
    /// reassigning.
    pub async fn link_telegram(
        &self,
        user_id: i64,
        bot_token: &str,
        chat_id: i64,
    ) -> Result<(), String> {
        let db = self.db.lock().await;
        // Check for an existing link to a different user.
        let existing: Option<i64> = db
            .query_row(
                "SELECT user_id FROM user_telegram_links WHERE bot_token = ? AND chat_id = ?",
                params![bot_token, chat_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("link_telegram check: {}", e))?;
        if let Some(existing_uid) = existing {
            if existing_uid == user_id {
                return Ok(());
            }
            return Err(format!(
                "chat {} already linked to user {}",
                chat_id, existing_uid
            ));
        }
        let now = Utc::now().timestamp();
        db.execute(
            "INSERT INTO user_telegram_links (user_id, bot_token, chat_id, created_at) \
             VALUES (?, ?, ?, ?)",
            params![user_id, bot_token, chat_id, now],
        )
        .map_err(|e| format!("link_telegram insert: {}", e))?;
        Ok(())
    }

    /// Resolve a (bot_token, chat_id) pair to a user_id, if linked.
    pub async fn resolve_telegram_chat(
        &self,
        bot_token: &str,
        chat_id: i64,
    ) -> Result<Option<i64>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT user_id FROM user_telegram_links WHERE bot_token = ? AND chat_id = ?",
            params![bot_token, chat_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("resolve_telegram_chat: {}", e))
    }
}

/// Hash a raw bearer token for storage + lookup.
///
/// Single-pass SHA256 is sufficient here because the raw token has
/// ~256 bits of uniform-random entropy — brute force is infeasible and
/// bcrypt/argon2-style work factors only matter for low-entropy secrets
/// like user-chosen passwords. base64url-no-pad gives us a stable
/// text-column form we can UNIQUE-constrain without pulling in `hex`.
pub fn hash_token(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, digest)
}
