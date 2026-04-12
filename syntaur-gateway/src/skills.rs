//! Skills registry — named, reusable workflows.
//!
//! Three kinds:
//!   - **Binary** — shell out to a configured executable. The skill body is
//!     the absolute path; user-supplied args are appended after argv\[0\].
//!   - **PromptTemplate** — expand `{{var}}` placeholders against user args
//!     and return the expanded text. Caller (typically the LLM via the
//!     `run_skill` tool) gets the expanded prompt back as the tool result
//!     and can act on it. This lets a skill be a reusable instruction
//!     library without needing real tool chaining.
//!   - **ToolChain** — JSON-encoded sequence of `{name, arguments}` calls.
//!     **Not yet supported** (would require a re-entrant ToolRegistry
//!     reference to dispatch each step). Returns an error for now.
//!
//! Schema lives in `src/index/schema.rs` v9 — table `skills`.
//!
//! ## rusqlite Send safety
//! `rusqlite::Statement<'_>` is `!Send` (it holds a `*mut sqlite3_stmt`).
//! Every method below either (a) returns immediately after the SQL with no
//! intervening `.await` on a different primitive, or (b) scopes the
//! statement in an inner block that drops before any cross-await. Don't
//! hold a Statement across `.await` or axum will reject the handler with
//! a confusing `Handler<_, _> not implemented` error. Same applies to
//! `plans.rs`, `slash.rs`, `tool_hooks.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    Binary,
    Prompt,
    ToolChain,
}

impl SkillKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Prompt => "prompt",
            Self::ToolChain => "tool_chain",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "binary" => Some(Self::Binary),
            "prompt" => Some(Self::Prompt),
            "tool_chain" => Some(Self::ToolChain),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub agent_id: String,
    pub kind: SkillKind,
    pub body: String,
    pub args_schema: Option<Value>,
    pub requires_approval: bool,
    pub created_at: i64,
}

pub struct SkillStore {
    db: Arc<Mutex<Connection>>,
}

impl SkillStore {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open skill store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[skills] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    pub async fn create(
        &self,
        name: &str,
        description: &str,
        agent_id: &str,
        kind: SkillKind,
        body: &str,
        args_schema: Option<&Value>,
        requires_approval: bool,
    ) -> Result<i64, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO skills (name, description, agent_id, kind, body, args_schema_json, requires_approval, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                name,
                description,
                agent_id,
                kind.as_str(),
                body,
                args_schema.map(|v| v.to_string()),
                if requires_approval { 1 } else { 0 },
                now
            ],
        )
        .map_err(|e| format!("insert skill: {}", e))?;
        Ok(db.last_insert_rowid())
    }

    pub async fn delete(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("DELETE FROM skills WHERE id = ?", params![id])
            .map_err(|e| format!("delete skill: {}", e))?;
        Ok(())
    }

    pub async fn list(&self, agent_filter: Option<&str>) -> Result<Vec<SkillRow>, String> {
        let db = self.db.lock().await;
        let (sql, has_filter) = match agent_filter {
            Some(_) => (
                "SELECT id, name, description, agent_id, kind, body, args_schema_json, \
                 requires_approval, created_at FROM skills WHERE agent_id = ? ORDER BY name",
                true,
            ),
            None => (
                "SELECT id, name, description, agent_id, kind, body, args_schema_json, \
                 requires_approval, created_at FROM skills ORDER BY name",
                false,
            ),
        };
        let mut stmt = db.prepare(sql).map_err(|e| format!("prep: {}", e))?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<SkillRow> {
            let kind_s: String = r.get(4)?;
            let args_schema_s: Option<String> = r.get(6)?;
            Ok(SkillRow {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                agent_id: r.get(3)?,
                kind: SkillKind::parse(&kind_s).unwrap_or(SkillKind::Prompt),
                body: r.get(5)?,
                args_schema: args_schema_s
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                requires_approval: r.get::<_, i64>(7)? != 0,
                created_at: r.get(8)?,
            })
        };
        let rows = if has_filter {
            stmt.query_map([agent_filter.unwrap()], map_row)
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

    pub async fn get_by_name(&self, name: &str) -> Result<Option<SkillRow>, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT id, name, description, agent_id, kind, body, args_schema_json, \
                 requires_approval, created_at FROM skills WHERE name = ?",
            )
            .map_err(|e| format!("prep: {}", e))?;
        let row = stmt
            .query_row([name], |r| {
                let kind_s: String = r.get(4)?;
                let args_schema_s: Option<String> = r.get(6)?;
                Ok(SkillRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    agent_id: r.get(3)?,
                    kind: SkillKind::parse(&kind_s).unwrap_or(SkillKind::Prompt),
                    body: r.get(5)?,
                    args_schema: args_schema_s
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok()),
                    requires_approval: r.get::<_, i64>(7)? != 0,
                    created_at: r.get(8)?,
                })
            })
            .ok();
        Ok(row)
    }

    /// Run a skill by name. Dispatches by `kind`:
    ///   - `Binary` → spawn the executable, append args, capture stdout
    ///   - `Prompt` → expand `{{key}}` placeholders against `args`
    ///   - `ToolChain` → not implemented (returns Err)
    pub async fn run(&self, name: &str, args: &Value) -> Result<String, String> {
        let skill = self
            .get_by_name(name)
            .await?
            .ok_or_else(|| format!("skill '{}' not found", name))?;
        match skill.kind {
            SkillKind::Binary => run_binary(&skill.body, args).await,
            SkillKind::Prompt => Ok(expand_template(&skill.body, args)),
            SkillKind::ToolChain => Err(
                "tool_chain skills are not yet supported (use kind=binary or kind=prompt)"
                    .to_string(),
            ),
        }
    }
}

