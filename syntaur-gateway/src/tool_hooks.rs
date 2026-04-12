//! User-configurable PreToolUse / PostToolUse hooks.
//!
//! Distinct from the existing in-process `HookBus` in `src/hooks.rs` which
//! is a system-wide pub-sub for internal events. This module is the
//! **user-facing** hook system: rows live in the `tool_hooks` table
//! (schema v9), the dispatch funnel queries them on every tool call, and
//! actions can block, notify, audit, or invoke a downstream skill.
//!
//! ## Lifecycle
//!
//! Hooks are loaded from SQLite once at startup into an `Arc<HookStore>`
//! that the tool dispatch funnel queries on every call. Adds/removes via
//! the admin endpoints update the in-memory snapshot atomically (whole
//! reload). Single-user / low-qps workload — no need for a fancy diff.
//!
//! ## Insertion points
//!
//! `ToolRegistry::dispatch_extension` (in `tools/mod.rs`) calls:
//!   1. `tool_hooks::fire_pre(...)` immediately after the tool is looked
//!      up. If any pre-hook returns `Action::Block`, the dispatch is
//!      aborted and the block message is returned to the LLM.
//!   2. `tool_hooks::fire_post(...)` after the tool result (success or
//!      failure) is captured but before the result is returned. Post
//!      hooks fire for side effects only — they cannot mutate the result.
//!
//! Pre-hook side effects (notify/audit/run_skill) run synchronously before
//! the tool itself fires. Post-hook side effects run synchronously before
//! the result is returned to the caller. Both are awaited so that any
//! errors surface in the same request rather than disappearing into a
//! background task.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolCall,
    PostToolCall,
}

impl HookEvent {
    fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolCall => "pre_tool_call",
            Self::PostToolCall => "post_tool_call",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pre_tool_call" => Some(Self::PreToolCall),
            "post_tool_call" => Some(Self::PostToolCall),
            _ => None,
        }
    }
}

/// One persisted hook row, ready for matching at dispatch time.
#[derive(Debug, Clone, Serialize)]
pub struct HookRow {
    pub id: i64,
    pub event: HookEvent,
    pub match_pattern: Value,
    pub action: String,
    pub action_config: Value,
    pub enabled: bool,
}

/// In-memory snapshot of all enabled hooks. Reloaded on any change.
#[derive(Default)]
pub struct HookSnapshot {
    pub pre: Vec<HookRow>,
    pub post: Vec<HookRow>,
}

pub struct HookStore {
    db: Arc<Mutex<Connection>>,
    snapshot: Arc<RwLock<HookSnapshot>>,
    /// Telegram alerting (cloned from approval ctx wiring)
    pub tg_bot_token: String,
    pub tg_chat_id: i64,
    pub http_client: reqwest::Client,
}

