//! Per-chat agent settings — backs the card-flip settings panel mounted on
//! every chat surface (see vault/projects/syntaur_per_chat_settings.md).
//!
//! Resolution rule: NULL columns fall back to the persona defaults baked
//! into agents/defaults.rs. Only fields the user has explicitly edited
//! become non-NULL. This keeps existing users' experience unchanged on
//! rollout — no row in `agent_settings` means "use the persona defaults
//! verbatim".
//!
//! Schema lives in `index/schema.rs` v69. CRUD here is intentionally
//! field-by-field rather than struct-level so partial PUTs don't clobber
//! columns the client didn't send.
//!
//! Phase 0 (this commit) ships the CRUD + the resolved struct that
//! `/api/agents/{id}/settings GET` returns. Phase 1 wires the GET/PUT
//! handlers to actual UI; later phases add the per-section payloads.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Stored row — every column nullable so absence means "inherit default".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentSettings {
    pub user_id: i64,
    pub agent_id: String,
    // Identity
    pub display_name: Option<String>,
    pub icon_blob_id: Option<i64>,
    pub accent_color: Option<String>,
    pub wake_phrase: Option<String>,
    pub shortcut: Option<String>,
    // Brain / Voice chains — JSON because the chain shape evolves
    pub llm_chain_json: Option<String>,
    pub tts_chain_json: Option<String>,
    pub stt_chain_json: Option<String>,
    pub voice_id: Option<String>,
    pub speaking_rate: Option<f32>,
    pub pitch_shift: Option<f32>,
    // Persona
    pub persona_prompt_override: Option<String>,
    // Tools
    pub tool_allowlist_json: Option<String>,
    // Memory / behavior
    pub memory_mode: Option<String>,
    pub context_budget: Option<i64>,
    pub temperature: Option<f32>,
    pub streaming: Option<bool>,
    pub show_thinking: Option<bool>,
    pub handoff_threshold: Option<String>,
    // Limits
    pub daily_cost_cap_cents: Option<i64>,
    pub rounds_cap: Option<i64>,
    pub per_turn_timeout_secs: Option<i64>,
    // Phase 0: cloud-vs-local toggle that re-orders chains
    pub compute_preference: Option<String>,
    // Phase 7
    pub pinned_convs_json: Option<String>,
    pub dashboard_widget: Option<bool>,
    pub updated_at: i64,
}

/// Fetch the stored row for (user, agent). Returns `Ok(None)` when the
/// row doesn't exist — callers should treat that as "all defaults apply".
pub fn get(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
) -> rusqlite::Result<Option<AgentSettings>> {
    conn.query_row(
        "SELECT user_id, agent_id, display_name, icon_blob_id, accent_color,
                wake_phrase, shortcut, llm_chain_json, tts_chain_json,
                stt_chain_json, voice_id, speaking_rate, pitch_shift,
                persona_prompt_override, tool_allowlist_json,
                memory_mode, context_budget, temperature, streaming,
                show_thinking, handoff_threshold,
                daily_cost_cap_cents, rounds_cap, per_turn_timeout_secs,
                compute_preference, pinned_convs_json, dashboard_widget,
                updated_at
         FROM agent_settings WHERE user_id = ? AND agent_id = ?",
        params![user_id, agent_id],
        |r| {
            Ok(AgentSettings {
                user_id: r.get(0)?,
                agent_id: r.get(1)?,
                display_name: r.get(2)?,
                icon_blob_id: r.get(3)?,
                accent_color: r.get(4)?,
                wake_phrase: r.get(5)?,
                shortcut: r.get(6)?,
                llm_chain_json: r.get(7)?,
                tts_chain_json: r.get(8)?,
                stt_chain_json: r.get(9)?,
                voice_id: r.get(10)?,
                speaking_rate: r.get::<_, Option<f64>>(11)?.map(|v| v as f32),
                pitch_shift: r.get::<_, Option<f64>>(12)?.map(|v| v as f32),
                persona_prompt_override: r.get(13)?,
                tool_allowlist_json: r.get(14)?,
                memory_mode: r.get(15)?,
                context_budget: r.get(16)?,
                temperature: r.get::<_, Option<f64>>(17)?.map(|v| v as f32),
                streaming: r.get::<_, Option<i64>>(18)?.map(|v| v != 0),
                show_thinking: r.get::<_, Option<i64>>(19)?.map(|v| v != 0),
                handoff_threshold: r.get(20)?,
                daily_cost_cap_cents: r.get(21)?,
                rounds_cap: r.get(22)?,
                per_turn_timeout_secs: r.get(23)?,
                compute_preference: r.get(24)?,
                pinned_convs_json: r.get(25)?,
                dashboard_widget: r.get::<_, Option<i64>>(26)?.map(|v| v != 0),
                updated_at: r.get(27)?,
            })
        },
    )
    .optional()
}

