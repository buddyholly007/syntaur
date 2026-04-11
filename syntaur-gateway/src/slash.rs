//! Slash commands — short user-invocable shortcuts.
//!
//! Three kinds:
//!   - `direct` — POST to a known internal endpoint, no LLM round-trip.
//!     `body_template` is the relative path (`/api/admin/users`, etc.) and
//!     args are forwarded as the JSON body.
//!   - `text_prompt` — expand `{{var}}` placeholders against args and post
//!     the result as a normal user message to the agent. The Telegram
//!     poller (`telegram::run_bot`) calls into this when it sees a leading
//!     `/` and the rest matches a registered name.
//!   - `skill_ref` — invoke a registered skill by name (from the skills
//!     table). `body_template` is the skill name. Args become skill args.
//!
//! ## Why a separate table from skills
//! A slash command is a *user shortcut*; a skill is a *named workflow*.
//! Many slash commands wrap skills (kind=skill_ref), but plenty don't:
//! `/clear`, `/status`, `/help`, `/plan` etc are all `direct` or
//! `text_prompt`. Keeping the tables separate means the slash registry
//! stays small and predictable; skills can grow without polluting the
//! command surface.
//!
//! ## Schema
//! `slash_commands` table in `src/index/schema.rs` v9.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashKind {
    Direct,
    TextPrompt,
    SkillRef,
}

impl SlashKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::TextPrompt => "text_prompt",
            Self::SkillRef => "skill_ref",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "direct" => Some(Self::Direct),
            "text_prompt" => Some(Self::TextPrompt),
            "skill_ref" => Some(Self::SkillRef),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SlashCommandRow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub agent_filter: Option<String>,
    pub kind: SlashKind,
    pub body_template: String,
    pub args_schema: Option<Value>,
    pub created_at: i64,
}

pub struct SlashStore {
    db: Arc<Mutex<Connection>>,
}

impl SlashStore {
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open slash store {}: {}", db_path.display(), e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {}", e))?;
        info!("[slash] opened {}", db_path.display());
        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
        }))
    }

    pub async fn create(
        &self,
        name: &str,
        description: &str,
        agent_filter: Option<&str>,
        kind: SlashKind,
        body_template: &str,
        args_schema: Option<&Value>,
    ) -> Result<i64, String> {
        let now = Utc::now().timestamp();
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO slash_commands (name, description, agent_filter, kind, body_template, args_schema_json, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                name,
                description,
                agent_filter,
                kind.as_str(),
                body_template,
                args_schema.map(|v| v.to_string()),
                now
            ],
        )
        .map_err(|e| format!("insert slash: {}", e))?;
        Ok(db.last_insert_rowid())
    }

    pub async fn delete(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().await;
        db.execute("DELETE FROM slash_commands WHERE id = ?", params![id])
            .map_err(|e| format!("delete slash: {}", e))?;
        Ok(())
    }

    pub async fn list(&self, agent_filter: Option<&str>) -> Result<Vec<SlashCommandRow>, String> {
        let db = self.db.lock().await;
        // Slash commands with agent_filter = NULL apply to all agents,
        // so when filtering by agent we still want to include the NULL rows.
        let (sql, has_filter) = match agent_filter {
            Some(_) => (
                "SELECT id, name, description, agent_filter, kind, body_template, \
                 args_schema_json, created_at FROM slash_commands \
                 WHERE agent_filter IS NULL OR agent_filter = ? ORDER BY name",
                true,
            ),
            None => (
                "SELECT id, name, description, agent_filter, kind, body_template, \
                 args_schema_json, created_at FROM slash_commands ORDER BY name",
                false,
            ),
        };
        let mut stmt = db.prepare(sql).map_err(|e| format!("prep: {}", e))?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<SlashCommandRow> {
            let kind_s: String = r.get(4)?;
            let args_schema_s: Option<String> = r.get(6)?;
            Ok(SlashCommandRow {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                agent_filter: r.get(3)?,
                kind: SlashKind::parse(&kind_s).unwrap_or(SlashKind::TextPrompt),
                body_template: r.get(5)?,
                args_schema: args_schema_s
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                created_at: r.get(7)?,
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

    pub async fn get_by_name(
        &self,
        name: &str,
        agent_filter: Option<&str>,
    ) -> Result<Option<SlashCommandRow>, String> {
        let db = self.db.lock().await;
        // Prefer agent-specific match; fall back to NULL agent_filter (global).
        let mut stmt = db
            .prepare(
                "SELECT id, name, description, agent_filter, kind, body_template, \
                 args_schema_json, created_at FROM slash_commands \
                 WHERE name = ? AND (agent_filter IS NULL OR agent_filter = ?) \
                 ORDER BY agent_filter IS NULL LIMIT 1",
            )
            .map_err(|e| format!("prep: {}", e))?;
        let agent = agent_filter.unwrap_or("");
        let row = stmt
            .query_row(params![name, agent], |r| {
                let kind_s: String = r.get(4)?;
                let args_schema_s: Option<String> = r.get(6)?;
                Ok(SlashCommandRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    agent_filter: r.get(3)?,
                    kind: SlashKind::parse(&kind_s).unwrap_or(SlashKind::TextPrompt),
                    body_template: r.get(5)?,
                    args_schema: args_schema_s
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok()),
                    created_at: r.get(7)?,
                })
            })
            .ok();
        Ok(row)
    }
}

/// Expand `{{var}}` placeholders in a template against a JSON object.
/// Identical to skills::expand_template but lifted out so slash commands
/// don't depend on the skills module.
pub fn expand_template(template: &str, args: &Value) -> String {
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