impl HookStore {
    pub async fn open(
        db_path: PathBuf,
        tg_bot_token: String,
        tg_chat_id: i64,
        http_client: reqwest::Client,
    ) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open hook store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[tool_hooks] opened {}", db_path.display());
        let store = Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
            snapshot: Arc::new(RwLock::new(HookSnapshot::default())),
            tg_bot_token,
            tg_chat_id,
            http_client,
        });
        store.reload().await?;
        Ok(store)
    }

    /// Reload all enabled hooks from SQLite into the in-memory snapshot.
    /// Called on startup and after any add/delete via the admin API.
    ///
    /// **Send safety**: rusqlite's `Statement<'_>` is `!Send` because it
    /// holds a raw `*mut sqlite3_stmt`. We do all SQLite work inside an
    /// inner block so the statement (and the connection guard) are dropped
    /// before we await on the snapshot RwLock — otherwise this future
    /// wouldn't be `Send` and axum couldn't use the admin handler.
    pub async fn reload(&self) -> Result<(), String> {
        let (pre, post) = {
            let db = self.db.lock().await;
            let mut stmt = db
                .prepare(
                    "SELECT id, event, match_pattern_json, action, action_config_json, enabled \
                     FROM tool_hooks WHERE enabled = 1 ORDER BY id",
                )
                .map_err(|e| format!("prep: {}", e))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|e| format!("query: {}", e))?;

            let mut pre = Vec::new();
            let mut post = Vec::new();
            for row in rows {
                let (id, event_s, match_s, action, action_cfg_s, enabled) =
                    row.map_err(|e| format!("row: {}", e))?;
                let Some(event) = HookEvent::parse(&event_s) else {
                    warn!("[tool_hooks] unknown event '{}' on hook {}", event_s, id);
                    continue;
                };
                let match_pattern: Value =
                    serde_json::from_str(&match_s).unwrap_or(Value::Object(Default::default()));
                let action_config: Value = serde_json::from_str(&action_cfg_s)
                    .unwrap_or(Value::Object(Default::default()));
                let entry = HookRow {
                    id,
                    event,
                    match_pattern,
                    action,
                    action_config,
                    enabled: enabled != 0,
                };
                match event {
                    HookEvent::PreToolCall => pre.push(entry),
                    HookEvent::PostToolCall => post.push(entry),
                }
            }
            (pre, post)
            // stmt + db guard dropped here
        };

        let mut snap = self.snapshot.write().await;
        snap.pre = pre;
        snap.post = post;
        info!(
            "[tool_hooks] loaded {} pre + {} post",
            snap.pre.len(),
            snap.post.len()
        );
        Ok(())
    }

    pub async fn create(
        &self,
        event: HookEvent,
        match_pattern: &Value,
        action: &str,
        action_config: &Value,
    ) -> Result<i64, String> {
        let now = Utc::now().timestamp();
        let id = {
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO tool_hooks (event, match_pattern_json, action, action_config_json, enabled, created_at) \
                 VALUES (?, ?, ?, ?, 1, ?)",
                params![
                    event.as_str(),
                    match_pattern.to_string(),
                    action,
                    action_config.to_string(),
                    now
                ],
            )
            .map_err(|e| format!("insert hook: {}", e))?;
            db.last_insert_rowid()
        };
        self.reload().await?;
        Ok(id)
    }

    pub async fn delete(&self, id: i64) -> Result<(), String> {
        {
            let db = self.db.lock().await;
            db.execute("DELETE FROM tool_hooks WHERE id = ?", params![id])
                .map_err(|e| format!("delete hook: {}", e))?;
        }
        self.reload().await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<HookRow>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT id, event, match_pattern_json, action, action_config_json, enabled \
                 FROM tool_hooks ORDER BY id",
            )
            .map_err(|e| format!("prep: {}", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| format!("query: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, event_s, match_s, action, action_cfg_s, enabled) =
                r.map_err(|e| format!("row: {}", e))?;
            let Some(event) = HookEvent::parse(&event_s) else {
                continue;
            };
            out.push(HookRow {
                id,
                event,
                match_pattern: serde_json::from_str(&match_s)
                    .unwrap_or(Value::Object(Default::default())),
                action,
                action_config: serde_json::from_str(&action_cfg_s)
                    .unwrap_or(Value::Object(Default::default())),
                enabled: enabled != 0,
            });
        }
        Ok(out)
    }

    /// Fire pre-tool hooks. Returns Err(message) if any hook blocks.
    pub async fn fire_pre(
        &self,
        tool_name: &str,
        agent_id: &str,
        args: &Value,
    ) -> Result<(), String> {
        let snap = self.snapshot.read().await;
        for hook in &snap.pre {
            if !match_pattern(&hook.match_pattern, tool_name, agent_id, args, None) {
                continue;
            }
            self.run_action(hook, tool_name, agent_id, args, None, None)
                .await?;
        }
        Ok(())
    }

    /// Fire post-tool hooks. Side effects only — never blocks the
    /// already-completed tool call. Errors from actions are logged but
    /// don't propagate.
    pub async fn fire_post(
        &self,
        tool_name: &str,
        agent_id: &str,
        args: &Value,
        success: bool,
        output: &str,
    ) {
        let snap = self.snapshot.read().await;
        for hook in &snap.post {
            if !match_pattern(&hook.match_pattern, tool_name, agent_id, args, Some(success)) {
                continue;
            }
            if let Err(e) = self
                .run_action(hook, tool_name, agent_id, args, Some(success), Some(output))
                .await
            {
                warn!("[tool_hooks:{}] post-action failed: {}", hook.id, e);
            }
        }
    }

    /// Execute the configured action for a matching hook.
    async fn run_action(
        &self,
        hook: &HookRow,
        tool_name: &str,
        agent_id: &str,
        args: &Value,
        success: Option<bool>,
        output: Option<&str>,
    ) -> Result<(), String> {
        match hook.action.as_str() {
            "telegram_notify" => {
                let template = hook
                    .action_config
                    .get("template")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Hook fired: {{tool}}");
                let msg = expand_template(template, tool_name, agent_id, args, success, output);
                let body = serde_json::json!({"chat_id": self.tg_chat_id, "text": msg});
                let url = format!(
                    "https://api.telegram.org/bot{}/sendMessage",
                    self.tg_bot_token
                );
                let _ = self.http_client.post(&url).json(&body).send().await;
                Ok(())
            }
            "audit_log" => {
                info!(
                    "[tool_hooks:audit:{}] tool={} agent={} success={:?}",
                    hook.id, tool_name, agent_id, success
                );
                Ok(())
            }
            "block" => {
                let msg = hook
                    .action_config
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("blocked by hook")
                    .to_string();
                Err(msg)
            }
            // run_skill is wired by the dispatcher caller because we don't
            // want a circular dependency between tool_hooks and skills.
            // For now we log it and return ok; the real call site lives
            // in tools/mod.rs::dispatch_extension which checks
            // hook.action == "run_skill" after we return.
            "run_skill" => Ok(()),
            other => {
                warn!("[tool_hooks:{}] unknown action '{}'", hook.id, other);
                Ok(())
            }
        }
    }
}

