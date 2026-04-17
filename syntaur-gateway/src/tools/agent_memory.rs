//! Agent memory tools — persistent, typed, cross-session knowledge.
//!
//! Five tools that let agents accumulate and recall structured memories:
//!   memory_save   — create or update a memory by key
//!   memory_recall — FTS5 search across memories
//!   memory_list   — compact index of all memories
//!   memory_update — partial update of an existing memory
//!   memory_forget — delete a memory
//!
//! Memories are scoped per-user + per-agent. The sharing model:
//!   - Main agent reads all except journal
//!   - Specialists read own + shared + main's user/feedback
//!   - Journal agent reads own only

use async_trait::async_trait;
use serde_json::{json, Value};

use super::extension::{Tool, ToolCapabilities, ToolContext, RichToolResult};

fn arg_str<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Build a WHERE clause for memory scoping based on the calling agent.
fn agent_scope_sql(agent_id: &str, user_id: i64) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    let clause = if agent_id == "main" {
        // Main agent reads everything except journal
        "m.user_id = ? AND m.agent_id != 'journal'".to_string()
    } else if agent_id == "journal" {
        // Journal reads only own
        params.push(Box::new(agent_id.to_string()));
        "m.user_id = ? AND m.agent_id = ?".to_string()
    } else {
        // Specialist reads own + shared + main's user/feedback
        params.push(Box::new(agent_id.to_string()));
        "(m.user_id = ? AND (m.agent_id = ? OR m.shared = 1 OR (m.agent_id = 'main' AND m.memory_type IN ('user','feedback'))))".to_string()
    };

    (clause, params)
}

// ── memory_save ─────────────────────────────────────────────────────────────

pub struct MemorySaveTool;

#[async_trait]
impl Tool for MemorySaveTool {
    fn name(&self) -> &str { "memory_save" }

    fn description(&self) -> &str {
        "Save a persistent memory that survives across conversations. \
         If a memory with the same key already exists, it is updated in place. \
         Types: user (preference), feedback (correction), project (state), \
         reference (pointer), fact (learned), insight (pattern), state (system)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["user","feedback","project","reference","fact","insight","state"], "description": "Memory type"},
                "key": {"type": "string", "description": "Topic key (like a filename). Unique per agent."},
                "title": {"type": "string", "description": "Human-readable title"},
                "content": {"type": "string", "description": "The memory content"},
                "description": {"type": "string", "description": "One-line summary for the index"},
                "tags": {"type": "string", "description": "Comma-separated tags"},
                "importance": {"type": "integer", "description": "1-10 importance for recall ranking"},
                "shared": {"type": "boolean", "description": "If true, visible to main agent + other specialists"}
            },
            "required": ["type", "key", "title", "content"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("No database path")?;
        let mem_type = arg_str(&args, "type");
        let key = arg_str(&args, "key");
        let title = arg_str(&args, "title");
        let content = arg_str(&args, "content");
        let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let tags = arg_str(&args, "tags");
        let importance = arg_i64(&args, "importance", 5);
        let shared = arg_bool(&args, "shared");

        if key.is_empty() || title.is_empty() || content.is_empty() {
            return Err("key, title, and content are required".to_string());
        }

        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO agent_memories \
             (user_id, agent_id, memory_type, key, title, description, content, tags, \
              importance, shared, source, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'agent_learned', ?11, ?11) \
             ON CONFLICT(user_id, agent_id, key) DO UPDATE SET \
               title = excluded.title, \
               description = excluded.description, \
               content = excluded.content, \
               tags = excluded.tags, \
               memory_type = excluded.memory_type, \
               importance = excluded.importance, \
               shared = excluded.shared, \
               updated_at = excluded.updated_at",
            rusqlite::params![
                ctx.user_id, ctx.agent_id, mem_type, key, title, description, content,
                tags, importance, shared as i64, now
            ],
        ).map_err(|e| format!("save memory: {}", e))?;

        Ok(RichToolResult::text(format!("Memory saved: [{}] {} — {}", mem_type, key, title)))
    }
}

// ── memory_recall ───────────────────────────────────────────────────────────

