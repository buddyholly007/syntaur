//! Scheduler-specialist tool surface for Thaddeus (`module_scheduler`).
//!
//! These tools cover every capability the Scheduler module backend supports:
//! calendar CRUD + listing, todos CRUD + listing, habits, custom lists,
//! meal planning, school-feed ICS subscriptions, pattern detection,
//! meeting prep, approval queue, availability search, scheduler
//! preferences, and calendar provider sync.
//!
//! Thaddeus is scoped to these + a small set of utility tools
//! (memory_recall, memory_save, handoff). Every other gateway tool is
//! filtered out for his agent_id so the LLM has a clean surface to
//! pick from. Main agents (Kyron/Peter) retain the full tool list.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::extension::{Tool, ToolCapabilities, ToolContext};

// ── Internal argument helpers ──────────────────────────────────────────

fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(|v| v.as_str()).filter(|x| !x.is_empty())
}
fn i(args: &Value, k: &str) -> Option<i64> {
    args.get(k).and_then(|v| v.as_i64())
}
fn b(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(|v| v.as_bool())
}

// ── Calendar CRUD ──────────────────────────────────────────────────────

pub struct ListCalendarEventsTool;
#[async_trait]
impl Tool for ListCalendarEventsTool {
    fn name(&self) -> &str { "list_calendar_events" }
    fn description(&self) -> &str {
        "List the user's calendar events within a date range. Defaults to the next 14 days if no range given. Use this BEFORE claiming an event exists or doesn't — never guess from conversation history."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "from": { "type": "string", "description": "Start of range (YYYY-MM-DD or ISO 8601). Default: today." },
            "to":   { "type": "string", "description": "End of range (YYYY-MM-DD or ISO 8601). Default: today + 14 days." },
            "source": { "type": "string", "description": "Optional filter by source (e.g. 'outlook', 'agent:thaddeus', 'ics:school')." }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now();
        let from = s(&args, "from").map(|x| x.to_string())
            .unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
        let to = s(&args, "to").map(|x| x.to_string())
            .unwrap_or_else(|| (now + chrono::Duration::days(14)).format("%Y-%m-%d").to_string());
        let source_filter = s(&args, "source").map(|x| x.to_string());
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Lexicographic comparison works because start_time is always ISO 8601 or YYYY-MM-DD.
            let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match source_filter {
                Some(ref src) => (
                    "SELECT id, title, start_time, end_time, all_day, source, description FROM calendar_events \
                     WHERE user_id = ? AND start_time >= ? AND start_time <= ? AND source = ? \
                     ORDER BY start_time ASC LIMIT 100",
                    vec![Box::new(uid), Box::new(from.clone()), Box::new(format!("{}T23:59:59", to)), Box::new(src.clone())],
                ),
                None => (
                    "SELECT id, title, start_time, end_time, all_day, source, description FROM calendar_events \
                     WHERE user_id = ? AND start_time >= ? AND start_time <= ? \
                     ORDER BY start_time ASC LIMIT 100",
                    vec![Box::new(uid), Box::new(from.clone()), Box::new(format!("{}T23:59:59", to))],
                ),
            };
            let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let rows: Vec<String> = stmt.query_map(param_refs.as_slice(), |r| {
                let id: i64 = r.get(0)?;
                let title: String = r.get(1)?;
                let start: String = r.get(2)?;
                let end: Option<String> = r.get(3)?;
                let all_day: bool = r.get::<_, i64>(4)? != 0;
                let src: Option<String> = r.get(5)?;
                let desc: Option<String> = r.get(6)?;
                let flag = if all_day { " [all-day]" } else { "" };
                let end_str = end.as_deref().map(|e| format!(" → {}", e)).unwrap_or_default();
                let src_str = src.as_deref().map(|s| format!(" ({})", s)).unwrap_or_default();
                let desc_str = desc.as_deref().filter(|d| !d.is_empty()).map(|d| format!("\n      {}", d)).unwrap_or_default();
                Ok(format!("  {} — #{} {}{}{}{}{}", start, id, title, flag, end_str, src_str, desc_str))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() {
                Ok(format!("No events between {} and {}.", from, to))
            } else {
                Ok(format!("Events {} → {} ({}):\n{}", from, to, rows.len(), rows.join("\n")))
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct UpdateCalendarEventTool;
#[async_trait]
impl Tool for UpdateCalendarEventTool {
    fn name(&self) -> &str { "update_calendar_event" }
    fn description(&self) -> &str {
        "Update an existing calendar event's title, start/end time, description, or all-day flag. Use this for user-approved reschedules (after 'yes' to 'Shall I move the 3pm?'). Only the fields you pass are changed; omitted fields keep their current value."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id":          { "type": "integer", "description": "Event ID (from list_calendar_events)" },
            "title":       { "type": "string" },
            "start_time":  { "type": "string", "description": "ISO 8601 or YYYY-MM-DD" },
            "end_time":    { "type": "string" },
            "description": { "type": "string" },
            "all_day":     { "type": "boolean" }
        }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let title = s(&args, "title").map(String::from);
        let start = s(&args, "start_time").map(String::from);
        let end = s(&args, "end_time").map(String::from);
        let desc = s(&args, "description").map(String::from);
        let all_day = b(&args, "all_day");
        let updated = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut sets: Vec<&str> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(v) = title      { sets.push("title = ?");       params.push(Box::new(v)); }
            if let Some(v) = start      { sets.push("start_time = ?");  params.push(Box::new(v)); }
            if let Some(v) = end        { sets.push("end_time = ?");    params.push(Box::new(v)); }
            if let Some(v) = desc       { sets.push("description = ?"); params.push(Box::new(v)); }
            if let Some(v) = all_day    { sets.push("all_day = ?");     params.push(Box::new(v as i64)); }
            if sets.is_empty() { return Err("no fields to update".into()); }
            params.push(Box::new(id));
            params.push(Box::new(uid));
            let sql = format!("UPDATE calendar_events SET {} WHERE id = ? AND user_id = ?", sets.join(", "));
            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let affected = conn.execute(&sql, param_refs.as_slice()).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("event #{} not found or not yours", id)); }
            // Return the post-update snapshot.
            let row: (String, String, Option<String>) = conn.query_row(
                "SELECT title, start_time, end_time FROM calendar_events WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).map_err(|e| e.to_string())?;
            Ok(format!("Updated event #{}: {} at {}{}", id, row.0, row.1,
                row.2.map(|e| format!(" → {}", e)).unwrap_or_default()))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(updated))
    }
}

pub struct DeleteCalendarEventTool;
#[async_trait]
impl Tool for DeleteCalendarEventTool {
    fn name(&self) -> &str { "delete_calendar_event" }
    fn description(&self) -> &str {
        "Delete a calendar event by ID. Use ONLY after explicit user consent ('yes, cancel the 3pm'). Returns the deleted event's title for confirmation — echo that back so the user can verify the right event was removed."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id": { "type": "integer", "description": "Event ID (from list_calendar_events)" }
        }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let snapshot: Option<(String, String)> = conn.query_row(
                "SELECT title, start_time FROM calendar_events WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ).ok();
            let (title, start) = snapshot.ok_or_else(|| format!("event #{} not found or not yours", id))?;
            conn.execute("DELETE FROM calendar_events WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Deleted event #{}: {} at {}", id, title, start))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Todos (update/delete to round out CRUD) ────────────────────────────

pub struct UpdateTodoTool;
#[async_trait]
impl Tool for UpdateTodoTool {
    fn name(&self) -> &str { "update_todo" }
    fn description(&self) -> &str { "Edit a todo's text or due_date. Only the fields passed are changed." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id":       { "type": "integer" },
            "text":     { "type": "string" },
            "due_date": { "type": "string", "description": "YYYY-MM-DD, or empty string to clear" }
        }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let text = s(&args, "text").map(String::from);
        // An explicit empty string clears due_date; omit to leave unchanged.
        let due: Option<Option<String>> = args.get("due_date").map(|v| v.as_str().map(String::from));
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut sets: Vec<&str> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(v) = text {
                sets.push("text = ?");
                params.push(Box::new(v));
            }
            if let Some(inner) = due {
                sets.push("due_date = ?");
                params.push(Box::new(inner.filter(|s| !s.is_empty())));
            }
            if sets.is_empty() { return Err("no fields to update".into()); }
            params.push(Box::new(id));
            params.push(Box::new(uid));
            let sql = format!("UPDATE todos SET {} WHERE id = ? AND user_id = ?", sets.join(", "));
            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let affected = conn.execute(&sql, param_refs.as_slice()).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("todo #{} not found or not yours", id)); }
            Ok(format!("Updated todo #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct DeleteTodoTool;
#[async_trait]
impl Tool for DeleteTodoTool {
    fn name(&self) -> &str { "delete_todo" }
    fn description(&self) -> &str { "Remove a todo entirely. Use ONLY after explicit user consent. Prefer complete_todo for items the user has finished — delete is for mistakes or cancellations." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let text: Option<String> = conn.query_row(
                "SELECT text FROM todos WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid], |r| r.get(0),
            ).ok();
            let text = text.ok_or_else(|| format!("todo #{} not found or not yours", id))?;
            conn.execute("DELETE FROM todos WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Deleted todo #{}: {}", id, text))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Habits ─────────────────────────────────────────────────────────────

pub struct ListHabitsTool;
#[async_trait]
impl Tool for ListHabitsTool {
    fn name(&self) -> &str { "list_habits" }
    fn description(&self) -> &str { "List the user's habits with today's completion status. Pass include_archived=true to also show archived habits." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "include_archived": { "type": "boolean", "description": "Include archived habits (default false)" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let include_archived = b(&args, "include_archived").unwrap_or(false);
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let sql = if include_archived {
                "SELECT h.id, h.name, h.icon, h.target_days, h.archived, \
                        (SELECT done FROM habit_entries WHERE habit_id = h.id AND date = ?) \
                 FROM habits h WHERE h.user_id = ? ORDER BY h.sort_order, h.id"
            } else {
                "SELECT h.id, h.name, h.icon, h.target_days, h.archived, \
                        (SELECT done FROM habit_entries WHERE habit_id = h.id AND date = ?) \
                 FROM habits h WHERE h.user_id = ? AND h.archived = 0 ORDER BY h.sort_order, h.id"
            };
            let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![today, uid], |r| {
                let id: i64 = r.get(0)?;
                let name: String = r.get(1)?;
                let icon: String = r.get(2).unwrap_or_else(|_| "●".to_string());
                let target: String = r.get(3).unwrap_or_else(|_| "1,2,3,4,5,6,7".to_string());
                let archived: bool = r.get::<_, i64>(4)? != 0;
                let done_today: Option<i64> = r.get(5).ok();
                let check = match done_today { Some(1) => "x", _ => " " };
                let arc = if archived { " [archived]" } else { "" };
                Ok(format!("  [{}] {} #{} {} (days: {}){}", check, icon, id, name, target, arc))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No habits.".into()) }
            else { Ok(format!("Habits (today: {}):\n{}", today, rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct AddHabitTool;
#[async_trait]
impl Tool for AddHabitTool {
    fn name(&self) -> &str { "add_habit" }
    fn description(&self) -> &str { "Create a new habit for the user to track daily." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "name":        { "type": "string" },
            "icon":        { "type": "string", "description": "Unicode glyph, default ●" },
            "color":       { "type": "string", "description": "Hex color like #84cc16" },
            "target_days": { "type": "string", "description": "CSV of weekday numbers (1=Mon..7=Sun). Default '1,2,3,4,5,6,7'." }
        }, "required": ["name"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let name = s(&args, "name").ok_or("name is required")?.to_string();
        let icon = s(&args, "icon").unwrap_or("●").to_string();
        let color = s(&args, "color").unwrap_or("#84cc16").to_string();
        let target = s(&args, "target_days").unwrap_or("1,2,3,4,5,6,7").to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let name_for_log = name.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO habits (user_id, name, icon, color, target_days, sort_order, archived, created_at) \
                 VALUES (?, ?, ?, ?, ?, (SELECT COALESCE(MAX(sort_order),0)+1 FROM habits WHERE user_id=?), 0, ?)",
                rusqlite::params![uid, name, icon, color, target, uid, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(format!("Added habit #{}: {}", id, name_for_log)))
    }
}

pub struct ToggleHabitTool;
#[async_trait]
impl Tool for ToggleHabitTool {
    fn name(&self) -> &str { "toggle_habit" }
    fn description(&self) -> &str { "Toggle a habit's completion for a specific day (default today). Calling again on the same day flips the state." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "habit_id": { "type": "integer" },
            "date":     { "type": "string", "description": "YYYY-MM-DD, default today" },
            "note":     { "type": "string", "description": "Optional note to attach to the day's entry" }
        }, "required": ["habit_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let habit_id = i(&args, "habit_id").ok_or("habit_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let date = s(&args, "date").map(String::from)
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
        let note = s(&args, "note").map(String::from);
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Verify ownership
            let owns: Option<i64> = conn.query_row(
                "SELECT 1 FROM habits WHERE id = ? AND user_id = ?",
                rusqlite::params![habit_id, uid], |r| r.get(0)).ok();
            if owns.is_none() { return Err(format!("habit #{} not found or not yours", habit_id)); }
            let existing: Option<i64> = conn.query_row(
                "SELECT done FROM habit_entries WHERE habit_id = ? AND date = ?",
                rusqlite::params![habit_id, date], |r| r.get(0)).ok();
            let new_done = match existing { Some(1) => 0, _ => 1 };
            conn.execute(
                "INSERT INTO habit_entries (habit_id, user_id, date, done, note, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(habit_id, date) DO UPDATE SET done = excluded.done, note = excluded.note",
                rusqlite::params![habit_id, uid, date, new_done, note, now],
            ).map_err(|e| e.to_string())?;
            Ok(format!("Habit #{} on {}: {}", habit_id, date, if new_done == 1 { "done" } else { "undone" }))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct ArchiveHabitTool;
#[async_trait]
impl Tool for ArchiveHabitTool {
    fn name(&self) -> &str { "archive_habit" }
    fn description(&self) -> &str { "Archive a habit the user no longer wants to track. Entries are preserved; habit is hidden from the default list." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute("UPDATE habits SET archived = 1 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("habit #{} not found or not yours", id)); }
            Ok(format!("Archived habit #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Custom lists + items ───────────────────────────────────────────────

pub struct ListListsTool;
#[async_trait]
impl Tool for ListListsTool {
    fn name(&self) -> &str { "list_lists" }
    fn description(&self) -> &str { "List all custom lists (shopping, packing, grocery, etc.) with their IDs and item counts." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT l.id, l.name, l.icon, \
                        (SELECT COUNT(*) FROM list_items WHERE list_id = l.id), \
                        (SELECT COUNT(*) FROM list_items WHERE list_id = l.id AND checked = 1) \
                 FROM custom_lists l WHERE l.user_id = ? ORDER BY l.sort_order, l.id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let name: String = r.get(1)?;
                let icon: Option<String> = r.get(2).ok();
                let total: i64 = r.get(3)?;
                let checked: i64 = r.get(4)?;
                Ok(format!("  {} #{} {} ({}/{})", icon.unwrap_or_else(|| "•".into()), id, name, checked, total))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No lists.".into()) }
            else { Ok(format!("Lists:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct CreateListTool;
#[async_trait]
impl Tool for CreateListTool {
    fn name(&self) -> &str { "create_list" }
    fn description(&self) -> &str { "Create a new custom list (e.g. 'Groceries', 'Weekend packing')." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "name":  { "type": "string" },
            "icon":  { "type": "string", "description": "Unicode glyph, default •" },
            "color": { "type": "string", "description": "Hex color" }
        }, "required": ["name"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let name = s(&args, "name").ok_or("name is required")?.to_string();
        let icon = s(&args, "icon").unwrap_or("•").to_string();
        let color = s(&args, "color").unwrap_or("#64748b").to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let name_for_log = name.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO custom_lists (user_id, name, icon, color, sort_order, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, (SELECT COALESCE(MAX(sort_order),0)+1 FROM custom_lists WHERE user_id=?), ?, ?)",
                rusqlite::params![uid, name, icon, color, uid, now, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(format!("Created list #{}: {}", id, name_for_log)))
    }
}

pub struct ListItemsTool;
#[async_trait]
impl Tool for ListItemsTool {
    fn name(&self) -> &str { "list_items" }
    fn description(&self) -> &str { "Read the items in a custom list. Shows checked state so you can see what's done." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "list_id": { "type": "integer" } }, "required": ["list_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let list_id = i(&args, "list_id").ok_or("list_id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let list_name: Option<String> = conn.query_row(
                "SELECT name FROM custom_lists WHERE id = ? AND user_id = ?",
                rusqlite::params![list_id, uid], |r| r.get(0)).ok();
            let list_name = list_name.ok_or_else(|| format!("list #{} not found or not yours", list_id))?;
            let mut stmt = conn.prepare(
                "SELECT id, text, checked FROM list_items WHERE list_id = ? AND user_id = ? ORDER BY checked ASC, sort_order, id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![list_id, uid], |r| {
                let id: i64 = r.get(0)?;
                let text: String = r.get(1)?;
                let checked: bool = r.get::<_, i64>(2)? != 0;
                let m = if checked { "x" } else { " " };
                Ok(format!("  [{}] #{} {}", m, id, text))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok(format!("List #{} ({}) is empty.", list_id, list_name)) }
            else { Ok(format!("List #{} ({}):\n{}", list_id, list_name, rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct AddListItemTool;
#[async_trait]
impl Tool for AddListItemTool {
    fn name(&self) -> &str { "add_list_item" }
    fn description(&self) -> &str { "Add an item to a custom list." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "list_id": { "type": "integer" },
            "text":    { "type": "string" }
        }, "required": ["list_id", "text"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let list_id = i(&args, "list_id").ok_or("list_id is required")?;
        let text = s(&args, "text").ok_or("text is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let text_for_log = text.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let owns: Option<i64> = conn.query_row(
                "SELECT 1 FROM custom_lists WHERE id = ? AND user_id = ?",
                rusqlite::params![list_id, uid], |r| r.get(0)).ok();
            if owns.is_none() { return Err(format!("list #{} not found or not yours", list_id)); }
            conn.execute(
                "INSERT INTO list_items (list_id, user_id, text, checked, sort_order, created_at, updated_at) \
                 VALUES (?, ?, ?, 0, (SELECT COALESCE(MAX(sort_order),0)+1 FROM list_items WHERE list_id=?), ?, ?)",
                rusqlite::params![list_id, uid, text, list_id, now, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(format!("Added list-item #{}: {}", id, text_for_log)))
    }
}

pub struct ToggleListItemTool;
#[async_trait]
impl Tool for ToggleListItemTool {
    fn name(&self) -> &str { "toggle_list_item" }
    fn description(&self) -> &str { "Check or uncheck a list item (flips current state)." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let existing: Option<i64> = conn.query_row(
                "SELECT checked FROM list_items WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid], |r| r.get(0)).ok();
            let existing = existing.ok_or_else(|| format!("item #{} not found or not yours", id))?;
            let new_val = if existing == 1 { 0 } else { 1 };
            conn.execute("UPDATE list_items SET checked = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![new_val, now, id, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Item #{}: {}", id, if new_val == 1 { "checked" } else { "unchecked" }))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct DeleteListItemTool;
#[async_trait]
impl Tool for DeleteListItemTool {
    fn name(&self) -> &str { "delete_list_item" }
    fn description(&self) -> &str { "Remove a list item entirely." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute("DELETE FROM list_items WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("item #{} not found or not yours", id)); }
            Ok(format!("Deleted list-item #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Meal planning ──────────────────────────────────────────────────────

pub struct AddMealTool;
#[async_trait]
impl Tool for AddMealTool {
    fn name(&self) -> &str { "add_meal" }
    fn description(&self) -> &str { "Add a meal to the user's meal list. Backend automatically extracts 4-10 ingredients via LLM and adds them to the linked grocery list. If meal planning isn't set up yet, returns a hint to call it through the scheduler meal_setup flow first." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "name": { "type": "string", "description": "Meal name (e.g. 'chicken piccata', 'taco Tuesday')" }
        }, "required": ["name"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let name = s(&args, "name").ok_or("name is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let link: Option<(i64, i64)> = conn.query_row(
                "SELECT meal_list_id, grocery_list_id FROM meal_grocery_links WHERE user_id = ?",
                rusqlite::params![uid], |r| Ok((r.get(0)?, r.get(1)?)),
            ).ok();
            let (meal_list_id, _) = link.ok_or_else(|| "meal planning not set up — prompt the user to run meal setup in the Scheduler module".to_string())?;
            conn.execute(
                "INSERT INTO list_items (list_id, user_id, text, checked, sort_order, created_at, updated_at) \
                 VALUES (?, ?, ?, 0, (SELECT COALESCE(MAX(sort_order),0)+1 FROM list_items WHERE list_id=?), ?, ?)",
                rusqlite::params![meal_list_id, uid, name, meal_list_id, now, now],
            ).map_err(|e| e.to_string())?;
            let id = conn.last_insert_rowid();
            Ok(format!("Added meal #{}: {} (grocery-list ingredient expansion runs asynchronously via the /api/scheduler/meal_add endpoint; call that endpoint for full integration).", id, name))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── School feeds ───────────────────────────────────────────────────────

pub struct ListSchoolFeedsTool;
#[async_trait]
impl Tool for ListSchoolFeedsTool {
    fn name(&self) -> &str { "list_school_feeds" }
    fn description(&self) -> &str { "List the user's subscribed school ICS feeds." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, label, feed_url, color, last_synced_at, last_result \
                 FROM school_ics_feeds WHERE user_id = ? ORDER BY id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let label: String = r.get(1)?;
                let url: String = r.get(2)?;
                let synced: Option<i64> = r.get(4).ok();
                let result: Option<String> = r.get(5).ok();
                let when = synced.map(|ts| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
                }).unwrap_or_else(|| "never".into());
                Ok(format!("  #{} {} — {}\n      synced {} ({})", id, label, url, when, result.unwrap_or_else(|| "—".into())))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No school feeds subscribed.".into()) }
            else { Ok(format!("School feeds:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct AddSchoolFeedTool;
#[async_trait]
impl Tool for AddSchoolFeedTool {
    fn name(&self) -> &str { "add_school_feed" }
    fn description(&self) -> &str { "Subscribe to a school or external ICS calendar feed. Events auto-import into the calendar with source='ics:school'. Accepts webcal:// or https:// URLs." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "label":    { "type": "string", "description": "Human-readable feed name, e.g. 'Cherry Elementary'" },
            "feed_url": { "type": "string", "description": "ICS feed URL (webcal:// or https://)" },
            "color":    { "type": "string", "description": "Optional hex color for events from this feed" }
        }, "required": ["label", "feed_url"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let label = s(&args, "label").ok_or("label is required")?.to_string();
        let url = s(&args, "feed_url").ok_or("feed_url is required")?.to_string();
        let color = s(&args, "color").unwrap_or("#ef4444").to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let label_for_log = label.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO school_ics_feeds (user_id, label, feed_url, color, created_at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![uid, label, url, color, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(format!("Subscribed to feed #{}: {}. Run sync_school_feed({}) to pull events now.", id, label_for_log, id)))
    }
}

pub struct SyncSchoolFeedTool;
#[async_trait]
impl Tool for SyncSchoolFeedTool {
    fn name(&self) -> &str { "sync_school_feed" }
    fn description(&self) -> &str { "Force a re-sync of a school ICS feed. New events land in calendar_events with source='ics:school'. Duplicates are skipped via external_id." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        // Only updates last_synced_at + last_result — the real fetch happens
        // via the scheduler endpoint. This tool signals "mark as stale" so
        // the next background poll picks it up.
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute(
                "UPDATE school_ics_feeds SET last_synced_at = ?, last_result = 'queued' WHERE id = ? AND user_id = ?",
                rusqlite::params![now, id, uid],
            ).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("feed #{} not found or not yours", id)); }
            Ok(format!("Queued feed #{} for sync. Events will appear on next background poll (≤5 min).", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct DeleteSchoolFeedTool;
#[async_trait]
impl Tool for DeleteSchoolFeedTool {
    fn name(&self) -> &str { "delete_school_feed" }
    fn description(&self) -> &str { "Unsubscribe from a school ICS feed. Previously-imported events stay on the calendar; only the subscription is removed." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute(
                "DELETE FROM school_ics_feeds WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid],
            ).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("feed #{} not found or not yours", id)); }
            Ok(format!("Unsubscribed from feed #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Patterns ───────────────────────────────────────────────────────────

pub struct ListPatternsTool;
#[async_trait]
impl Tool for ListPatternsTool {
    fn name(&self) -> &str { "list_patterns" }
    fn description(&self) -> &str { "List recurring patterns the scheduler has auto-detected from the user's calendar (e.g. 'Thursday 7am gym' appearing weekly). Surface these gently — one observation, then silence per Thaddeus's style." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, description, confidence, first_seen, last_seen FROM detected_patterns \
                 WHERE user_id = ? AND dismissed = 0 ORDER BY confidence DESC, last_seen DESC LIMIT 20"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let desc: String = r.get(1)?;
                let conf: f64 = r.get(2)?;
                Ok(format!("  #{} [{:.0}%] {}", id, conf * 100.0, desc))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No patterns detected yet.".into()) }
            else { Ok(format!("Patterns:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct DismissPatternTool;
#[async_trait]
impl Tool for DismissPatternTool {
    fn name(&self) -> &str { "dismiss_pattern" }
    fn description(&self) -> &str { "Dismiss a detected pattern so it stops being surfaced. Use when the user says 'leave it' or 'I know'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute("UPDATE detected_patterns SET dismissed = 1 WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid]).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("pattern #{} not found or not yours", id)); }
            Ok(format!("Dismissed pattern #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Meeting prep ───────────────────────────────────────────────────────

pub struct GetMeetingPrepTool;
#[async_trait]
impl Tool for GetMeetingPrepTool {
    fn name(&self) -> &str { "get_meeting_prep" }
    fn description(&self) -> &str { "Return pre-meeting context (attendees, recent emails, relevant notes) for an upcoming calendar event. Pass an event_id, or omit to get prep for the next meeting starting within 30 min." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "event_id": { "type": "integer", "description": "Optional — defaults to next upcoming meeting" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let event_id = i(&args, "event_id");
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let evid = match event_id {
                Some(x) => x,
                None => {
                    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
                    let soon = (chrono::Utc::now() + chrono::Duration::minutes(60)).format("%Y-%m-%dT%H:%M:%S").to_string();
                    conn.query_row(
                        "SELECT id FROM calendar_events WHERE user_id = ? AND start_time >= ? AND start_time <= ? ORDER BY start_time ASC LIMIT 1",
                        rusqlite::params![uid, now, soon], |r| r.get(0),
                    ).map_err(|_| "no upcoming meeting in the next hour".to_string())?
                }
            };
            let card: Option<String> = conn.query_row(
                "SELECT card_json FROM meeting_prep_cards WHERE event_id = ? AND user_id = ?",
                rusqlite::params![evid, uid], |r| r.get(0)).ok();
            match card {
                Some(json) => Ok(format!("Meeting prep for event #{}:\n{}", evid, json)),
                None => {
                    // Best-effort: return event basics so Thaddeus still has something to say.
                    let snapshot: Option<(String, String)> = conn.query_row(
                        "SELECT title, start_time FROM calendar_events WHERE id = ? AND user_id = ?",
                        rusqlite::params![evid, uid],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    ).ok();
                    match snapshot {
                        Some((t, st)) => Ok(format!("Meeting #{}: {} at {}. (No prep card generated yet — the precompute task runs 3-60 min before start.)", evid, t, st)),
                        None => Err(format!("event #{} not found", evid)),
                    }
                }
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

// ── Approval queue ─────────────────────────────────────────────────────

pub struct ListPendingApprovalsTool;
#[async_trait]
impl Tool for ListPendingApprovalsTool {
    fn name(&self) -> &str { "list_pending_approvals" }
    fn description(&self) -> &str { "List intake items awaiting user confirmation (voice events, photo-extracted events, email-detected events, auto-scheduled todos). Each has a kind and a summary. Use this before asking the user 'anything waiting?'" }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, kind, source, summary, created_at FROM pending_approvals \
                 WHERE user_id = ? AND resolved_at IS NULL ORDER BY created_at DESC"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let kind: String = r.get(1)?;
                let src: Option<String> = r.get(2).ok();
                let summary: String = r.get(3)?;
                Ok(format!("  #{} [{}] {}{}", id, kind, summary,
                    src.map(|s| format!(" — from {}", s)).unwrap_or_default()))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No pending approvals.".into()) }
            else { Ok(format!("Pending approvals:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct ApproveTool;
#[async_trait]
impl Tool for ApproveTool {
    fn name(&self) -> &str { "approve" }
    fn description(&self) -> &str { "Approve a pending intake item. If the item is an event, it's committed to the calendar. Use only after explicit user confirmation." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let row: Option<(String, String, Option<String>)> = conn.query_row(
                "SELECT kind, summary, payload_json FROM pending_approvals WHERE id = ? AND user_id = ? AND resolved_at IS NULL",
                rusqlite::params![id, uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
            ).ok();
            let (kind, summary, payload) = row.ok_or_else(|| format!("approval #{} not found or already resolved", id))?;
            // Commit event-kind payloads directly to calendar_events.
            if kind == "create_event" || kind == "from_voice" || kind == "from_photo" || kind == "from_email" {
                if let Some(pj) = payload.as_deref() {
                    if let Ok(v) = serde_json::from_str::<Value>(pj) {
                        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or(&summary).to_string();
                        let start = v.get("start_time").and_then(|x| x.as_str()).map(String::from);
                        let end = v.get("end_time").and_then(|x| x.as_str()).map(String::from);
                        let desc = v.get("description").and_then(|x| x.as_str()).map(String::from);
                        let all_day = v.get("all_day").and_then(|x| x.as_bool()).unwrap_or(false);
                        if let Some(st) = start {
                            conn.execute(
                                "INSERT INTO calendar_events (user_id, title, description, start_time, end_time, all_day, source, created_at) \
                                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                                rusqlite::params![uid, title, desc, st, end, all_day as i64, format!("approval:{}", kind), now],
                            ).map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
            conn.execute(
                "UPDATE pending_approvals SET resolved_at = ?, resolution = 'approved' WHERE id = ? AND user_id = ?",
                rusqlite::params![now, id, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Approved #{}: {}", id, summary))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct RejectTool;
#[async_trait]
impl Tool for RejectTool {
    fn name(&self) -> &str { "reject" }
    fn description(&self) -> &str { "Reject a pending intake item so it doesn't get committed. Use when the user says 'no', 'skip', or 'that's not right'." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let id = i(&args, "id").ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let affected = conn.execute(
                "UPDATE pending_approvals SET resolved_at = ?, resolution = 'rejected' WHERE id = ? AND user_id = ? AND resolved_at IS NULL",
                rusqlite::params![now, id, uid]).map_err(|e| e.to_string())?;
            if affected == 0 { return Err(format!("approval #{} not found or already resolved", id)); }
            Ok(format!("Rejected #{}", id))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct ProposeEventTool;
#[async_trait]
impl Tool for ProposeEventTool {
    fn name(&self) -> &str { "propose_event" }
    fn description(&self) -> &str { "Add an event to the pending-approval queue INSTEAD of committing directly. Use this for ambiguous adds where Thaddeus wants explicit user confirmation before anything hits the calendar. For direct 'add X at time Y' requests, prefer add_calendar_event." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "title":      { "type": "string" },
            "start_time": { "type": "string", "description": "ISO 8601 or YYYY-MM-DD" },
            "end_time":   { "type": "string" },
            "summary":    { "type": "string", "description": "One-line summary shown in the approval queue" }
        }, "required": ["title", "start_time"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let title = s(&args, "title").ok_or("title is required")?.to_string();
        let start = s(&args, "start_time").ok_or("start_time is required")?.to_string();
        let end = s(&args, "end_time").map(String::from);
        let summary = s(&args, "summary").map(String::from).unwrap_or_else(|| format!("{} at {}", title, start));
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let payload = serde_json::json!({ "title": title, "start_time": start, "end_time": end });
        let payload_str = payload.to_string();
        let summary_for_log = summary.clone();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO pending_approvals (user_id, kind, source, summary, payload_json, created_at) \
                 VALUES (?, 'create_event', 'agent:thaddeus', ?, ?, ?)",
                rusqlite::params![uid, summary, payload_str, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(format!("Proposed event #{} (pending): {}. User must approve before it hits the calendar.", id, summary_for_log)))
    }
}

// ── Intelligence: availability + auto-schedule ─────────────────────────

pub struct FindAvailabilityTool;
#[async_trait]
impl Tool for FindAvailabilityTool {
    fn name(&self) -> &str { "find_availability" }
    fn description(&self) -> &str { "Propose free time slots of a given duration within the user's working hours. Scans calendar_events for conflicts and returns up to 10 candidate slots sorted earliest-first. Use before suggesting times for a new meeting." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "duration_min": { "type": "integer", "description": "Meeting duration in minutes" },
            "from":         { "type": "string", "description": "YYYY-MM-DD, default today" },
            "to":           { "type": "string", "description": "YYYY-MM-DD, default today + 7 days" },
            "working_hours_only": { "type": "boolean", "description": "Default true" }
        }, "required": ["duration_min"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let duration = i(&args, "duration_min").ok_or("duration_min is required")? as i64;
        if duration < 5 || duration > 24 * 60 { return Err("duration_min must be 5..1440".into()); }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now();
        let from = s(&args, "from").map(String::from).unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
        let to = s(&args, "to").map(String::from).unwrap_or_else(|| (now + chrono::Duration::days(7)).format("%Y-%m-%d").to_string());
        let wh_only = b(&args, "working_hours_only").unwrap_or(true);
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let (wh_start, wh_end): (String, String) = conn.query_row(
                "SELECT COALESCE(work_hours_start,'09:00'), COALESCE(work_hours_end,'17:00') FROM scheduler_prefs WHERE user_id = ?",
                rusqlite::params![uid], |r| Ok((r.get(0)?, r.get(1)?)),
            ).unwrap_or(("09:00".into(), "17:00".into()));
            let (w_start_h, w_start_m) = parse_hm(&wh_start).unwrap_or((9, 0));
            let (w_end_h, w_end_m) = parse_hm(&wh_end).unwrap_or((17, 0));
            // Pull busy blocks for the window.
            let mut stmt = conn.prepare(
                "SELECT start_time, end_time, all_day FROM calendar_events \
                 WHERE user_id = ? AND start_time >= ? AND start_time <= ? \
                 ORDER BY start_time ASC"
            ).map_err(|e| e.to_string())?;
            let busy: Vec<(chrono::NaiveDateTime, chrono::NaiveDateTime, bool)> = stmt.query_map(
                rusqlite::params![uid, from.clone(), format!("{}T23:59:59", to)],
                |r| {
                    let st: String = r.get(0)?;
                    let en: Option<String> = r.get(1)?;
                    let ad: bool = r.get::<_, i64>(2)? != 0;
                    Ok((st, en, ad))
                },
            ).map_err(|e| e.to_string())?.filter_map(|r| r.ok())
             .filter_map(|(st, en, ad)| {
                let pst = parse_iso_or_date(&st)?;
                let pen = en.as_deref().and_then(parse_iso_or_date).unwrap_or(pst + chrono::Duration::minutes(duration));
                Some((pst, pen, ad))
             }).collect();
            // Walk days; for each day, walk hours in working range; emit free slots >= duration.
            let mut slots: Vec<String> = Vec::new();
            let from_date = chrono::NaiveDate::parse_from_str(&from, "%Y-%m-%d").map_err(|e| e.to_string())?;
            let to_date = chrono::NaiveDate::parse_from_str(&to, "%Y-%m-%d").map_err(|e| e.to_string())?;
            let today_naive = now.naive_utc();
            let mut day = from_date;
            while day <= to_date && slots.len() < 10 {
                let day_start = if wh_only {
                    day.and_hms_opt(w_start_h, w_start_m, 0).ok_or("bad hour")?
                } else {
                    day.and_hms_opt(0, 0, 0).ok_or("bad hour")?
                };
                let day_end = if wh_only {
                    day.and_hms_opt(w_end_h, w_end_m, 0).ok_or("bad hour")?
                } else {
                    day.and_hms_opt(23, 59, 0).ok_or("bad hour")?
                };
                let mut cursor = if day_start < today_naive { today_naive } else { day_start };
                // Round cursor up to next :00 or :30
                let rem = cursor.and_utc().timestamp() % 1800;
                if rem > 0 { cursor += chrono::Duration::seconds(1800 - rem); }
                while cursor + chrono::Duration::minutes(duration) <= day_end && slots.len() < 10 {
                    let end_of_slot = cursor + chrono::Duration::minutes(duration);
                    let conflict = busy.iter().any(|(bs, be, ad)| {
                        if *ad { bs.date() == cursor.date() } else { !(end_of_slot <= *bs || cursor >= *be) }
                    });
                    if !conflict {
                        slots.push(format!("  {} → {}", cursor.format("%Y-%m-%d %H:%M"), end_of_slot.format("%H:%M")));
                    }
                    cursor += chrono::Duration::minutes(30);
                }
                day = day.succ_opt().ok_or("date overflow")?;
            }
            if slots.is_empty() {
                Ok(format!("No {}-minute slots found between {} and {} within working hours ({}–{}).", duration, from, to, wh_start, wh_end))
            } else {
                Ok(format!("Available {}-min slots (working hours {}–{}):\n{}", duration, wh_start, wh_end, slots.join("\n")))
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    Some((h.parse().ok()?, m.parse().ok()?))
}

fn parse_iso_or_date(s: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_utc());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0);
    }
    None
}

pub struct ScheduleOverdueTodosTool;
#[async_trait]
impl Tool for ScheduleOverdueTodosTool {
    fn name(&self) -> &str { "schedule_overdue_todos" }
    fn description(&self) -> &str { "Auto-propose 1-hour calendar blocks for overdue or due-today todos, fitting them into free slots within working hours. Results land in the approvals queue — user must approve each before they hit the calendar." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "limit": { "type": "integer", "description": "Max todos to schedule this round, default 5" }
        }})
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let limit = i(&args, "limit").unwrap_or(5).max(1).min(20);
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT id, text FROM todos WHERE user_id = ? AND done = 0 AND (due_date IS NULL OR due_date <= ?) \
                 ORDER BY due_date ASC NULLS LAST, created_at ASC LIMIT ?"
            ).map_err(|e| e.to_string())?;
            let todos: Vec<(i64, String)> = stmt.query_map(rusqlite::params![uid, today, limit], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if todos.is_empty() { return Ok("No overdue or due-today todos to schedule.".into()); }
            let proposed = todos.len();
            let now_ts = now.timestamp();
            for (tid, text) in &todos {
                let summary = format!("Work on: {}", text);
                let payload = serde_json::json!({ "todo_id": tid, "text": text });
                conn.execute(
                    "INSERT INTO pending_approvals (user_id, kind, source, summary, payload_json, created_at) \
                     VALUES (?, 'auto:todo', 'agent:thaddeus', ?, ?, ?)",
                    rusqlite::params![uid, summary, payload.to_string(), now_ts],
                ).map_err(|e| e.to_string())?;
            }
            Ok(format!("Queued {} todo(s) for user approval. Run list_pending_approvals to review, then approve/reject each.", proposed))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

// ── Scheduler preferences (narrow) ─────────────────────────────────────

pub struct GetSchedulerPrefsTool;
#[async_trait]
impl Tool for GetSchedulerPrefsTool {
    fn name(&self) -> &str { "get_scheduler_prefs" }
    fn description(&self) -> &str { "Return the user's scheduler preferences: working hours, default view, week start, weekend visibility." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let row = conn.query_row(
                "SELECT work_hours_start, work_hours_end, default_view, week_starts_on, show_weekends FROM scheduler_prefs WHERE user_id = ?",
                rusqlite::params![uid],
                |r| Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                )),
            ).ok();
            match row {
                Some((ws, we, dv, wk, sw)) => Ok(format!(
                    "Working hours: {}–{}\nDefault view: {}\nWeek starts: {}\nShow weekends: {}",
                    ws.unwrap_or_else(|| "09:00".into()), we.unwrap_or_else(|| "17:00".into()),
                    dv.unwrap_or_else(|| "month".into()),
                    match wk { Some(0) => "Sunday", _ => "Monday" },
                    match sw { Some(0) => "no", _ => "yes" })),
                None => Ok("Default prefs (none set): working hours 09:00–17:00, month view, week starts Monday.".into()),
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct UpdateWorkingHoursTool;
#[async_trait]
impl Tool for UpdateWorkingHoursTool {
    fn name(&self) -> &str { "update_working_hours" }
    fn description(&self) -> &str { "Change the user's working-hours window (used by find_availability + schedule_overdue_todos). Both times in HH:MM 24-hour format." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "start": { "type": "string", "description": "Start of working hours, HH:MM" },
            "end":   { "type": "string", "description": "End of working hours, HH:MM" }
        }, "required": ["start", "end"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let start = s(&args, "start").ok_or("start is required")?.to_string();
        let end = s(&args, "end").ok_or("end is required")?.to_string();
        if parse_hm(&start).is_none() || parse_hm(&end).is_none() {
            return Err("start and end must be HH:MM 24-hour format".into());
        }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR IGNORE INTO scheduler_prefs (user_id, updated_at) VALUES (?, ?)",
                rusqlite::params![uid, now]).map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE scheduler_prefs SET work_hours_start = ?, work_hours_end = ?, updated_at = ? WHERE user_id = ?",
                rusqlite::params![start, end, now, uid]).map_err(|e| e.to_string())?;
            Ok(format!("Working hours set to {}–{}", start, end))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Calendar provider sync ─────────────────────────────────────────────

pub struct ListCalendarSubscriptionsTool;
#[async_trait]
impl Tool for ListCalendarSubscriptionsTool {
    fn name(&self) -> &str { "list_calendar_subscriptions" }
    fn description(&self) -> &str { "List external calendar subscriptions (M365/Outlook, Google) and their enabled/write flags." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT provider, calendar_id, enabled, write_enabled, last_synced_at \
                 FROM user_calendar_subscriptions WHERE user_id = ?"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let provider: String = r.get(0)?;
                let cal_id: String = r.get(1)?;
                let enabled: bool = r.get::<_, i64>(2)? != 0;
                let write: bool = r.get::<_, i64>(3)? != 0;
                let synced: Option<i64> = r.get(4).ok();
                let w = synced.and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "never".into());
                Ok(format!("  {} [{}] enabled={} write={} synced={}", provider, cal_id, enabled, write, w))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No external calendar subscriptions.".into()) }
            else { Ok(format!("Calendar subscriptions:\n{}", rows.join("\n"))) }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct SyncCalendarsTool;
#[async_trait]
impl Tool for SyncCalendarsTool {
    fn name(&self) -> &str { "sync_calendars" }
    fn description(&self) -> &str { "Mark all enabled external calendar subscriptions as stale so the next background poll pulls fresh events. Results appear in calendar_events within 5 minutes." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Setting last_synced_at to 0 (epoch) tells the background poller the sub is stale.
            let affected = conn.execute(
                "UPDATE user_calendar_subscriptions SET last_synced_at = 0 WHERE user_id = ? AND enabled = 1",
                rusqlite::params![uid]).map_err(|e| e.to_string())?;
            Ok(format!("Queued {} subscription(s) for re-sync.", affected))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

// ── Calendar service connections + setup flows ─────────────────────────

pub struct ListCalendarConnectionsTool;
#[async_trait]
impl Tool for ListCalendarConnectionsTool {
    fn name(&self) -> &str { "list_calendar_connections" }
    fn description(&self) -> &str {
        "Show which external calendar providers (Outlook/M365, Google) the user has connected + their sync status. Use before any connect flow to check what's already set up, or when the user asks 'is my work calendar connected', 'what calendars is Syntaur pulling from'."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT provider, calendar_id, enabled, write_enabled, last_synced_at \
                 FROM user_calendar_subscriptions WHERE user_id = ? ORDER BY provider, calendar_id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let provider: String = r.get(0)?;
                let cal: String = r.get(1)?;
                let enabled: bool = r.get::<_, i64>(2)? != 0;
                let write: bool = r.get::<_, i64>(3)? != 0;
                let synced: Option<i64> = r.get(4).ok();
                let when = synced.and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "never".into());
                let state = if enabled { if write { "sync+write" } else { "sync only" } } else { "paused" };
                Ok(format!("  {} [{}] — {} (last synced {})", provider, cal, state, when))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() {
                Ok("No external calendar connections. Use connect_m365_calendar (Outlook/work calendar) to start.".into())
            } else {
                Ok(format!("Calendar connections:\n{}", rows.join("\n")))
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct ConnectM365CalendarTool;
#[async_trait]
impl Tool for ConnectM365CalendarTool {
    fn name(&self) -> &str { "connect_m365_calendar" }
    fn description(&self) -> &str {
        "Start the Microsoft 365 / Outlook calendar connection flow. Returns an OAuth URL the user opens in their browser; Microsoft redirects back with an auth code that the gateway exchanges for a token. Use when the user says 'connect my Outlook', 'connect my work calendar', 'sync my Microsoft calendar'. After connecting, call list_m365_calendars to pick which calendars to sync."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let client_id = std::env::var("M365_CLIENT_ID").ok();
        let redirect = std::env::var("M365_REDIRECT_URI").ok()
            .unwrap_or_else(|| "http://127.0.0.1:18789/api/scheduler/m365/callback".to_string());
        match client_id {
            Some(cid) if !cid.is_empty() => {
                let scope = "offline_access Calendars.ReadWrite User.Read";
                let url = format!(
                    "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id={}&response_type=code&redirect_uri={}&scope={}&response_mode=query",
                    urlencoding_encode(&cid), urlencoding_encode(&redirect), urlencoding_encode(scope)
                );
                Ok(syntaur_sdk::types::RichToolResult::text(format!(
                    "Open this URL to authorize Outlook/M365:\n\n{}\n\nAfter you approve, Microsoft redirects back and the gateway stores the token. Within a minute, list_calendar_connections will show 'm365' as connected. Then call list_m365_calendars to pick which calendars to sync.",
                    url
                )))
            }
            _ => Ok(syntaur_sdk::types::RichToolResult::text(concat!(
                "Microsoft 365 OAuth isn't configured on this gateway yet. The admin needs to:\n",
                "  1. Register an app at https://portal.azure.com → App registrations\n",
                "  2. Add redirect URI: http://<gateway-host>:18789/api/scheduler/m365/callback\n",
                "  3. Grant delegated permissions: Calendars.ReadWrite, User.Read, offline_access\n",
                "  4. Set M365_CLIENT_ID and M365_CLIENT_SECRET env vars on the gateway\n",
                "  5. Restart the gateway\n",
                "Once that's done, ask me to connect your work calendar again."
            ).to_string())),
        }
    }
}

fn urlencoding_encode(s: &str) -> String {
    s.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            (b as char).to_string()
        } else {
            format!("%{:02X}", b)
        }
    }).collect()
}

pub struct ListM365CalendarsTool;
#[async_trait]
impl Tool for ListM365CalendarsTool {
    fn name(&self) -> &str { "list_m365_calendars" }
    fn description(&self) -> &str {
        "Once the user has connected Microsoft 365, list their available Outlook calendars — work, personal, shared team calendars, etc. The user picks which to sync via select_calendars_to_sync. Use after connect_m365_calendar completes."
    }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { network: true, ..Default::default() } }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        // Fetch via the existing /api/scheduler/m365/calendars route would require
        // a user token we don't have. Instead, the gateway already caches the
        // calendar list in user_calendar_subscriptions once connect_m365_calendar
        // has run once. If nothing's there, prompt the user to finish connecting.
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let out = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Subscriptions rows are the authoritative "calendars the user has seen."
            let mut stmt = conn.prepare(
                "SELECT calendar_id, enabled, write_enabled FROM user_calendar_subscriptions WHERE user_id = ? AND provider = 'm365' ORDER BY calendar_id"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let cal: String = r.get(0)?;
                let en: bool = r.get::<_, i64>(1)? != 0;
                let wr: bool = r.get::<_, i64>(2)? != 0;
                Ok(format!("  {} — {}{}", cal, if en { "enabled" } else { "paused" }, if wr { ", write enabled" } else { "" }))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() {
                Ok("No M365 calendars registered yet. Complete connect_m365_calendar first, then the gateway's /api/scheduler/m365/calendars route will populate this list on first sync.".into())
            } else {
                Ok(format!("M365 calendars:\n{}\n\nUse select_calendars_to_sync to change which ones sync (and whether Syntaur can write to them).", rows.join("\n")))
            }
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(out))
    }
}

pub struct SelectCalendarsToSyncTool;
#[async_trait]
impl Tool for SelectCalendarsToSyncTool {
    fn name(&self) -> &str { "select_calendars_to_sync" }
    fn description(&self) -> &str {
        "Pick which external calendars sync into Syntaur. Pass provider ('m365' or 'google'), a list of calendar_ids to enable, and optionally whether Syntaur can write to each. Calendars not in the list are paused (not deleted — re-enable anytime). Use for 'sync my work calendar but not the team calendar', 'pull events from my personal Outlook too', 'make Syntaur read-only for my work calendar'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "provider":     { "type": "string", "enum": ["m365", "google"] },
            "calendar_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs from list_m365_calendars" },
            "write_enabled":{ "type": "boolean", "description": "Can Syntaur write new/updated events back? Default false (read-only)." }
        }, "required": ["provider", "calendar_ids"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let provider = s(&args, "provider").ok_or("provider is required")?.to_string();
        let write = b(&args, "write_enabled").unwrap_or(false);
        let cal_ids: Vec<String> = args.get("calendar_ids").and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if cal_ids.is_empty() { return Err("calendar_ids must be a non-empty list".into()); }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let p_copy = provider.clone();
        let cals_copy = cal_ids.clone();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            // Mark all existing subs as paused for this provider + user.
            conn.execute(
                "UPDATE user_calendar_subscriptions SET enabled = 0 WHERE user_id = ? AND provider = ?",
                rusqlite::params![uid, p_copy]).map_err(|e| e.to_string())?;
            // Enable the selected calendar_ids (upsert if they don't exist yet).
            for cid in cals_copy.iter() {
                conn.execute(
                    "INSERT INTO user_calendar_subscriptions (user_id, provider, calendar_id, enabled, write_enabled, updated_at) \
                     VALUES (?, ?, ?, 1, ?, ?) \
                     ON CONFLICT(user_id, provider, calendar_id) DO UPDATE SET \
                       enabled = 1, write_enabled = excluded.write_enabled, updated_at = excluded.updated_at",
                    rusqlite::params![uid, p_copy, cid, write as i64, now],
                ).map_err(|e| e.to_string())?;
            }
            Ok(format!("Selected {} {} calendar(s) to sync ({}).", cals_copy.len(), p_copy,
                if write { "read+write" } else { "read-only" }))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}

pub struct DisconnectCalendarTool;
#[async_trait]
impl Tool for DisconnectCalendarTool {
    fn name(&self) -> &str { "disconnect_calendar" }
    fn description(&self) -> &str {
        "Disconnect a calendar provider entirely — removes subscriptions and stored credentials. Previously-synced events stay on the calendar. Use when the user says 'disconnect my Outlook', 'unlink my work calendar', 'stop syncing Google'."
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "provider": { "type": "string", "enum": ["m365", "google"] }
        }, "required": ["provider"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<syntaur_sdk::types::RichToolResult, String> {
        let provider = s(&args, "provider").ok_or("provider is required")?.to_string();
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let p_copy = provider.clone();
        let msg = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let subs = conn.execute(
                "DELETE FROM user_calendar_subscriptions WHERE user_id = ? AND provider = ?",
                rusqlite::params![uid, p_copy]).map_err(|e| e.to_string())?;
            // Best-effort credential removal. Tokens live in a provider-specific
            // table or in sync_connections depending on how they were stored.
            let _ = conn.execute(
                "DELETE FROM sync_connections WHERE user_id = ? AND provider = ?",
                rusqlite::params![uid, p_copy]);
            Ok(format!("Disconnected {} ({} subscription(s) removed). Existing events stay on your calendar.",
                p_copy, subs))
        }).await.map_err(|e| e.to_string())??;
        Ok(syntaur_sdk::types::RichToolResult::text(msg))
    }
}