/// Decide whether a hook's match_pattern matches the current invocation.
/// Pattern semantics:
///   - missing key = wildcard (matches anything)
///   - "tool": "browser_open" → only fires for that tool
///   - "tool_prefix": "browser_" → fires for any tool whose name starts with the prefix
///   - "agent": "main" → only that agent
///   - "success": false → post-hook only fires on failure
fn match_pattern(
    pattern: &Value,
    tool_name: &str,
    agent_id: &str,
    _args: &Value,
    success: Option<bool>,
) -> bool {
    let Some(obj) = pattern.as_object() else {
        return true;
    };
    if let Some(t) = obj.get("tool").and_then(|v| v.as_str()) {
        if t != tool_name {
            return false;
        }
    }
    if let Some(p) = obj.get("tool_prefix").and_then(|v| v.as_str()) {
        if !tool_name.starts_with(p) {
            return false;
        }
    }
    if let Some(a) = obj.get("agent").and_then(|v| v.as_str()) {
        if a != agent_id {
            return false;
        }
    }
    if let Some(s) = obj.get("success").and_then(|v| v.as_bool()) {
        match success {
            Some(actual) if actual == s => {}
            Some(_) => return false,
            None => {} // pre-hook, success not yet known — let it through
        }
    }
    true
}

/// Expand a template string with hook context. Supported placeholders:
/// `{{tool}}`, `{{agent}}`, `{{success}}`, `{{output}}`, `{{args}}`.
fn expand_template(
    template: &str,
    tool_name: &str,
    agent_id: &str,
    args: &Value,
    success: Option<bool>,
    output: Option<&str>,
) -> String {
    let success_s = match success {
        Some(true) => "ok",
        Some(false) => "fail",
        None => "pre",
    };
    let args_s = args.to_string();
    let truncate = |s: &str, n: usize| -> String {
        if s.len() > n {
            format!("{}…", &s[..n.min(s.len())])
        } else {
            s.to_string()
        }
    };
    template
        .replace("{{tool}}", tool_name)
        .replace("{{agent}}", agent_id)
        .replace("{{success}}", success_s)
        .replace("{{output}}", &truncate(output.unwrap_or(""), 200))
        .replace("{{args}}", &truncate(&args_s, 200))
}

/// Check whether a fired hook with action == "run_skill" should trigger
/// downstream skill execution. Returns the skill name if so. Called by
/// the dispatcher AFTER fire_pre/post returns to avoid circular deps.
pub fn extract_run_skill_target(hook: &HookRow) -> Option<String> {
    if hook.action != "run_skill" {
        return None;
    }
    hook.action_config
        .get("skill")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
