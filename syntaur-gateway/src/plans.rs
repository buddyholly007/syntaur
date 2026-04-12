//! Plan mode — multi-step approval-gated workflows.
//!
//! A **plan** is a titled list of ordered steps that the user (or Felix)
//! can propose, send for human approval via Telegram inline keyboard, and
//! then have executed sequentially by the gateway.
//!
//! ## Step kinds
//!   - `tool` — call a registered tool by name with the given args, via
//!     the existing tool dispatch funnel
//!   - `skill` — invoke a registered skill by name with the given args
//!   - `note` — informational only, never executed (used for human
//!     reminders interleaved with executable steps)
//!
//! ## Lifecycle
//!   1. `propose_plan` HTTP / tool creates the plan + steps in SQLite,
//!      sets status='pending', sends Telegram approval keyboard
//!   2. The user clicks Approve → telegram callback resolves the
//!      `PlanRegistry` oneshot → handler marks plan='approved' and spawns
//!      the executor task
//!   3. The executor walks `plan_steps` ord-by-ord, recording per-step
//!      status + result_text, and finalizes plan status='complete'/'failed'
//!   4. Denial → status='denied', no execution
//!
//! ## Schema
//! `plans` + `plan_steps` tables in `src/index/schema.rs` v9.
//!
//! ## Why a separate module from approval/
//! The existing `approval` module gates a *single* tool call. Plans gate
//! a *whole sequence* and need to persist intermediate state (step results,
//! step ordering). Sharing code would have meant gutting one of them; cheap
//! duplication is the right call here.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::{info, warn};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Tool,
    Skill,
    Note,
}