/// Apply a JSON patch to (user, agent). Only fields present in the patch
/// are updated; absent fields are left as-is. Inserts the row if it
/// doesn't exist yet.
///
/// We use SQLite's UPSERT (INSERT OR REPLACE would clobber columns the
/// caller didn't send) by reading the existing row, merging, then writing.
/// This keeps the API surface a single PUT that supports `{ "temperature": 0.5 }`
/// without overwriting llm_chain_json or anything else.
pub fn patch(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
    patch: &serde_json::Value,
) -> rusqlite::Result<AgentSettings> {
    let mut row = get(conn, user_id, agent_id)?.unwrap_or(AgentSettings {
        user_id,
        agent_id: agent_id.to_string(),
        ..Default::default()
    });

    // Merge each known field. Absence in the patch leaves the column alone;
    // an explicit JSON `null` sets the column to NULL (i.e. "revert to
    // default"). String values are trim-and-empty-to-null.
    let m = match patch.as_object() {
        Some(m) => m,
        None => return Ok(row),
    };
    if let Some(v) = m.get("display_name") {
        row.display_name = json_str(v);
    }
    if let Some(v) = m.get("icon_blob_id") {
        row.icon_blob_id = json_i64(v);
    }
    if let Some(v) = m.get("accent_color") {
        row.accent_color = json_str(v);
    }
    if let Some(v) = m.get("wake_phrase") {
        row.wake_phrase = json_str(v);
    }
    if let Some(v) = m.get("shortcut") {
        row.shortcut = json_str(v);
    }
    if let Some(v) = m.get("llm_chain_json") {
        row.llm_chain_json = json_str_or_object(v);
    }
    if let Some(v) = m.get("tts_chain_json") {
        row.tts_chain_json = json_str_or_object(v);
    }
    if let Some(v) = m.get("stt_chain_json") {
        row.stt_chain_json = json_str_or_object(v);
    }
    if let Some(v) = m.get("voice_id") {
        row.voice_id = json_str(v);
    }
    if let Some(v) = m.get("speaking_rate") {
        row.speaking_rate = json_f32(v);
    }
    if let Some(v) = m.get("pitch_shift") {
        row.pitch_shift = json_f32(v);
    }
    if let Some(v) = m.get("persona_prompt_override") {
        row.persona_prompt_override = json_str(v);
    }
    if let Some(v) = m.get("tool_allowlist_json") {
        row.tool_allowlist_json = json_str_or_object(v);
    }
    if let Some(v) = m.get("memory_mode") {
        row.memory_mode = json_str(v);
    }
    if let Some(v) = m.get("context_budget") {
        row.context_budget = json_i64(v);
    }
    if let Some(v) = m.get("temperature") {
        row.temperature = json_f32(v);
    }
    if let Some(v) = m.get("streaming") {
        row.streaming = json_bool(v);
    }
    if let Some(v) = m.get("show_thinking") {
        row.show_thinking = json_bool(v);
    }
    if let Some(v) = m.get("handoff_threshold") {
        row.handoff_threshold = json_str(v);
    }
    if let Some(v) = m.get("daily_cost_cap_cents") {
        row.daily_cost_cap_cents = json_i64(v);
    }
    if let Some(v) = m.get("rounds_cap") {
        row.rounds_cap = json_i64(v);
    }
    if let Some(v) = m.get("per_turn_timeout_secs") {
        row.per_turn_timeout_secs = json_i64(v);
    }
    if let Some(v) = m.get("compute_preference") {
        row.compute_preference = json_str(v);
    }
    if let Some(v) = m.get("pinned_convs_json") {
        row.pinned_convs_json = json_str_or_object(v);
    }
    if let Some(v) = m.get("dashboard_widget") {
        row.dashboard_widget = json_bool(v);
    }

    row.updated_at = now_secs();

    conn.execute(
        "INSERT OR REPLACE INTO agent_settings
            (user_id, agent_id, display_name, icon_blob_id, accent_color,
             wake_phrase, shortcut, llm_chain_json, tts_chain_json, stt_chain_json,
             voice_id, speaking_rate, pitch_shift, persona_prompt_override,
             tool_allowlist_json, memory_mode, context_budget, temperature,
             streaming, show_thinking, handoff_threshold,
             daily_cost_cap_cents, rounds_cap, per_turn_timeout_secs,
             compute_preference, pinned_convs_json, dashboard_widget, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            row.user_id,
            row.agent_id,
            row.display_name,
            row.icon_blob_id,
            row.accent_color,
            row.wake_phrase,
            row.shortcut,
            row.llm_chain_json,
            row.tts_chain_json,
            row.stt_chain_json,
            row.voice_id,
            row.speaking_rate.map(|v| v as f64),
            row.pitch_shift.map(|v| v as f64),
            row.persona_prompt_override,
            row.tool_allowlist_json,
            row.memory_mode,
            row.context_budget,
            row.temperature.map(|v| v as f64),
            row.streaming.map(|b| if b { 1 } else { 0 }),
            row.show_thinking.map(|b| if b { 1 } else { 0 }),
            row.handoff_threshold,
            row.daily_cost_cap_cents,
            row.rounds_cap,
            row.per_turn_timeout_secs,
            row.compute_preference,
            row.pinned_convs_json,
            row.dashboard_widget.map(|b| if b { 1 } else { 0 }),
            row.updated_at,
        ],
    )?;

    Ok(row)
}