/// Spawn a binary skill, passing string args from the JSON object as
/// `--key value` pairs. Captures stdout (truncated to 16KB) and returns it.
async fn run_binary(path: &str, args: &Value) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(path);
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            cmd.arg(format!("--{}", k));
            match v {
                Value::String(s) => cmd.arg(s),
                Value::Number(n) => cmd.arg(n.to_string()),
                Value::Bool(b) => cmd.arg(b.to_string()),
                other => cmd.arg(other.to_string()),
            };
        }
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("spawn '{}': {}", path, e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if !stdout.is_empty() {
        stdout
    } else {
        stderr
    };
    let trimmed = if combined.len() > 16384 {
        format!("{}\n…[truncated {} bytes]", &combined[..16384], combined.len() - 16384)
    } else {
        combined
    };
    if output.status.success() {
        Ok(trimmed)
    } else {
        Err(format!(
            "binary exited with status {}: {}",
            output.status, trimmed
        ))
    }
}

/// Expand `{{key}}` placeholders in a template against a JSON args object.
/// Missing keys are left as the literal placeholder so the LLM can see what
/// was unfilled rather than getting silent empty strings.
fn expand_template(template: &str, args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        return template.to_string();
    };
    let mut out = template.to_string();
    for (k, v) in obj {
        let needle = format!("{{{{{}}}}}", k);
        let val = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&needle, &val);
    }
    out
}

// ── run_skill tool ─────────────────────────────────────────────────────────
//
// Exposed to the LLM as a normal trait-based tool. Carries an Arc<SkillStore>
// internally so the dispatch funnel doesn't need a special case — it just
// calls execute() like any other Tool.

pub struct RunSkillTool {
    pub store: Arc<SkillStore>,
}

#[async_trait]
impl Tool for RunSkillTool {
    fn name(&self) -> &str {
        "run_skill"
    }
    fn description(&self) -> &str {
        "Run a registered skill by name. Skills are reusable workflows registered via the admin API. Pass the skill name and an args object — the kind (binary/prompt) determines how it's invoked."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Skill name (must already exist in the registry)"},
                "args": {"type": "object", "description": "Arguments passed to the skill — keys/values depend on the skill", "additionalProperties": true}
            },
            "required": ["name"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        // Conservative default — skills can do anything, so treat as
        // potentially destructive + non-idempotent. Specific skills with
        // requires_approval=1 will be gated by the funnel separately once
        // wired into the approval list.
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: false,
            requires_approval: None,
            circuit_name: Some("run_skill"),
            rate_limit: None,
        }
    }
    async fn execute(
        &self,
        args: Value,
        _ctx: &ToolContext<'_>,
    ) -> Result<RichToolResult, String> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "run_skill: 'name' is required".to_string())?;
        let skill_args = args.get("args").cloned().unwrap_or(json!({}));
        let out = self.store.run(name, &skill_args).await?;
        Ok(RichToolResult::text(out))
    }
}
