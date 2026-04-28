//! User + token + Telegram-link + password + invite persistence.
//!
//! One store for all: `users`, `user_api_tokens`, `user_telegram_links`,
//! `user_invites`, `user_agents`. Same `~/.syntaur/index.db` file.

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
    pub role: String,
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
    pub user_role: String,
    pub token_id: i64,
    /// Comma-separated scope list. Empty = unscoped (full access — this is
    /// every web-session token). Non-empty = the token can only reach
    /// endpoints that accept one of these scopes. See `Principal::has_scope`.
    pub scopes: String,
}

/// A user-owned agent definition.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UserAgent {
    pub id: i64,
    pub user_id: i64,
    pub agent_id: String,
    pub display_name: String,
    pub base_agent: String,
    pub system_prompt: Option<String>,
    pub workspace: Option<String>,
    pub tool_profile: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// Added in schema v40: agent is eligible for the main-thread picker and
    /// gets Peter/Kyron-tier privileges (cross-module reads + handoff).
    #[serde(default)]
    pub is_main_thread: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<String>,
    #[serde(default)]
    pub imported_from: Option<String>,
}

/// An invite code record.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Invite {
    pub id: i64,
    pub code: String,
    pub created_by: i64,
    pub name_hint: Option<String>,
    pub role: String,
    pub expires_at: i64,
    pub sharing_preset: Option<String>,
    pub consumed_at: Option<i64>,
    pub consumed_by: Option<i64>,
    pub created_at: i64,
}

/// A sharing grant record.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SharingGrant {
    pub id: i64,
    pub grantor_user_id: i64,
    pub grantee_user_id: i64,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub created_at: i64,
}