pub struct MemoryRecallTool;

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str { "memory_recall" }

    fn description(&self) -> &str {
        "Search your persistent memories by keyword. Returns matching memories \
         ranked by relevance. Respects agent privacy boundaries."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search keywords"},
                "type": {"type": "string", "description": "Optional: filter by memory type"},
                "limit": {"type": "integer", "description": "Max results (default 10)"}
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("No database path")?;
        let query = arg_str(&args, "query");
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let limit = arg_i64(&args, "limit", 10);

        if query.is_empty() {
            return Err("query is required".to_string());
        }

        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;

        let (scope_clause, mut scope_params) = agent_scope_sql(ctx.agent_id, ctx.user_id);

        // FTS5 search
        let sanitized = query.replace('"', "").replace('\'', "");
        let fts_query = sanitized.split_whitespace()
            .map(|w| format!("\"{}\"", w))
            .collect::<Vec<_>>()
            .join(" OR ");

        let mut sql = format!(
            "SELECT m.key, m.memory_type, m.title, m.description, m.content, \
                    m.importance, m.confidence, m.updated_at, m.access_count \
             FROM agent_memories m \
             JOIN agent_memories_fts f ON f.rowid = m.id \
             WHERE {} AND agent_memories_fts MATCH ?",
            scope_clause
        );
        scope_params.push(Box::new(fts_query));

        if let Some(t) = type_filter {
            sql.push_str(" AND m.memory_type = ?");
            scope_params.push(Box::new(t.to_string()));
        }

        sql.push_str(&format!(" ORDER BY m.importance DESC, m.updated_at DESC LIMIT {}", limit));

        let refs: Vec<&dyn rusqlite::ToSql> = scope_params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;
        let results: Vec<String> = stmt.query_map(refs.as_slice(), |r| {
            let key: String = r.get(0)?;
            let mtype: String = r.get(1)?;
            let title: String = r.get(2)?;
            let desc: Option<String> = r.get(3)?;
            let content: String = r.get(4)?;
            let importance: i64 = r.get(5)?;
            let confidence: f64 = r.get(6)?;
            let updated: i64 = r.get(7)?;
            let access: i64 = r.get(8)?;

            let age_days = (chrono::Utc::now().timestamp() - updated) / 86400;
            let stale = if age_days > 90 { " [STALE — verify]" } else { "" };

            Ok(format!(
                "[{}] {} — {}{}\n  {}\n  importance={}, confidence={:.1}, accessed={}x, {}d old",
                mtype, key, title, stale,
                if content.len() > 200 { format!("{}...", &content[..197]) } else { content },
                importance, confidence, access, age_days
            ))
        }).map_err(|e| format!("query: {}", e))?
        .filter_map(Result::ok)
        .collect();

        // Update access counts
        for r in &results {
            if let Some(key) = r.split(']').nth(1).and_then(|s| s.split('—').next()).map(|s| s.trim()) {
                let _ = conn.execute(
                    "UPDATE agent_memories SET access_count = access_count + 1, last_accessed_at = ? WHERE user_id = ? AND agent_id = ? AND key = ?",
                    rusqlite::params![chrono::Utc::now().timestamp(), ctx.user_id, ctx.agent_id, key],
                );
            }
        }

        if results.is_empty() {
            Ok(RichToolResult::text(format!("No memories matching '{}' in your scope.", query)))
        } else {
            Ok(RichToolResult::text(format!("{} memories found:\n\n{}", results.len(), results.join("\n\n"))))
        }
    }
}

// ── memory_list ─────────────────────────────────────────────────────────────

pub struct MemoryListTool;

#[async_trait]
impl Tool for MemoryListTool {
    fn name(&self) -> &str { "memory_list" }

    fn description(&self) -> &str {
        "List all your persistent memories as a compact index. Optionally filter by type."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "description": "Optional: filter by type (user/feedback/project/reference/fact/insight/state)"}
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("No database path")?;
        let type_filter = args.get("type").and_then(|v| v.as_str());

        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let (scope_clause, mut scope_params) = agent_scope_sql(ctx.agent_id, ctx.user_id);

        let mut sql = format!(
            "SELECT m.key, m.memory_type, m.title, m.description, m.importance, m.updated_at, m.agent_id \
             FROM agent_memories m WHERE {}", scope_clause
        );
        if let Some(t) = type_filter {
            sql.push_str(" AND m.memory_type = ?");
            scope_params.push(Box::new(t.to_string()));
        }
        sql.push_str(" ORDER BY m.memory_type, m.importance DESC");