impl StepKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Skill => "skill",
            Self::Note => "note",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "tool" => Some(Self::Tool),
            "skill" => Some(Self::Skill),
            "note" => Some(Self::Note),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanRow {
    pub id: i64,
    pub user_id: i64,
    pub agent_id: String,
    pub title: String,
    pub rationale: String,
    pub status: String,
    pub created_at: i64,
    pub approved_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStepRow {
    pub id: i64,
    pub plan_id: i64,
    pub ord: i64,
    pub step_kind: StepKind,
    pub step_target: String,
    pub args: Value,
    pub status: String,
    pub result_text: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// Step input for `propose_plan`. Plain JSON-friendly shape so the LLM and
/// HTTP callers can construct it identically.
#[derive(Debug, Clone, Deserialize)]
pub struct ProposeStep {
    pub kind: String,           // 'tool' | 'skill' | 'note'
    pub target: String,         // tool name OR skill name OR note text
    #[serde(default)]
    pub args: Value,
}

pub struct PlanStore {
    db: Arc<Mutex<Connection>>,
}

impl PlanStore {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open plan store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[plans] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Create a plan + its steps in a single transaction. Returns the new
    /// plan id. Status starts as 'pending' until approval.
    pub async fn create(
        &self,
        user_id: i64,
        agent_id: &str,
        title: &str,
        rationale: &str,
        steps: &[ProposeStep],
    ) -> Result<i64, String> {
        let now = Utc::now().timestamp();
        let mut db = self.db.lock().await;
        let tx = db
            .transaction()
            .map_err(|e| format!("tx: {}", e))?;
        tx.execute(
            "INSERT INTO plans (user_id, agent_id, title, rationale, status, created_at) \
             VALUES (?, ?, ?, ?, 'pending', ?)",
            params![user_id, agent_id, title, rationale, now],
        )
        .map_err(|e| format!("insert plan: {}", e))?;
        let plan_id = tx.last_insert_rowid();
        for (i, step) in steps.iter().enumerate() {
            // Validate kind early so we don't persist garbage
            if StepKind::parse(&step.kind).is_none() {
                return Err(format!(
                    "step {}: kind '{}' must be tool|skill|note",
                    i, step.kind
                ));
            }
            tx.execute(
                "INSERT INTO plan_steps (plan_id, ord, step_kind, step_target, args_json, status) \
                 VALUES (?, ?, ?, ?, ?, 'pending')",
                params![
                    plan_id,
                    i as i64,
                    step.kind,
                    step.target,
                    step.args.to_string()
                ],
            )
            .map_err(|e| format!("insert step {}: {}", i, e))?;
        }
        tx.commit().map_err(|e| format!("commit: {}", e))?;
        Ok(plan_id)
    }

    pub async fn get(&self, plan_id: i64) -> Result<Option<(PlanRow, Vec<PlanStepRow>)>, String> {
        let db = self.db.lock().await;
        let plan = db
            .query_row(
                "SELECT id, user_id, agent_id, title, rationale, status, created_at, \
                 approved_at, completed_at, error FROM plans WHERE id = ?",
                params![plan_id],
                |r| {
                    Ok(PlanRow {
                        id: r.get(0)?,
                        user_id: r.get(1)?,
                        agent_id: r.get(2)?,
                        title: r.get(3)?,
                        rationale: r.get(4)?,
                        status: r.get(5)?,
                        created_at: r.get(6)?,
                        approved_at: r.get(7)?,
                        completed_at: r.get(8)?,
                        error: r.get(9)?,
                    })
                },
            )
            .ok();
        let Some(plan) = plan else {
            return Ok(None);
        };
        let mut stmt = db
            .prepare(
                "SELECT id, plan_id, ord, step_kind, step_target, args_json, status, \
                 result_text, started_at, completed_at FROM plan_steps WHERE plan_id = ? ORDER BY ord",
            )
            .map_err(|e| format!("prep steps: {}", e))?;
        let steps = stmt
            .query_map([plan_id], |r| {
                let kind_s: String = r.get(3)?;
                let args_s: String = r.get(5)?;
                Ok(PlanStepRow {
                    id: r.get(0)?,
                    plan_id: r.get(1)?,
                    ord: r.get(2)?,
                    step_kind: StepKind::parse(&kind_s).unwrap_or(StepKind::Note),
                    step_target: r.get(4)?,
                    args: serde_json::from_str(&args_s).unwrap_or(json!({})),
                    status: r.get(6)?,
                    result_text: r.get(7)?,
                    started_at: r.get(8)?,
                    completed_at: r.get(9)?,
                })
            })
            .map_err(|e| format!("query steps: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("row: {}", e))?;
        Ok(Some((plan, steps)))
    }

    pub async fn list(&self, user_id_filter: Option<i64>) -> Result<Vec<PlanRow>, String> {
        let db = self.db.lock().await;
        let (sql, has_filter) = match user_id_filter {
            Some(_) => (
                "SELECT id, user_id, agent_id, title, rationale, status, created_at, \
                 approved_at, completed_at, error FROM plans WHERE user_id = ? \
                 ORDER BY created_at DESC LIMIT 100",
                true,
            ),
            None => (
                "SELECT id, user_id, agent_id, title, rationale, status, created_at, \
                 approved_at, completed_at, error FROM plans ORDER BY created_at DESC LIMIT 100",
                false,
            ),
        };
        let mut stmt = db.prepare(sql).map_err(|e| format!("prep: {}", e))?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<PlanRow> {
            Ok(PlanRow {
                id: r.get(0)?,
                user_id: r.get(1)?,
                agent_id: r.get(2)?,
                title: r.get(3)?,
                rationale: r.get(4)?,
                status: r.get(5)?,
                created_at: r.get(6)?,
                approved_at: r.get(7)?,
                completed_at: r.get(8)?,
                error: r.get(9)?,
            })
        };
        let rows = if has_filter {
            stmt.query_map([user_id_filter.unwrap()], map_row)
                .map_err(|e| format!("query: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("row: {}", e))?
        } else {
            stmt.query_map([], map_row)
                .map_err(|e| format!("query: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("row: {}", e))?
        };
        Ok(rows)
    }

    pub async fn mark_approved(&self, plan_id: i64) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plans SET status='approved', approved_at=? WHERE id=? AND status='pending'",
            params![now, plan_id],
        )
        .map_err(|e| format!("approve: {}", e))?;
        Ok(())
    }

    pub async fn mark_denied(&self, plan_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plans SET status='denied' WHERE id=? AND status='pending'",
            params![plan_id],
        )
        .map_err(|e| format!("deny: {}", e))?;
        Ok(())
    }

    pub async fn mark_executing(&self, plan_id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plans SET status='executing' WHERE id=?",
            params![plan_id],
        )
        .map_err(|e| format!("executing: {}", e))?;
        Ok(())
    }

    pub async fn mark_complete(&self, plan_id: i64) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plans SET status='complete', completed_at=? WHERE id=?",
            params![now, plan_id],
        )
        .map_err(|e| format!("complete: {}", e))?;
        Ok(())
    }

    pub async fn mark_failed(&self, plan_id: i64, err: &str) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plans SET status='failed', error=?, completed_at=? WHERE id=?",
            params![err, now, plan_id],
        )
        .map_err(|e| format!("fail: {}", e))?;
        Ok(())
    }

    pub async fn step_start(&self, step_id: i64) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plan_steps SET status='running', started_at=? WHERE id=?",
            params![now, step_id],
        )
        .map_err(|e| format!("step_start: {}", e))?;
        Ok(())
    }

    pub async fn step_complete(
        &self,
        step_id: i64,
        ok: bool,
        result_text: &str,
    ) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let status = if ok { "complete" } else { "failed" };
        let db = self.db.lock().await;
        db.execute(
            "UPDATE plan_steps SET status=?, result_text=?, completed_at=? WHERE id=?",
            params![status, result_text, now, step_id],
        )
        .map_err(|e| format!("step_complete: {}", e))?;
        Ok(())
    }
}

