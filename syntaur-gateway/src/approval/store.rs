//! Persistent store for pending approval actions.
//!
//! Schema lives in the same `~/.syntaur/index.db` as the document index
//! and research sessions, added as schema v3.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::info;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingAction {
    pub id: i64,
    pub agent: String,
    pub tool_name: String,
    pub args_json: String,
    pub status: PendingStatus,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
}

impl PendingStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
        }
    }
}

pub struct PendingActionStore {
    db: Arc<Mutex<Connection>>,
}

impl PendingActionStore {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open pending_actions store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[approval:store] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Insert a new pending action and return its rowid (used as the
    /// approval action_id throughout the system). `user_id` stamps the
    /// owning principal (v5 Item 3) so that a different user cannot
    /// approve this action on the owner's behalf.
    pub async fn create(
        &self,
        agent: &str,
        tool_name: &str,
        args_json: &str,
        user_id: i64,
    ) -> Result<i64, String> {
        let db = Arc::clone(&self.db);
        let agent = agent.to_string();
        let tool_name = tool_name.to_string();
        let args_json = args_json.to_string();
        let now = Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = db.blocking_lock();
            conn.execute(
                "INSERT INTO pending_actions (agent, tool_name, args_json, status, created_at, user_id) \
                 VALUES (?, ?, ?, 'pending', ?, ?)",
                params![&agent, &tool_name, &args_json, now, user_id],
            )
            .map_err(|e| format!("insert pending: {}", e))?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Mark a pending action as approved/denied/timed_out.
    ///
    /// `scope` enforces per-user ownership (v5 Item 3): when `Some(uid)`,
    /// the UPDATE only touches rows where `user_id = uid`, so a real
    /// user can't approve another user's action. `None` is the legacy
    /// admin path — affects any row. Returns the number of rows
    /// actually updated so callers can tell an unowned approve apart
    /// from a real resolve.
    pub async fn resolve(
        &self,
        id: i64,
        status: PendingStatus,
        resolved_by: Option<String>,
        scope: Option<i64>,
    ) -> Result<usize, String> {
        let db = Arc::clone(&self.db);
        let now = Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let conn = db.blocking_lock();
            let n = match scope {
                None => conn.execute(
                    "UPDATE pending_actions SET status = ?, resolved_at = ?, resolved_by = ? WHERE id = ?",
                    params![status.as_str(), now, &resolved_by, id],
                ),
                Some(uid) => conn.execute(
                    "UPDATE pending_actions SET status = ?, resolved_at = ?, resolved_by = ? WHERE id = ? AND user_id = ?",
                    params![status.as_str(), now, &resolved_by, id, uid],
                ),
            }
            .map_err(|e| format!("resolve pending: {}", e))?;
            Ok(n)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Look up an action by id.
    #[allow(dead_code)]
    pub async fn get(&self, id: i64) -> Option<PendingAction> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> Option<PendingAction> {
            let conn = db.blocking_lock();
            conn.query_row(
                "SELECT id, agent, tool_name, args_json, status, created_at, resolved_at, resolved_by \
                 FROM pending_actions WHERE id = ?",
                params![id],
                |r| {
                    let status_str: String = r.get(4)?;
                    let status = match status_str.as_str() {
                        "approved" => PendingStatus::Approved,
                        "denied" => PendingStatus::Denied,
                        "timed_out" => PendingStatus::TimedOut,
                        _ => PendingStatus::Pending,
                    };
                    Ok(PendingAction {
                        id: r.get(0)?,
                        agent: r.get(1)?,
                        tool_name: r.get(2)?,
                        args_json: r.get(3)?,
                        status,
                        created_at: r.get(5)?,
                        resolved_at: r.get(6)?,
                        resolved_by: r.get(7)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
        })
        .await
        .ok()
        .flatten()
    }

    /// Recently resolved actions for an agent (used by future audit UI).
    #[allow(dead_code)]
    pub async fn list_recent(&self, agent: &str, limit: usize) -> Vec<PendingAction> {
        let db = Arc::clone(&self.db);
        let agent = agent.to_string();
        tokio::task::spawn_blocking(move || -> Vec<PendingAction> {
            let conn = db.blocking_lock();
            let mut stmt = match conn.prepare(
                "SELECT id, agent, tool_name, args_json, status, created_at, resolved_at, resolved_by \
                 FROM pending_actions WHERE agent = ? ORDER BY created_at DESC LIMIT ?",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt
                .query_map(params![&agent, limit as i64], |r| {
                    let status_str: String = r.get(4)?;
                    let status = match status_str.as_str() {
                        "approved" => PendingStatus::Approved,
                        "denied" => PendingStatus::Denied,
                        "timed_out" => PendingStatus::TimedOut,
                        _ => PendingStatus::Pending,
                    };
                    Ok(PendingAction {
                        id: r.get(0)?,
                        agent: r.get(1)?,
                        tool_name: r.get(2)?,
                        args_json: r.get(3)?,
                        status,
                        created_at: r.get(5)?,
                        resolved_at: r.get(6)?,
                        resolved_by: r.get(7)?,
                    })
                })
                .ok();
            match rows {
                Some(iter) => iter.filter_map(Result::ok).collect(),
                None => Vec::new(),
            }
        })
        .await
        .unwrap_or_default()
    }
}