/// Wipe all overrides for (user, agent). Subsequent reads return None
/// and the resolver falls back to the persona defaults.
pub fn delete(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
) -> rusqlite::Result<usize> {
    let _ = conn.execute(
        "DELETE FROM agent_icons WHERE user_id = ? AND agent_id = ?",
        params![user_id, agent_id],
    );
    conn.execute(
        "DELETE FROM agent_settings WHERE user_id = ? AND agent_id = ?",
        params![user_id, agent_id],
    )
}

// ── Per-agent icon storage ────────────────────────────────────────────

pub fn put_icon(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
    content_type: &str,
    bytes: &[u8],
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO agent_icons (user_id, agent_id, content_type, bytes, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(user_id, agent_id) DO UPDATE SET
             content_type = excluded.content_type,
             bytes = excluded.bytes,
             updated_at = excluded.updated_at",
        params![user_id, agent_id, content_type, bytes, now_secs()],
    )?;
    // Stash the FK into agent_settings so resolver can show "icon present"
    // without a second query. We use 1 as a sentinel — the actual fetch
    // hits agent_icons by (user, agent).
    let _ = conn.execute(
        "INSERT INTO agent_settings (user_id, agent_id, icon_blob_id, updated_at)
         VALUES (?, ?, 1, ?)
         ON CONFLICT(user_id, agent_id) DO UPDATE SET
             icon_blob_id = 1, updated_at = excluded.updated_at",
        params![user_id, agent_id, now_secs()],
    );
    Ok(())
}

pub fn get_icon(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
) -> rusqlite::Result<Option<(String, Vec<u8>)>> {
    conn.query_row(
        "SELECT content_type, bytes FROM agent_icons
         WHERE user_id = ? AND agent_id = ?",
        params![user_id, agent_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
    )
    .optional()
}

pub fn delete_icon(
    conn: &Connection,
    user_id: i64,
    agent_id: &str,
) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "DELETE FROM agent_icons WHERE user_id = ? AND agent_id = ?",
        params![user_id, agent_id],
    )?;
    let _ = conn.execute(
        "UPDATE agent_settings SET icon_blob_id = NULL, updated_at = ?
         WHERE user_id = ? AND agent_id = ?",
        params![now_secs(), user_id, agent_id],
    );
    Ok(n)
}

// ── JSON value coercion helpers ────────────────────────────────────────

fn json_str(v: &serde_json::Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn json_str_or_object(v: &serde_json::Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        return Some(s.to_string());
    }
    // Accept array/object payloads inline — caller doesn't have to JSON-encode
    // a nested structure twice.
    Some(v.to_string())
}

fn json_i64(v: &serde_json::Value) -> Option<i64> {
    if v.is_null() {
        return None;
    }
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn json_f32(v: &serde_json::Value) -> Option<f32> {
    if v.is_null() {
        return None;
    }
    v.as_f64()
        .map(|f| f as f32)
        .or_else(|| v.as_i64().map(|i| i as f32))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn json_bool(v: &serde_json::Value) -> Option<bool> {
    if v.is_null() {
        return None;
    }
    v.as_bool()
        .or_else(|| v.as_i64().map(|i| i != 0))
        .or_else(|| {
            v.as_str().and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            })
        })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
