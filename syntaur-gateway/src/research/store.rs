//! Persistent research session store.
//!
//! Lives in the same SQLite file as the document index (`~/.syntaur/index.db`)
//! but uses a separate `Connection` so reads/writes don't fight the document
//! index path. WAL mode (set by `Indexer::open`) makes concurrent connections
//! safe for this workload.
//!
//! Sessions are written through 4 lifecycle methods:
//!   `create()` → `update_status()` → `store_plan()` → `store_evidence()` →
//!   `store_report()` → `mark_complete()`
//!
//! `find_cached_recent()` looks up a previous successful session for the same
//! `(agent, query)` hash within a max-age window — used for the result cache.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use log::{debug, info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::evidence::{EvidenceItem, Plan, PlanStep, ResearchReport};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRow {
    pub id: String,
    pub agent: String,
    pub query: String,
    pub status: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

pub struct SessionStore {
    db: Arc<Mutex<Connection>>,
}

impl SessionStore {
    /// Open a second connection to the index database. The schema must
    /// already be migrated by `Indexer::open` before this is called.
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open session store {}: {}", db_path.display(), e))?;
        // Same WAL settings as the indexer
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[research:store] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Create a new session row in `pending` state. Returns the generated id.
    /// `user_id` stamps the owning caller for v5 Item 3 per-user scoping
    /// (0 = legacy admin, positive = real user id).
    pub async fn create(
        &self,
        agent: &str,
        query: &str,
        user_id: i64,
    ) -> Result<String, String> {
        let id = format!("res-{}", Uuid::new_v4().simple());
        let hash = hash_query(agent, query);
        let now = Utc::now().timestamp();
        let db = Arc::clone(&self.db);
        let id_clone = id.clone();
        let agent = agent.to_string();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "INSERT INTO research_sessions (id, agent, query, query_hash, status, created_at, user_id) \
                 VALUES (?, ?, ?, ?, 'pending', ?, ?)",
                params![&id_clone, &agent, &query, &hash, now, user_id],
            )
            .map_err(|e| format!("insert session: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))??;
        Ok(id)
    }

    /// Update the status column. Use the `phase` enum-like values: 'planning',
    /// 'orchestrating', 'reporting', 'complete', 'failed'.
    pub async fn update_status(&self, id: &str, status: &str) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        let status = status.to_string();
        let now = Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            // First-status-change marks started_at
            conn.execute(
                "UPDATE research_sessions SET status = ?, started_at = COALESCE(started_at, ?) WHERE id = ?",
                params![&status, now, &id],
            )
            .map_err(|e| format!("update status: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    pub async fn store_plan(&self, id: &str, plan: &Plan) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        let plan_json = serde_json::to_string(&plan.plan)
            .map_err(|e| format!("serialize plan: {}", e))?;
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "UPDATE research_sessions SET plan_json = ? WHERE id = ?",
                params![&plan_json, &id],
            )
            .map_err(|e| format!("store plan: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    pub async fn store_evidence(&self, id: &str, evidence: &[EvidenceItem]) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        let evidence_json = serde_json::to_string(evidence)
            .map_err(|e| format!("serialize evidence: {}", e))?;
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "UPDATE research_sessions SET evidence_json = ? WHERE id = ?",
                params![&evidence_json, &id],
            )
            .map_err(|e| format!("store evidence: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    pub async fn store_report(&self, id: &str, report_text: &str) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        let report_text = report_text.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "UPDATE research_sessions SET report_text = ? WHERE id = ?",
                params![&report_text, &id],
            )
            .map_err(|e| format!("store report: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    pub async fn mark_complete(
        &self,
        id: &str,
        duration_ms: u64,
        error: Option<String>,
    ) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        let now = Utc::now().timestamp();
        let status = if error.is_some() { "failed" } else { "complete" };
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "UPDATE research_sessions SET status = ?, completed_at = ?, duration_ms = ?, error = ? WHERE id = ?",
                params![status, now, duration_ms as i64, &error, &id],
            )
            .map_err(|e| format!("mark complete: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Find a recent successful session for the same (agent, query). Returns
    /// the most recent if its age <= `max_age_secs`.
    pub async fn find_cached_recent(
        &self,
        agent: &str,
        query: &str,
        max_age_secs: i64,
        scope: Option<i64>,
    ) -> Option<ResearchReport> {
        let hash = hash_query(agent, query);
        let now = Utc::now().timestamp();
        let cutoff = now - max_age_secs;
        let db = Arc::clone(&self.db);
        let agent = agent.to_string();
        tokio::task::spawn_blocking(move || -> Option<ResearchReport> {
            let conn = db.blocking_lock();
            // Cache lookups are scoped per-user so Alice's cached result
            // never gets served to Bob (and vice versa). Admin (scope=None)
            // sees everything.
            let row: Option<(String, String, String, String, String, i64, Option<i64>)> = match scope {
                None => conn
                    .query_row(
                        "SELECT id, query, plan_json, evidence_json, report_text, created_at, duration_ms \
                         FROM research_sessions \
                         WHERE query_hash = ? AND agent = ? AND status = 'complete' AND created_at >= ? \
                         ORDER BY created_at DESC LIMIT 1",
                        params![&hash, &agent, cutoff],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
                    )
                    .optional()
                    .ok()
                    .flatten(),
                Some(uid) => conn
                    .query_row(
                        "SELECT id, query, plan_json, evidence_json, report_text, created_at, duration_ms \
                         FROM research_sessions \
                         WHERE query_hash = ? AND agent = ? AND status = 'complete' AND created_at >= ? AND user_id = ? \
                         ORDER BY created_at DESC LIMIT 1",
                        params![&hash, &agent, cutoff, uid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
                    )
                    .optional()
                    .ok()
                    .flatten(),
            };
            let (id, query, plan_json, evidence_json, report_text, _created, duration_ms) = row?;
            let plan: Vec<PlanStep> = serde_json::from_str(&plan_json).unwrap_or_default();
            let evidence: Vec<EvidenceItem> = serde_json::from_str(&evidence_json).unwrap_or_default();
            debug!("[research:store] cache HIT {} (age cutoff {}s)", id, max_age_secs);
            Some(ResearchReport {
                query,
                plan,
                evidence,
                report: report_text,
                total_duration_ms: duration_ms.unwrap_or(0) as u64,
                error: None,
            })
        })
        .await
        .ok()
        .flatten()
    }

    /// Get a session by id (any status). Used by /api/research/{id} GET.
    /// `scope` = user filter (None for admin).
    pub async fn get(&self, id: &str, scope: Option<Vec<i64>>) -> Option<ResearchReport> {
        let db = Arc::clone(&self.db);
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Option<ResearchReport> {
            let conn = db.blocking_lock();
            let base = "SELECT query, plan_json, evidence_json, report_text, duration_ms, error, status \
                        FROM research_sessions WHERE id = ?";
            let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>, String)> = match scope {
                None => conn
                    .query_row(base, params![&id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
                    .optional().ok().flatten(),
                Some(ref uids) => {
                    let placeholders = uids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                    let sql = format!("{base} AND user_id IN ({placeholders})");
                    let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    all_params.push(Box::new(id.clone()));
                    for uid in uids { all_params.push(Box::new(*uid)); }
                    let refs: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|b| b.as_ref()).collect();
                    conn.query_row(&sql, refs.as_slice(),
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
                        .optional().ok().flatten()
                }
            };
            let (query, plan_json, evidence_json, report_text, duration_ms, error, _status) = row?;
            let plan: Vec<PlanStep> = plan_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let evidence: Vec<EvidenceItem> = evidence_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            Some(ResearchReport {
                query,
                plan,
                evidence,
                report: report_text.unwrap_or_default(),
                total_duration_ms: duration_ms.unwrap_or(0) as u64,
                error,
            })
        })
        .await
        .ok()
        .flatten()
    }

    /// List the N most recent sessions across every agent (any status).
    /// Used by the /research page's "recent research" card.
    pub async fn list_recent_all(&self, limit: usize) -> Vec<SessionRow> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> Vec<SessionRow> {
            let conn = db.blocking_lock();
            let mut stmt = match conn.prepare(
                "SELECT id, agent, query, status, created_at, completed_at, duration_ms, error \
                 FROM research_sessions ORDER BY created_at DESC LIMIT ?",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt
                .query_map(params![limit as i64], |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        agent: r.get(1)?,
                        query: r.get(2)?,
                        status: r.get(3)?,
                        created_at: r.get(4)?,
                        completed_at: r.get(5)?,
                        duration_ms: r.get(6)?,
                        error: r.get(7)?,
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

    /// List the N most recent sessions for an agent (any status).
    /// Used for /api/research/recent and future debugging UI.
    #[allow(dead_code)]
    pub async fn list_recent(&self, agent: &str, limit: usize) -> Vec<SessionRow> {
        let db = Arc::clone(&self.db);
        let agent = agent.to_string();
        tokio::task::spawn_blocking(move || -> Vec<SessionRow> {
            let conn = db.blocking_lock();
            let mut stmt = match conn.prepare(
                "SELECT id, agent, query, status, created_at, completed_at, duration_ms, error \
                 FROM research_sessions WHERE agent = ? ORDER BY created_at DESC LIMIT ?",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt
                .query_map(params![&agent, limit as i64], |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        agent: r.get(1)?,
                        query: r.get(2)?,
                        status: r.get(3)?,
                        created_at: r.get(4)?,
                        completed_at: r.get(5)?,
                        duration_ms: r.get(6)?,
                        error: r.get(7)?,
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

/// Stable hash for cache lookup. Normalizes whitespace + lowercases.
pub fn hash_query(agent: &str, query: &str) -> String {
    let normalized = query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let mut hasher = Sha256::new();
    hasher.update(agent.as_bytes());
    hasher.update(b"\0");
    hasher.update(normalized.as_bytes());
    let bytes = hasher.finalize();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Helper to convert `SystemTime::now()` to a unix epoch second i64.
#[allow(dead_code)]
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Used to suppress unused-import warnings if `warn` becomes unused later.
#[allow(dead_code)]
fn _ensure_log_used() {
    warn!("noop");
}


impl SessionStore {
    /// Persist the unique (source, external_id) pairs that a research session
    /// cited. Used by the cache invalidation pass: when an indexed document
    /// changes, we look up which sessions referenced it and mark them stale.
    pub async fn store_doc_refs(
        &self,
        session_id: &str,
        refs: Vec<(String, String)>,
    ) -> Result<(), String> {
        if refs.is_empty() {
            return Ok(());
        }
        let db = Arc::clone(&self.db);
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut conn = db.blocking_lock();
            let tx = conn.transaction().map_err(|e| format!("begin: {}", e))?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT OR IGNORE INTO research_session_doc_refs (session_id, source, external_id) VALUES (?, ?, ?)",
                    )
                    .map_err(|e| format!("prepare doc_refs: {}", e))?;
                for (source, external_id) in refs {
                    let _ = stmt.execute(params![&session_id, &source, &external_id]);
                }
            }
            tx.commit().map_err(|e| format!("commit: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Internal: mark all 'complete' research sessions that cited the given
    /// document as 'stale' so they won't be served from cache anymore.
    /// Returns the number of sessions invalidated.
    pub async fn mark_stale_for_doc_impl(
        &self,
        source: &str,
        external_id: &str,
    ) -> Result<usize, String> {
        let db = Arc::clone(&self.db);
        let source = source.to_string();
        let external_id = external_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let conn = db.blocking_lock();
            let n = conn.execute(
                "UPDATE research_sessions SET status = 'stale'                  WHERE status = 'complete' AND id IN (                      SELECT session_id FROM research_session_doc_refs                      WHERE source = ? AND external_id = ?                  )",
                params![&source, &external_id],
            ).map_err(|e| format!("mark stale: {}", e))?;
            Ok(n)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }
}

#[async_trait::async_trait]
impl crate::index::StaleNotifier for SessionStore {
    async fn mark_stale_for_doc(&self, source: &str, external_id: &str) {
        match self.mark_stale_for_doc_impl(source, external_id).await {
            Ok(n) if n > 0 => {
                log::info!(
                    "[research:store] invalidated {} cached session(s) for {}/{}",
                    n, source, external_id
                );
            }
            Ok(_) => {}
            Err(e) => log::warn!("[research:store] mark_stale failed: {}", e),
        }
    }
}