/// A personality document for shaping a user's AI agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonalityDoc {
    pub id: i64,
    pub user_id: i64,
    pub agent_id: String,
    pub doc_type: String,
    pub title: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
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

    /// Create a new user with an optional password and role.
    pub async fn create_user(&self, name: &str) -> Result<User, String> {
        self.create_user_full(name, "user", None).await
    }

    pub async fn create_user_full(
        &self,
        name: &str,
        role: &str,
        password_hash: Option<&str>,
    ) -> Result<User, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO users (name, role, password_hash, created_at, disabled) VALUES (?, ?, ?, ?, 0)",
            params![name, role, password_hash, now],
        )
        .map_err(|e| format!("create user '{}': {}", name, e))?;
        let id = db.last_insert_rowid();
        // Seed default persona agents for the new user
        if let Err(e) = crate::agents::defaults::clone_for_user(&db, id) {
            log::warn!("failed to clone default agents for user {}: {}", id, e);
        }
        Ok(User {
            id,
            name: name.to_string(),
            role: role.to_string(),
            created_at: now,
            disabled: false,
        })
    }

    pub async fn get_user(&self, id: i64) -> Result<Option<User>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT id, name, COALESCE(role,'user'), created_at, disabled FROM users WHERE id = ?",
            params![id],
            |r| {
                Ok(User {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    role: r.get(2)?,
                    created_at: r.get(3)?,
                    disabled: r.get::<_, i64>(4)? != 0,
                })
            },
        )
        .optional()
        .map_err(|e| format!("get_user: {}", e))
    }

    pub async fn get_user_by_name(&self, name: &str) -> Result<Option<User>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT id, name, COALESCE(role,'user'), created_at, disabled FROM users WHERE name = ?",
            params![name],
            |r| {
                Ok(User {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    role: r.get(2)?,
                    created_at: r.get(3)?,
                    disabled: r.get::<_, i64>(4)? != 0,
                })
            },
        )
        .optional()
        .map_err(|e| format!("get_user_by_name: {}", e))
    }

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare("SELECT id, name, COALESCE(role,'user'), created_at, disabled FROM users ORDER BY id")
            .map_err(|e| format!("list_users prepare: {}", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(User {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    role: r.get(2)?,
                    created_at: r.get(3)?,
                    disabled: r.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(|e| format!("list_users query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list_users row: {}", e))?);
        }
        Ok(out)
    }

    pub async fn update_user_role(&self, id: i64, role: &str) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("UPDATE users SET role = ? WHERE id = ?", params![role, id])
            .map_err(|e| format!("update_user_role: {}", e))?;
        Ok(())
    }

    pub async fn disable_user(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("UPDATE users SET disabled = 1 WHERE id = ?", params![id])
            .map_err(|e| format!("disable_user: {}", e))?;
        Ok(())
    }

    pub async fn enable_user(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("UPDATE users SET disabled = 0 WHERE id = ?", params![id])
            .map_err(|e| format!("enable_user: {}", e))?;
        Ok(())
    }

    pub async fn delete_user(&self, id: i64) -> Result<(), String> {
        if id == 1 {
            return Err("cannot delete the primary admin user".to_string());
        }
        let db = self.db.lock().await;
        db.execute("DELETE FROM users WHERE id = ?", params![id])
            .map_err(|e| format!("delete_user: {}", e))?;
        Ok(())
    }

    pub async fn is_empty(&self) -> Result<bool, String> {
        let db = self.db.lock().await;
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(|e| format!("users count: {}", e))?;
        Ok(n == 0)
    }

    // ── passwords ────────────────────────────────────────────────────────

    pub async fn set_password(&self, user_id: i64, password: &str) -> Result<(), String> {
        let hash = hash_password(password)?;
        let db = self.db.lock().await;
        db.execute(
            "UPDATE users SET password_hash = ? WHERE id = ?",
            params![hash, user_id],
        )
        .map_err(|e| format!("set_password: {}", e))?;
        Ok(())
    }

    pub async fn verify_password(&self, user_id: i64, password: &str) -> Result<bool, String> {
        let db = self.db.lock().await;
        let hash: Option<String> = db
            .query_row(
                "SELECT password_hash FROM users WHERE id = ? AND disabled = 0",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("verify_password: {}", e))?
            .flatten();
        match hash {
            Some(h) => Ok(verify_password_hash(password, &h)),
            None => Ok(false),
        }
    }

    pub async fn has_password(&self, user_id: i64) -> Result<bool, String> {
        let db = self.db.lock().await;
        let hash: Option<String> = db
            .query_row(
                "SELECT password_hash FROM users WHERE id = ?",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("has_password: {}", e))?
            .flatten();
        Ok(hash.is_some())
    }

    // ── tokens ────────────────────────────────────────────────────────────

    pub async fn mint_token(&self, user_id: i64, name: &str) -> Result<String, String> {
        self.mint_token_with_expiry(user_id, name, None).await
    }

    pub async fn mint_token_with_expiry(&self, user_id: i64, name: &str, expiry_hours: Option<u64>) -> Result<String, String> {
        self.mint_token_scoped(user_id, name, "", expiry_hours).await
    }

    /// Mint a short-TTL scoped token (MACE sessions etc.). Scopes is a
    /// comma-separated list of scope names — see `Principal::has_scope`.
    /// Empty string = unscoped (full access).
    pub async fn mint_token_scoped(&self, user_id: i64, name: &str, scopes: &str, expiry_hours: Option<u64>) -> Result<String, String> {
        use rand::RngCore;

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
        let expires_at = expiry_hours.map(|h| now + (h as i64) * 3600);

        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_api_tokens (user_id, token_hash, name, created_at, expires_at, scopes) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![user_id, hash, name, now, expires_at, scopes],
        )
        .map_err(|e| format!("mint_token: {}", e))?;

        Ok(raw)
    }

    pub async fn resolve_token(&self, raw_token: &str) -> Result<Option<ResolvedToken>, String> {
        let hash = hash_token(raw_token);
        let db = self.db.lock().await;
        let row = db
            .query_row(
                "SELECT t.id, t.user_id, u.name, COALESCE(u.role,'user'), t.revoked_at, t.expires_at, COALESCE(t.scopes, '') \
                 FROM user_api_tokens t \
                 JOIN users u ON u.id = t.user_id \
                 WHERE t.token_hash = ? AND u.disabled = 0",
                params![hash],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("resolve_token: {}", e))?;

        let Some((token_id, user_id, user_name, user_role, revoked_at, expires_at, scopes)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(exp) = expires_at {
            if Utc::now().timestamp() > exp {
                return Ok(None);
            }
        }
        let now = Utc::now().timestamp();
        let _ = db.execute(
            "UPDATE user_api_tokens SET last_used_at = ? WHERE id = ?",
            params![now, token_id],
        );
        Ok(Some(ResolvedToken {
            user_id,
            user_name,
            user_role,
            token_id,
            scopes,
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

    // ── invites ───────────────────────────────────────────────────────────

    pub async fn create_invite(
        &self,
        created_by: i64,
        name_hint: Option<&str>,
        role: &str,
        expires_hours: u64,
        sharing_preset: Option<&str>,
    ) -> Result<Invite, String> {
        use rand::RngCore;
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        let code = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            bytes,
        );
        let now = Utc::now().timestamp();
        let expires_at = now + (expires_hours as i64) * 3600;

        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_invites (code, created_by, name_hint, role, expires_at, sharing_preset, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![code, created_by, name_hint, role, expires_at, sharing_preset, now],
        )
        .map_err(|e| format!("create_invite: {}", e))?;
        let id = db.last_insert_rowid();
        Ok(Invite {
            id,
            code,
            created_by,
            name_hint: name_hint.map(String::from),
            role: role.to_string(),
            expires_at,
            sharing_preset: sharing_preset.map(String::from),
            consumed_at: None,
            consumed_by: None,
            created_at: now,
        })
    }

    pub async fn consume_invite(&self, code: &str, user_id: i64) -> Result<Invite, String> {
        let db = self.db.lock().await;
        let invite: Invite = db
            .query_row(
                "SELECT id, code, created_by, name_hint, role, expires_at, sharing_preset, consumed_at, consumed_by, created_at \
                 FROM user_invites WHERE code = ?",
                params![code],
                |r| {
                    Ok(Invite {
                        id: r.get(0)?,
                        code: r.get(1)?,
                        created_by: r.get(2)?,
                        name_hint: r.get(3)?,
                        role: r.get(4)?,
                        expires_at: r.get(5)?,
                        sharing_preset: r.get(6)?,
                        consumed_at: r.get(7)?,
                        consumed_by: r.get(8)?,
                        created_at: r.get(9)?,
                    })
                },
            )
            .map_err(|e| format!("invite not found: {}", e))?;

        if invite.consumed_at.is_some() {
            return Err("invite already used".to_string());
        }
        if Utc::now().timestamp() > invite.expires_at {
            return Err("invite expired".to_string());
        }

        let now = Utc::now().timestamp();
        db.execute(
            "UPDATE user_invites SET consumed_at = ?, consumed_by = ? WHERE id = ?",
            params![now, user_id, invite.id],
        )
        .map_err(|e| format!("consume_invite: {}", e))?;

        Ok(invite)
    }

    pub async fn list_invites(&self) -> Result<Vec<Invite>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT id, code, created_by, name_hint, role, expires_at, sharing_preset, consumed_at, consumed_by, created_at \
                 FROM user_invites ORDER BY id DESC",
            )
            .map_err(|e| format!("list_invites: {}", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Invite {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    created_by: r.get(2)?,
                    name_hint: r.get(3)?,
                    role: r.get(4)?,
                    expires_at: r.get(5)?,
                    sharing_preset: r.get(6)?,
                    consumed_at: r.get(7)?,
                    consumed_by: r.get(8)?,
                    created_at: r.get(9)?,
                })
            })
            .map_err(|e| format!("list_invites query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list_invites row: {}", e))?);
        }
        Ok(out)
    }

    // ── user agents ──────────────────────────────────────────────────────

    pub async fn list_user_agents(&self, user_id: i64) -> Result<Vec<UserAgent>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT id, user_id, agent_id, display_name, base_agent, system_prompt, \
                 workspace, tool_profile, enabled, created_at, updated_at, \
                 is_main_thread, description, avatar_color, imported_from \
                 FROM user_agents WHERE user_id = ? ORDER BY agent_id",
            )
            .map_err(|e| format!("list_user_agents: {}", e))?;
        let rows = stmt
            .query_map(params![user_id], map_user_agent)
            .map_err(|e| format!("list_user_agents query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list_user_agents row: {}", e))?);
        }
        Ok(out)
    }

    pub async fn get_user_agent(&self, user_id: i64, agent_id: &str) -> Result<Option<UserAgent>, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT id, user_id, agent_id, display_name, base_agent, system_prompt, \
             workspace, tool_profile, enabled, created_at, updated_at, \
             is_main_thread, description, avatar_color, imported_from \
             FROM user_agents WHERE user_id = ? AND agent_id = ?",
            params![user_id, agent_id],
            map_user_agent,
        )
        .optional()
        .map_err(|e| format!("get_user_agent: {}", e))
    }

    /// Self-heal seed: if `user_agents` has zero rows for this user, run
    /// `clone_for_user` (idempotent) so the chat surface stops falling
    /// through to the unrenamed `module_agent_defaults` defaults. Closes the
    /// gap that left Sean with an empty user_agents table — `clone_for_user`
    /// is wired into user-create, but pre-existing users (or any user whose
    /// row got wiped) had no recovery path. Safe to call on every chat.
    pub async fn ensure_user_agents_seeded(&self, user_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        let n: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM user_agents WHERE user_id = ?",
                params![user_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("count user_agents: {}", e))?;
        if n > 0 {
            return Ok(());
        }
        crate::agents::defaults::clone_for_user(&db, user_id)
            .map(|_| ())
            .map_err(|e| format!("clone_for_user: {}", e))
    }

    /// Create a new user-owned agent with the full set of v40 fields. Used
    /// by the Settings → Agents page (both manual create + file-import
    /// flows). `agent_id` must be unique per user; the caller is responsible
    /// for slugifying a user-provided name if needed.
    pub async fn create_custom_agent(
        &self,
        user_id: i64,
        agent_id: &str,
        display_name: &str,
        base_agent: &str,
        description: Option<&str>,
        system_prompt: Option<&str>,
        is_main_thread: bool,
        avatar_color: Option<&str>,
        imported_from: Option<&str>,
    ) -> Result<UserAgent, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_agents (user_id, agent_id, display_name, base_agent, \
                                       system_prompt, description, avatar_color, \
                                       imported_from, is_main_thread, \
                                       created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                user_id, agent_id, display_name, base_agent,
                system_prompt, description, avatar_color,
                imported_from, is_main_thread as i64,
                now, now,
            ],
        )
        .map_err(|e| format!("create_custom_agent: {}", e))?;

        db.query_row(
            "SELECT id, user_id, agent_id, display_name, base_agent, system_prompt, \
             workspace, tool_profile, enabled, created_at, updated_at, \
             is_main_thread, description, avatar_color, imported_from \
             FROM user_agents WHERE user_id = ? AND agent_id = ?",
            params![user_id, agent_id],
            map_user_agent,
        )
        .map_err(|e| format!("create_custom_agent load: {}", e))
    }

    pub async fn create_user_agent(
        &self,
        user_id: i64,
        agent_id: &str,
        display_name: &str,
        base_agent: &str,
        system_prompt: Option<&str>,
    ) -> Result<UserAgent, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_agents (user_id, agent_id, display_name, base_agent, system_prompt, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![user_id, agent_id, display_name, base_agent, system_prompt, now, now],
        )
        .map_err(|e| format!("create_user_agent: {}", e))?;
        let id = db.last_insert_rowid();
        Ok(UserAgent {
            id,
            user_id,
            agent_id: agent_id.to_string(),
            display_name: display_name.to_string(),
            base_agent: base_agent.to_string(),
            system_prompt: system_prompt.map(String::from),
            workspace: None,
            tool_profile: "full".to_string(),
            enabled: true,
            created_at: now,
            updated_at: now,
            is_main_thread: false,
            description: None,
            avatar_color: None,
            imported_from: None,
        })
    }

    pub async fn update_user_agent(
        &self,
        user_id: i64,
        agent_id: &str,
        display_name: Option<&str>,
        system_prompt: Option<Option<&str>>,
        enabled: Option<bool>,
    ) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        if let Some(dn) = display_name {
            db.execute(
                "UPDATE user_agents SET display_name = ?, updated_at = ? WHERE user_id = ? AND agent_id = ?",
                params![dn, now, user_id, agent_id],
            )
            .map_err(|e| format!("update_user_agent display_name: {}", e))?;
        }
        if let Some(sp) = system_prompt {
            db.execute(
                "UPDATE user_agents SET system_prompt = ?, updated_at = ? WHERE user_id = ? AND agent_id = ?",
                params![sp, now, user_id, agent_id],
            )
            .map_err(|e| format!("update_user_agent system_prompt: {}", e))?;
        }
        if let Some(en) = enabled {
            db.execute(
                "UPDATE user_agents SET enabled = ?, updated_at = ? WHERE user_id = ? AND agent_id = ?",
                params![en as i64, now, user_id, agent_id],
            )
            .map_err(|e| format!("update_user_agent enabled: {}", e))?;
        }
        Ok(())
    }

    pub async fn delete_user_agent(&self, user_id: i64, agent_id: &str) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute(
            "DELETE FROM user_agents WHERE user_id = ? AND agent_id = ?",
            params![user_id, agent_id],
        )
        .map_err(|e| format!("delete_user_agent: {}", e))?;
        Ok(())
    }

    // ── data location ─────────────────────────────────────────────────

    pub async fn get_data_dir(&self, user_id: i64) -> Option<String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT data_dir FROM users WHERE id = ?",
            params![user_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub async fn set_data_dir(&self, user_id: i64, path: &str) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute(
            "UPDATE users SET data_dir = ? WHERE id = ?",
            params![path, user_id],
        )
        .map_err(|e| format!("set_data_dir: {e}"))?;
        Ok(())
    }

    // ── sharing grants ─────────────────────────────────────────────────

    /// Get the list of user_ids whose data `requesting_user_id` can see
    /// for a given resource_type + resource_id. Returns None = no filter (see all).
    pub async fn visible_user_ids(
        &self,
        requesting_user_id: i64,
        sharing_mode: &str,
        resource_type: &str,
        resource_id: Option<&str>,
    ) -> Option<Vec<i64>> {
        match sharing_mode {
            "shared" => None, // everyone sees everything
            "selective" => {
                let db = self.db.lock().await;
                let mut ids = vec![requesting_user_id];
                // Find all grantors who have granted this resource to the requester
                let mut stmt = db.prepare(
                    "SELECT DISTINCT grantor_user_id FROM sharing_grants \
                     WHERE grantee_user_id = ? AND resource_type = ? \
                     AND (resource_id = ? OR resource_id = '*' OR resource_id IS NULL)"
                ).ok();
                if let Some(ref mut s) = stmt {
                    let rid = resource_id.unwrap_or("*");
                    if let Ok(rows) = s.query_map(params![requesting_user_id, resource_type, rid], |r| r.get::<_, i64>(0)) {
                        for r in rows.flatten() {
                            if !ids.contains(&r) {
                                ids.push(r);
                            }
                        }
                    }
                }
                // Also include user_id=0 (system/shared data)
                if !ids.contains(&0) {
                    ids.push(0);
                }
                Some(ids)
            }
            _ => {
                // "isolated" — own data + system data only
                Some(vec![requesting_user_id, 0])
            }
        }
    }

    pub async fn list_grants_for_user(&self, grantee_user_id: i64) -> Result<Vec<SharingGrant>, String> {
        let db = self.db.lock().await;
        let mut stmt = db.prepare(
            "SELECT id, grantor_user_id, grantee_user_id, resource_type, resource_id, created_at \
             FROM sharing_grants WHERE grantee_user_id = ? ORDER BY resource_type, resource_id"
        ).map_err(|e| format!("list_grants: {e}"))?;
        let rows = stmt.query_map(params![grantee_user_id], |r| {
            Ok(SharingGrant {
                id: r.get(0)?,
                grantor_user_id: r.get(1)?,
                grantee_user_id: r.get(2)?,
                resource_type: r.get(3)?,
                resource_id: r.get(4)?,
                created_at: r.get(5)?,
            })
        }).map_err(|e| format!("list_grants query: {e}"))?;
        let mut out = Vec::new();
        for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
        Ok(out)
    }

    /// Bulk-set grants for a grantee from a grantor. Replaces all existing grants
    /// from that grantor to that grantee.
    pub async fn set_grants(
        &self,
        grantor_user_id: i64,
        grantee_user_id: i64,
        grants: &[(String, Option<String>)], // (resource_type, resource_id)
    ) -> Result<usize, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        // Remove old grants from this grantor to this grantee
        db.execute(
            "DELETE FROM sharing_grants WHERE grantor_user_id = ? AND grantee_user_id = ?",
            params![grantor_user_id, grantee_user_id],
        ).map_err(|e| format!("delete old grants: {e}"))?;
        // Insert new grants
        let mut count = 0;
        for (rt, rid) in grants {
            db.execute(
                "INSERT OR IGNORE INTO sharing_grants (grantor_user_id, grantee_user_id, resource_type, resource_id, created_at) \
                 VALUES (?, ?, ?, ?, ?)",
                params![grantor_user_id, grantee_user_id, rt, rid.as_deref(), now],
            ).map_err(|e| format!("insert grant: {e}"))?;
            count += 1;
        }
        Ok(count)
    }

    pub async fn delete_grant(&self, grant_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("DELETE FROM sharing_grants WHERE id = ?", params![grant_id])
            .map_err(|e| format!("delete_grant: {e}"))?;
        Ok(())
    }

    /// Expand a sharing preset JSON string into grants.
    /// Preset format: [{"resource_type":"oauth","resource_id":"*"}, ...]
    pub async fn apply_sharing_preset(
        &self,
        grantor_user_id: i64,
        grantee_user_id: i64,
        preset_json: &str,
    ) -> Result<usize, String> {
        let entries: Vec<serde_json::Value> = serde_json::from_str(preset_json)
            .map_err(|e| format!("parse preset: {e}"))?;
        let grants: Vec<(String, Option<String>)> = entries.iter().filter_map(|v| {
            let rt = v.get("resource_type")?.as_str()?.to_string();
            let rid = v.get("resource_id").and_then(|r| r.as_str()).map(String::from);
            Some((rt, rid))
        }).collect();
        self.set_grants(grantor_user_id, grantee_user_id, &grants).await
    }

    // ── personality docs ──────────────────────────────────────────────────

    pub async fn list_personality_docs(&self, user_id: i64, agent_id: &str) -> Result<Vec<PersonalityDoc>, String> {
        let db = self.db.lock().await;
        let mut stmt = db.prepare(
            "SELECT id, user_id, agent_id, doc_type, title, content, created_at, updated_at \
             FROM user_personality_docs WHERE user_id = ? AND agent_id = ? ORDER BY doc_type, id"
        ).map_err(|e| format!("list_personality: {e}"))?;
        let rows = stmt.query_map(params![user_id, agent_id], |r| {
            Ok(PersonalityDoc {
                id: r.get(0)?,
                user_id: r.get(1)?,
                agent_id: r.get(2)?,
                doc_type: r.get(3)?,
                title: r.get(4)?,
                content: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        }).map_err(|e| format!("query: {e}"))?;
        let mut out = Vec::new();
        for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
        Ok(out)
    }

    pub async fn create_personality_doc(
        &self,
        user_id: i64,
        agent_id: &str,
        doc_type: &str,
        title: &str,
        content: &str,
    ) -> Result<PersonalityDoc, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO user_personality_docs (user_id, agent_id, doc_type, title, content, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![user_id, agent_id, doc_type, title, content, now, now],
        ).map_err(|e| format!("create_personality: {e}"))?;
        let id = db.last_insert_rowid();
        Ok(PersonalityDoc {
            id, user_id, agent_id: agent_id.to_string(),
            doc_type: doc_type.to_string(), title: title.to_string(),
            content: content.to_string(), created_at: now, updated_at: now,
        })
    }

    pub async fn update_personality_doc(&self, id: i64, user_id: i64, title: Option<&str>, content: Option<&str>) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        if let Some(t) = title {
            db.execute("UPDATE user_personality_docs SET title = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                params![t, now, id, user_id]).map_err(|e| format!("update title: {e}"))?;
        }
        if let Some(c) = content {
            db.execute("UPDATE user_personality_docs SET content = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                params![c, now, id, user_id]).map_err(|e| format!("update content: {e}"))?;
        }
        Ok(())
    }

    pub async fn delete_personality_doc(&self, id: i64, user_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("DELETE FROM user_personality_docs WHERE id = ? AND user_id = ?", params![id, user_id])
            .map_err(|e| format!("delete_personality: {e}"))?;
        Ok(())
    }

    /// Get combined personality text for a user+agent, truncated to max_chars.
    pub async fn personality_prompt(&self, user_id: i64, agent_id: &str, max_chars: usize) -> String {
        let docs = self.list_personality_docs(user_id, agent_id).await.unwrap_or_default();
        if docs.is_empty() { return String::new(); }
        let mut parts = Vec::new();
        let mut total = 0;
        for doc in &docs {
            let header = format!("[{}] {}", doc.doc_type, doc.title);
            let entry = format!("{}\n{}", header, doc.content);
            if total + entry.len() > max_chars {
                let remaining = max_chars.saturating_sub(total);
                if remaining > 50 {
                    parts.push(format!("{}...[truncated]", &entry[..remaining.min(entry.len())]));
                }
                break;
            }
            total += entry.len();
            parts.push(entry);
        }
        if parts.is_empty() { return String::new(); }
        format!("# About this user\n\n{}", parts.join("\n\n"))
    }

    // ── onboarding ───────────────────────────────────────────────────────

    pub async fn is_onboarding_complete(&self, user_id: i64) -> bool {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT COALESCE(onboarding_complete, 1) FROM users WHERE id = ?",
            params![user_id],
            |r| r.get::<_, i64>(0),
        ).unwrap_or(1) != 0
    }

    pub async fn set_onboarding_complete(&self, user_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("UPDATE users SET onboarding_complete = 1 WHERE id = ?", params![user_id])
            .map_err(|e| format!("set_onboarding: {e}"))?;
        Ok(())
    }

    // ── sharing config ───────────────────────────────────────────────────

    pub async fn get_sharing_mode(&self) -> Result<String, String> {
        let db = self.db.lock().await;
        db.query_row(
            "SELECT mode FROM sharing_config ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("get_sharing_mode: {}", e))
        .map(|o| o.unwrap_or_else(|| "shared".to_string()))
    }

    pub async fn set_sharing_mode(&self, mode: &str, by_user: i64) -> Result<(), String> {
        let valid = ["shared", "isolated", "selective"];
        if !valid.contains(&mode) {
            return Err(format!("invalid sharing mode '{}', expected one of: {}", mode, valid.join(", ")));
        }
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO sharing_config (mode, updated_at, updated_by) VALUES (?, ?, ?)",
            params![mode, now, by_user],
        )
        .map_err(|e| format!("set_sharing_mode: {}", e))?;
        Ok(())
    }

    // ── Telegram chat links ───────────────────────────────────────────────

    pub async fn link_telegram(
        &self,
        user_id: i64,
        bot_token: &str,
        chat_id: i64,
    ) -> Result<(), String> {
        let db = self.db.lock().await;
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

fn map_user_agent(r: &rusqlite::Row) -> rusqlite::Result<UserAgent> {
    Ok(UserAgent {
        id: r.get(0)?,
        user_id: r.get(1)?,
        agent_id: r.get(2)?,
        display_name: r.get(3)?,
        base_agent: r.get(4)?,
        system_prompt: r.get(5)?,
        workspace: r.get(6)?,
        tool_profile: r.get(7)?,
        enabled: r.get::<_, i64>(8)? != 0,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        // v40 columns — all optional; rows from older schemas will still
        // round-trip through this mapper since the column list below is
        // always in the new order where the query selects all 15 columns.
        is_main_thread: r.get::<_, Option<i64>>(11).unwrap_or(None).unwrap_or(0) != 0,
        description: r.get::<_, Option<String>>(12).unwrap_or(None),
        avatar_color: r.get::<_, Option<String>>(13).unwrap_or(None),
        imported_from: r.get::<_, Option<String>>(14).unwrap_or(None),
    })
}

/// Hash a raw bearer token for storage + lookup.
pub fn hash_token(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, digest)
}

/// Hash a user-chosen password with argon2id.
pub fn hash_password(password: &str) -> Result<String, String> {
    use argon2::{Argon2, PasswordHasher};
    use argon2::password_hash::SaltString;
    use argon2::password_hash::rand_core::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("hash_password: {}", e))
}

/// Verify a password against an argon2id hash.
pub fn verify_password_hash(password: &str, hash: &str) -> bool {
    use argon2::{Argon2, PasswordVerifier};
    use argon2::PasswordHash;

    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