        let refs: Vec<&dyn rusqlite::ToSql> = scope_params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;
        let lines: Vec<String> = stmt.query_map(refs.as_slice(), |r| {
            let key: String = r.get(0)?;
            let mtype: String = r.get(1)?;
            let title: String = r.get(2)?;
            let desc: Option<String> = r.get(3)?;
            let importance: i64 = r.get(4)?;
            let agent: String = r.get(6)?;
            let source_tag = if agent != ctx.agent_id { format!(" (from {})", agent) } else { String::new() };
            Ok(format!("[{}] {} — {}{}", mtype, key, desc.as_deref().unwrap_or(&title), source_tag))
        }).map_err(|e| format!("query: {}", e))?
        .filter_map(Result::ok)
        .collect();

        if lines.is_empty() {
            Ok(RichToolResult::text("No memories saved yet. Use memory_save to create one.".to_string()))
        } else {
            Ok(RichToolResult::text(format!("{} memories:\n{}", lines.len(), lines.join("\n"))))
        }
    }
}

// ── memory_update ───────────────────────────────────────────────────────────

pub struct MemoryUpdateTool;

#[async_trait]
impl Tool for MemoryUpdateTool {
    fn name(&self) -> &str { "memory_update" }

    fn description(&self) -> &str {
        "Update an existing memory's content, tags, or importance. The key must already exist."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "The memory key to update"},
                "content": {"type": "string", "description": "New content (replaces existing)"},
                "description": {"type": "string", "description": "New one-line description"},
                "tags": {"type": "string", "description": "New tags (replaces existing)"},
                "importance": {"type": "integer", "description": "New importance (1-10)"}
            },
            "required": ["key"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("No database path")?;
        let key = arg_str(&args, "key");
        if key.is_empty() { return Err("key is required".to_string()); }

        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp();

        let mut sets = vec!["updated_at = ?".to_string()];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];

        if let Some(c) = args.get("content").and_then(|v| v.as_str()) {
            sets.push("content = ?".to_string()); params.push(Box::new(c.to_string()));
        }
        if let Some(d) = args.get("description").and_then(|v| v.as_str()) {
            sets.push("description = ?".to_string()); params.push(Box::new(d.to_string()));
        }
        if let Some(t) = args.get("tags").and_then(|v| v.as_str()) {
            sets.push("tags = ?".to_string()); params.push(Box::new(t.to_string()));
        }
        if let Some(i) = args.get("importance").and_then(|v| v.as_i64()) {
            sets.push("importance = ?".to_string()); params.push(Box::new(i));
        }

        params.push(Box::new(ctx.user_id));
        params.push(Box::new(ctx.agent_id.to_string()));
        params.push(Box::new(key.to_string()));

        let sql = format!(
            "UPDATE agent_memories SET {} WHERE user_id = ? AND agent_id = ? AND key = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let updated = conn.execute(&sql, refs.as_slice()).map_err(|e| format!("update: {}", e))?;

        if updated == 0 {
            Err(format!("No memory found with key '{}' for this agent", key))
        } else {
            Ok(RichToolResult::text(format!("Memory updated: {}", key)))
        }
    }
}

// ── memory_forget ───────────────────────────────────────────────────────────

pub struct MemoryForgetTool;

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &str { "memory_forget" }

    fn description(&self) -> &str {
        "Delete a memory by key. Use when the user says 'forget that' or when a fact is no longer true."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "The memory key to delete"}
            },
            "required": ["key"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("No database path")?;
        let key = arg_str(&args, "key");
        if key.is_empty() { return Err("key is required".to_string()); }

        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let deleted = conn.execute(
            "DELETE FROM agent_memories WHERE user_id = ? AND agent_id = ? AND key = ?",
            rusqlite::params![ctx.user_id, ctx.agent_id, key],
        ).map_err(|e| format!("delete: {}", e))?;

        if deleted == 0 {
            Err(format!("No memory found with key '{}'", key))
        } else {
            Ok(RichToolResult::text(format!("Memory forgotten: {}", key)))
        }
    }
}