/// In-process registry of pending plan approvals waiting on a oneshot
/// channel. Mirrors `approval::ApprovalRegistry` but namespaced to plans
/// so callbacks don't collide. Used by both the propose_plan handler and
/// the telegram callback router.
pub struct PlanRegistry {
    pending: Mutex<std::collections::HashMap<i64, oneshot::Sender<bool>>>,
}

impl PlanRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(std::collections::HashMap::new()),
        })
    }

    pub async fn register(&self, plan_id: i64) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        let mut map = self.pending.lock().await;
        map.insert(plan_id, tx);
        rx
    }

    pub async fn resolve(&self, plan_id: i64, approved: bool) -> bool {
        let mut map = self.pending.lock().await;
        if let Some(tx) = map.remove(&plan_id) {
            let _ = tx.send(approved);
            true
        } else {
            false
        }
    }
}

impl Default for PlanRegistry {
    fn default() -> Self {
        Self {
            pending: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

/// Send a Telegram approval keyboard for a plan. Inline keyboard buttons
/// use callback_data prefixes `plan_approve:{id}` / `plan_deny:{id}` so the
/// telegram poller can route them to `PlanRegistry::resolve` instead of
/// the existing `ApprovalRegistry::resolve`.
pub async fn send_approval(
    http: &reqwest::Client,
    bot_token: &str,
    chat_id: i64,
    plan_id: i64,
    plan: &PlanRow,
    steps: &[PlanStepRow],
) -> Result<(), String> {
    let mut summary = format!("📋 *Plan #{}*\n*{}*\n", plan_id, plan.title);
    if !plan.rationale.is_empty() {
        summary.push_str(&format!("\n_{}_\n", plan.rationale));
    }
    summary.push_str("\n*Steps:*\n");
    for s in steps {
        let label = match s.step_kind {
            StepKind::Tool => "🔧",
            StepKind::Skill => "⚙",
            StepKind::Note => "📝",
        };
        summary.push_str(&format!("{} {}. {}\n", label, s.ord + 1, s.step_target));
    }
    summary.push_str("\nReply with the buttons below.");

    let keyboard = json!({
        "inline_keyboard": [[
            {"text": "✅ Approve", "callback_data": format!("plan_approve:{}", plan_id)},
            {"text": "❌ Deny", "callback_data": format!("plan_deny:{}", plan_id)}
        ]]
    });

    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let payload = json!({
        "chat_id": chat_id,
        "text": summary,
        "parse_mode": "Markdown",
        "reply_markup": keyboard,
    });
    let resp = http
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("send plan approval: {}", e))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("telegram error: {}", body));
    }
    info!("[plans] sent approval prompt for plan {}", plan_id);
    Ok(())
}

/// Execute the approved plan, walking each step in order. The dispatcher
/// closure is provided so that the executor doesn't need to know about
/// the ToolRegistry/SkillStore types directly — the caller wires it up
/// once and passes it in.
///
/// Failure of any step marks the whole plan failed and stops execution.
/// Note steps are recorded as 'complete' immediately without dispatching.
pub async fn execute_plan<F, Fut>(
    store: Arc<PlanStore>,
    plan_id: i64,
    dispatcher: F,
) -> Result<(), String>
where
    F: Fn(StepKind, String, Value) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<String, String>> + Send,
{
    store.mark_executing(plan_id).await?;
    let Some((_plan, steps)) = store.get(plan_id).await? else {
        return Err(format!("plan {} disappeared mid-execution", plan_id));
    };
    for step in steps {
        if step.step_kind == StepKind::Note {
            let _ = store
                .step_complete(step.id, true, &step.step_target)
                .await;
            continue;
        }
        store.step_start(step.id).await?;
        let result = dispatcher(step.step_kind, step.step_target.clone(), step.args.clone()).await;
        match result {
            Ok(text) => {
                let _ = store.step_complete(step.id, true, &text).await;
            }
            Err(e) => {
                let _ = store.step_complete(step.id, false, &e).await;
                let msg = format!("step {} ({}) failed: {}", step.ord + 1, step.step_target, e);
                let _ = store.mark_failed(plan_id, &msg).await;
                warn!("[plans:{}] {}", plan_id, msg);
                return Err(msg);
            }
        }
    }
    store.mark_complete(plan_id).await?;
    info!("[plans:{}] complete", plan_id);
    Ok(())
}
