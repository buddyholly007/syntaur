//! Conversation manager: explicit session create/resume for the main agent loop.
//!
//! Conversations are persisted to the same `~/.syntaur/index.db` (schema v5)
//! so they survive restarts. Each conversation has an id, an agent, a title,
//! a user_id (v5 Item 3), a list of messages, and a timestamp range.
//!
//! ## Scoping model (v5 Item 3)
//!
//! Every row in `conversations_v2` has a `user_id`. Writes stamp the
//! caller's id; reads accept an `Option<i64>` **scope**:
//!
//! * `None` → caller is the legacy/admin principal, no filter applied.
//! * `Some(vec![uid, ...])` → filter `WHERE user_id IN (uid, ...)`.
//!
//! This is the single mechanism that stops user A from reading user B's
//! conversations. The admin path is an escape hatch; normal callers must
//! always pass their visible_user_ids from the SharingResolver.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::info;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub user_id: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvMessage {
    pub id: i64,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

pub struct ConversationManager {
    db: Arc<Mutex<Connection>>,
}

impl ConversationManager {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open conversations store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[conversations] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Start a new conversation with an empty message list. `user_id` is
    /// the principal's user id (0 for legacy admin). Returns the id.
    pub async fn create(
        &self,
        agent: &str,
        title: &str,
        user_id: i64,
    ) -> Result<String, String> {
        let id = format!("conv-{}", Uuid::new_v4().simple());
        let now = Utc::now().timestamp();
        let db = Arc::clone(&self.db);
        let id_clone = id.clone();
        let agent = agent.to_string();
        let title = title.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "INSERT INTO conversations_v2 (id, agent, title, created_at, updated_at, user_id) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![&id_clone, &agent, &title, now, now, user_id],
            )
            .map_err(|e| format!("insert conv: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))??;
        Ok(id)
    }

    /// Append a message to a conversation. Updates the conversation's
    /// updated_at timestamp. Note: this does NOT enforce scope — callers
    /// are expected to check ownership via `get(id, scope)` first. This
    /// keeps the hot path (LLM → tool → append) simple.
    pub async fn append(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, String> {
        let db = Arc::clone(&self.db);
        let cid = conversation_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let now = Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let mut conn = db.blocking_lock();
            let tx = conn.transaction().map_err(|e| format!("begin: {}", e))?;
            tx.execute(
                "INSERT INTO conversation_messages_v2 (conversation_id, role, content, created_at) \
                 VALUES (?, ?, ?, ?)",
                params![&cid, &role, &content, now],
            )
            .map_err(|e| format!("insert msg: {}", e))?;
            let mid = tx.last_insert_rowid();
            tx.execute(
                "UPDATE conversations_v2 SET updated_at = ? WHERE id = ?",
                params![now, &cid],
            )
            .map_err(|e| format!("touch conv: {}", e))?;
            tx.commit().map_err(|e| format!("commit: {}", e))?;
            Ok(mid)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Retrieve a conversation summary by id, scoped to a user.
    /// `scope = None` bypasses the filter (admin).
    pub async fn get(&self, id: &str, scope: Option<Vec<i64>>) -> Option<Conversation> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Option<Conversation> {
            let conn = db.blocking_lock();
            let base = "SELECT c.id, c.agent, c.title, c.user_id, c.created_at, c.updated_at, \
                        (SELECT COUNT(*) FROM conversation_messages_v2 WHERE conversation_id = c.id) \
                        FROM conversations_v2 c WHERE c.id = ?";
            match scope {
                None => conn.query_row(base, params![&id], row_to_conv).optional().ok().flatten(),
                Some(ref uids) => {
                    let placeholders = uids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                    let sql = format!("{} AND c.user_id IN ({placeholders})", base);
                    let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    all_params.push(Box::new(id.clone()));
                    for uid in uids { all_params.push(Box::new(*uid)); }
                    let refs: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|b| b.as_ref()).collect();
                    conn.query_row(&sql, refs.as_slice(), row_to_conv).optional().ok().flatten()
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    /// Get all messages for a conversation in chronological order. Scoped
    /// by joining back to conversations_v2 so Bob can't read Alice's
    /// messages even if he knows her conversation id.
    pub async fn messages(&self, id: &str, scope: Option<Vec<i64>>) -> Vec<ConvMessage> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Vec<ConvMessage> {
            let conn = db.blocking_lock();
            let base = "SELECT m.id, m.conversation_id, m.role, m.content, m.created_at \
                        FROM conversation_messages_v2 m \
                        JOIN conversations_v2 c ON c.id = m.conversation_id \
                        WHERE m.conversation_id = ?";
            match scope {
                None => {
                    let sql = format!("{base} ORDER BY m.id ASC");
                    let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return Vec::new() };
                    stmt.query_map(params![&id], row_to_msg).ok()
                        .map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default()
                }
                Some(ref uids) => {
                    let placeholders = uids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                    let sql = format!("{base} AND c.user_id IN ({placeholders}) ORDER BY m.id ASC");
                    let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    all_params.push(Box::new(id.clone()));
                    for uid in uids { all_params.push(Box::new(*uid)); }
                    let refs: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|b| b.as_ref()).collect();
                    let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return Vec::new() };
                    stmt.query_map(refs.as_slice(), row_to_msg).ok()
                        .map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default()
                }
            }
        })
        .await
        .unwrap_or_default()
    }

    /// List the N most recent conversations for an agent, scoped by user.
    pub async fn list_recent(
        &self,
        agent: &str,
        limit: usize,
        scope: Option<Vec<i64>>,
    ) -> Vec<Conversation> {
        let db = Arc::clone(&self.db);
        let agent = agent.to_string();
        tokio::task::spawn_blocking(move || -> Vec<Conversation> {
            let conn = db.blocking_lock();
            let base = "SELECT c.id, c.agent, c.title, c.user_id, c.created_at, c.updated_at, \
                        (SELECT COUNT(*) FROM conversation_messages_v2 WHERE conversation_id = c.id) \
                        FROM conversations_v2 c WHERE c.agent = ?";
            match scope {
                None => {
                    let sql = format!("{base} ORDER BY c.updated_at DESC LIMIT ?");
                    let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return Vec::new() };
                    stmt.query_map(params![&agent, limit as i64], row_to_conv).ok()
                        .map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default()
                }
                Some(ref uids) => {
                    let placeholders = uids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                    let sql = format!("{base} AND c.user_id IN ({placeholders}) ORDER BY c.updated_at DESC LIMIT ?");
                    let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    all_params.push(Box::new(agent.clone()));
                    for uid in uids { all_params.push(Box::new(*uid)); }
                    all_params.push(Box::new(limit as i64));
                    let refs: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|b| b.as_ref()).collect();
                    let mut stmt = match conn.prepare(&sql) { Ok(s) => s, Err(_) => return Vec::new() };
                    stmt.query_map(refs.as_slice(), row_to_conv).ok()
                        .map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default()
                }
            }
        })
        .await
        .unwrap_or_default()
    }
}

fn row_to_conv(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: r.get(0)?,
        agent: r.get(1)?,
        title: r.get(2)?,
        user_id: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        message_count: r.get(6)?,
    })
}

fn row_to_msg(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConvMessage> {
    Ok(ConvMessage {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        created_at: r.get(4)?,
    })
}
