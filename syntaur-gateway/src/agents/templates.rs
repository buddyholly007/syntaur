//! Template substitution engine for persona system prompts.
//!
//! Replaces `{{var}}` and `{{var|default:"fallback"}}` patterns in a
//! template string with values from a per-call context map. Used to fill
//! in the `system_prompt_template` rows stored in `module_agent_defaults`
//! before sending them to the LLM.
//!
//! Syntax:
//!   `{{var}}`                        — substitute var or leave empty
//!   `{{var|default:"fallback"}}`     — substitute var or "fallback"
//!   `{{var|default:bare}}`           — substitute var or `bare` (unquoted)
//!
//! Unknown variables resolve to their default, or empty string if no
//! default is given. Empty-string values in the context are treated as
//! "use the default" so authors can seed optional vars safely.

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // {{var}} or {{var|default:"..."}} or {{var|default:bare}}
        Regex::new(
            r#"\{\{\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*(?:\|\s*default\s*:\s*(?:"([^"]*)"|([^\s}]+)))?\s*\}\}"#,
        )
        .expect("template regex should compile")
    })
}

/// Substitute `{{var}}` patterns in `template` with values from `ctx`.
/// Unknown vars or empty values fall through to the declared default.
pub fn substitute(template: &str, ctx: &HashMap<&str, String>) -> String {
    template_re()
        .replace_all(template, |caps: &regex::Captures| {
            let var = &caps[1];
            let default_quoted = caps.get(2).map(|m| m.as_str().to_string());
            let default_bare = caps.get(3).map(|m| m.as_str().to_string());
            match ctx.get(var) {
                Some(v) if !v.is_empty() => v.clone(),
                _ => default_quoted.or(default_bare).unwrap_or_default(),
            }
        })
        .to_string()
}

/// Fetch a default persona template + display name from the registry.
pub fn load_default(
    conn: &rusqlite::Connection,
    agent_key: &str,
) -> rusqlite::Result<Option<(String, String, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT system_prompt_template, default_display_name, default_humor_value \
         FROM module_agent_defaults WHERE agent_key = ?1",
    )?;
    let mut rows = stmt.query(rusqlite::params![agent_key])?;
    if let Some(row) = rows.next()? {
        Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
    } else {
        Ok(None)
    }
}

/// Build a base context for persona substitution from the user's stable
/// profile (first name, personality doc, current year, etc.). Callers
/// add module-specific vars on top before calling `substitute()`.
pub fn base_context(
    user_first_name: Option<&str>,
    personality_doc: Option<&str>,
    agent_display_name: &str,
    main_agent_name: &str,
    humor_level: i64,
) -> HashMap<&'static str, String> {
    let mut ctx = HashMap::new();
    ctx.insert(
        "user_first_name",
        user_first_name.unwrap_or("the user").to_string(),
    );
    if let Some(p) = personality_doc {
        if !p.is_empty() {
            ctx.insert("personality_doc", p.to_string());
        }
    }
    ctx.insert("agent_name", agent_display_name.to_string());
    ctx.insert("main_agent_name", main_agent_name.to_string());
    ctx.insert("humor_level", humor_level.to_string());
    ctx.insert(
        "current_tax_year",
        chrono::Utc::now().format("%Y").to_string(),
    );
    ctx
}

// ── Module-specific context ──────────────────────────────────────────────────

/// Populate module-specific template variables by querying the gateway DB.
/// Called inside `spawn_blocking` — all operations are synchronous.
///
/// Returns additional vars to merge into the base context before substitution.
/// Keys that produce empty results are omitted (template defaults apply).
pub fn module_context(
    conn: &rusqlite::Connection,
    agent_key: &str,
    user_id: i64,
) -> HashMap<&'static str, String> {
    let mut ctx = HashMap::new();
    match agent_key {
        "module_tax" => populate_tax(&mut ctx, conn, user_id),
        "module_research" => populate_research(&mut ctx, conn),
        "module_music" => populate_music(&mut ctx, conn, user_id),
        "module_scheduler" => populate_scheduler(&mut ctx, conn, user_id),
        "module_coders" => populate_coders(&mut ctx, conn),
        "module_social" => populate_social(&mut ctx, conn, user_id),
        _ => {}
    }
    ctx
}

/// Seed Nyota's prompt with everything the user has configured in
/// /social → Settings (and per-platform panels). Each pref is read
/// independently; if a key is unset or empty, the template's `default:`
/// clause kicks in.
///
/// Privacy-scope prefs (calendar/music/research) gate cross-module
/// context reads. Journal is never touched, regardless of pref state.
fn populate_social(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection, user_id: i64) {
    let pref = |key: &str| -> Option<String> {
        conn.query_row(
            "SELECT value FROM user_preferences WHERE user_id = ? AND key = ?",
            rusqlite::params![user_id, key],
            |r| r.get::<_, Option<String>>(0),
        ).ok().flatten()
            .filter(|s| !s.trim().is_empty())
    };

    if let Some(b) = pref("social.brand_voice") { ctx.insert("brand_voice", b); }
    if let Some(a) = pref("social.audience")    { ctx.insert("audience", a); }

    // Blocklist — stored as comma-separated string. Rendered as a short
    // "avoid these" line so Nyota knows what not to say.
    if let Some(bl) = pref("social.blocklist.words") {
        let joined = bl.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        if !joined.is_empty() {
            ctx.insert("avoid_terms", format!("Avoid in drafts: {}.", joined));
        }
    }

    // Tone dials — humor + formality sliders (0-10). Surfaced as a short
    // calibration line so the prompt stays in Nyota's voice but leans
    // toward the user's sliders.
    let humor     = pref("social.tone.humor").and_then(|v| v.parse::<u8>().ok()).unwrap_or(4);
    let formality = pref("social.tone.formality").and_then(|v| v.parse::<u8>().ok()).unwrap_or(4);
    ctx.insert("tone_dials", format!("Humor {}/10, formality {}/10.", humor, formality));

    // Connected platforms — short list Nyota uses as quick context.
    if let Ok(mut stmt) = conn.prepare(
        "SELECT platform, display_name FROM social_connections \
         WHERE user_id = ? AND status IN ('connected','degraded') ORDER BY platform"
    ) {
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map([user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
            .ok()
            .map(|iter| iter.filter_map(Result::ok).collect())
            .unwrap_or_default();
        if !rows.is_empty() {
            let list = rows.iter()
                .map(|(p, h)| match h.as_deref() {
                    Some(hn) if !hn.is_empty() => format!("{} ({})", p, hn),
                    _ => p.clone(),
                })
                .collect::<Vec<_>>().join(", ");
            ctx.insert("connected_platforms", list);
        }
    }

    // Privacy-gated cross-module context. User must explicitly opt in
    // for each scope. Journal is hardcoded-off and never read, even if
    // someone sets the pref externally.
    let allow_calendar = pref("social.privacy.calendar").map(|v| v == "true").unwrap_or(false);
    let allow_music    = pref("social.privacy.music").map(|v| v == "true").unwrap_or(false);

    let mut ctx_bits: Vec<String> = Vec::new();
    if allow_calendar {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let horizon = (chrono::Utc::now() + chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT title, start_time FROM calendar_events \
             WHERE (user_id = ?1 OR user_id = 0) AND start_time >= ?2 AND start_time < ?3 \
             ORDER BY start_time LIMIT 5"
        ) {
            let lines: Vec<String> = stmt.query_map(rusqlite::params![user_id, &today, &horizon], |r| {
                Ok(format!("{} ({})", r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }).ok().map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default();
            if !lines.is_empty() {
                ctx_bits.push(format!("Upcoming (7d): {}", lines.join("; ")));
            }
        }
    }
    if allow_music {
        // Recent local music additions — helpful when drafting around a release.
        if let Ok(mut stmt) = conn.prepare(
            "SELECT title, artist FROM local_music_tracks WHERE user_id = ? \
             ORDER BY indexed_at DESC LIMIT 3"
        ) {
            let lines: Vec<String> = stmt.query_map([user_id], |r| {
                Ok(format!("{} — {}",
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(1)?.unwrap_or_default()))
            }).ok().map(|iter| iter.filter_map(Result::ok).collect()).unwrap_or_default();
            if !lines.is_empty() {
                ctx_bits.push(format!("Recent tracks: {}", lines.join("; ")));
            }
        }
    }
    if !ctx_bits.is_empty() {
        ctx.insert("social_context_summary", ctx_bits.join(" | "));
    }
}

fn populate_tax(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection, user_id: i64) {
    let year: i64 = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025);
    let row = conn.query_row(
        "SELECT first_name, last_name, filing_status, state, city \
         FROM taxpayer_profiles WHERE user_id = ?1 AND tax_year = ?2",
        rusqlite::params![user_id, year],
        |r| Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
        )),
    );
    if let Ok((first, last, filing, state, city)) = row {
        let mut parts = Vec::new();
        if let (Some(f), Some(l)) = (&first, &last) {
            parts.push(format!("{} {}", f, l));
        }
        if let Some(f) = &filing { parts.push(format!("Filing: {}", f)); }
        if let (Some(c), Some(s)) = (&city, &state) {
            parts.push(format!("{}, {}", c, s));
        } else if let Some(s) = &state {
            parts.push(s.clone());
        }
        if !parts.is_empty() {
            ctx.insert("tax_profile_summary", parts.join(" | "));
        }
    }
    let receipt_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM receipts WHERE user_id = ?1", rusqlite::params![user_id], |r| r.get(0))
        .unwrap_or(0);
    if receipt_count > 0 {
        let summary = ctx.entry("tax_profile_summary").or_insert_with(String::new);
        if !summary.is_empty() { summary.push_str(" | "); }
        summary.push_str(&format!("{} receipts on file", receipt_count));
    }
}

fn populate_research(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection) {
    let doc_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap_or(0);
    let source_count: i64 = conn
        .query_row("SELECT COUNT(DISTINCT source) FROM documents", [], |r| r.get(0))
        .unwrap_or(0);
    if doc_count > 0 {
        ctx.insert("kb_summary", format!("{} documents from {} sources", doc_count, source_count));
    }
    let session_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM research_sessions WHERE status = 'complete'", [], |r| r.get(0))
        .unwrap_or(0);
    if session_count > 0 {
        ctx.insert("research_sessions_summary", format!("{} completed research sessions", session_count));
    }
}

fn populate_music(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection, user_id: i64) {
    let mut providers = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT provider, display_name FROM sync_connections \
         WHERE (user_id = ?1 OR user_id = 0) AND status = 'active'"
    ) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![user_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        }) {
            for row in rows.flatten() {
                let p = row.0.to_lowercase();
                if ["spotify", "apple_music", "tidal", "youtube_music", "plex", "music_assistant"]
                    .iter().any(|k| p.contains(k))
                {
                    providers.push(row.1.unwrap_or(row.0));
                }
            }
        }
    }
    if !providers.is_empty() {
        ctx.insert("music_providers", providers.join(", "));
    }
}

fn populate_scheduler(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection, user_id: i64) {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let day_after = (chrono::Utc::now() + chrono::Duration::days(2)).format("%Y-%m-%d").to_string();
    let mut events = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT title, start_time FROM calendar_events \
         WHERE (user_id = ?1 OR user_id = 0) AND start_time >= ?2 AND start_time < ?3 \
         ORDER BY start_time LIMIT 10"
    ) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![user_id, &today, &day_after], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for row in rows.flatten() {
                events.push(format!("{} ({})", row.0, row.1));
            }
        }
    }
    if events.is_empty() {
        ctx.insert("calendar_snapshot", "No events scheduled in the next 48 hours.".to_string());
    } else {
        ctx.insert("calendar_snapshot", format!("Next 48h: {}", events.join("; ")));
    }
}

fn populate_coders(ctx: &mut HashMap<&'static str, String>, conn: &rusqlite::Connection) {
    // Terminal hosts are in a separate DB (terminal module), so we can only
    // report what's in the main index. Leave the placeholder for now —
    // the template default applies.
    let _ = conn;
    let _ = ctx;
}


/// Build a compact memory index for injection into the system prompt.
/// Returns the top N most relevant memories as a formatted text block.
///
/// Relevance = importance * recency_weight. Stale memories (>90 days)
/// are marked. The output is ~500 tokens max.
/// Calculate how many memories to inject based on the model's context window.
/// Ensures the system prompt (persona + memories + personality + module context)
/// doesn't crowd out conversation history.
///
/// Budget allocation:
///   - Reserve 20% for response generation (max_tokens)
///   - Persona template: fixed (what it is)
///   - Memories: 5-15% of remaining, adaptive
///   - Personality doc: fixed (what it is)
///   - Conversation history: everything left
pub fn context_budget_memories(context_window_tokens: u64) -> usize {
    match context_window_tokens {
        0..=4096 => 2,          // 4K: bare minimum — 2 most important memories
        4097..=8192 => 4,       // 8K: light — 4 memories
        8193..=16384 => 6,      // 16K: moderate
        16385..=32768 => 8,     // 32K: comfortable
        32769..=65536 => 10,    // 64K: standard (current default)
        65537..=131072 => 15,   // 128K: generous
        _ => 20,                // 256K+: load lots
    }
}

/// Estimate token count for a string (rough: ~4 chars per token for English).
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

pub fn build_memory_injection(
    conn: &rusqlite::Connection,
    user_id: i64,
    agent_id: &str,
    max_memories: usize,
) -> String {
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    let scope_clause = if agent_id == "main" {
        "m.user_id = ? AND m.agent_id != 'journal'".to_string()
    } else if agent_id == "journal" {
        params.push(Box::new(agent_id.to_string()));
        "m.user_id = ? AND m.agent_id = ?".to_string()
    } else {
        params.push(Box::new(agent_id.to_string()));
        "(m.user_id = ? AND (m.agent_id = ? OR m.shared = 1 OR (m.agent_id = 'main' AND m.memory_type IN ('user','feedback'))))".to_string()
    };

    let sql = format!(
        "SELECT m.memory_type, m.key, m.title, m.description, m.content, \
                m.importance, m.updated_at, m.confidence, m.agent_id \
         FROM agent_memories m \
         WHERE {} \
         ORDER BY m.importance DESC, m.updated_at DESC \
         LIMIT {}",
        scope_clause, max_memories
    );

    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let now = chrono::Utc::now().timestamp();
    let lines: Vec<String> = stmt
        .query_map(refs.as_slice(), |r| {
            let mtype: String = r.get(0)?;
            let key: String = r.get(1)?;
            let _title: String = r.get(2)?;
            let desc: Option<String> = r.get(3)?;
            let content: String = r.get(4)?;
            let _importance: i64 = r.get(5)?;
            let updated: i64 = r.get(6)?;
            let confidence: f64 = r.get(7)?;
            let agent: String = r.get(8)?;

            let age_days = (now - updated) / 86400;
            let stale = if age_days > 90 { " [stale]" } else { "" };
            let conf = if confidence < 0.8 { " [uncertain]" } else { "" };
            let summary = desc.unwrap_or_else(|| {
                if content.len() > 80 { format!("{}...", &content[..77]) } else { content }
            });

            Ok(format!("[{}] {}: {}{}{}", mtype, key, summary, stale, conf))
        })
        .ok()
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default();

    if lines.is_empty() {
        return String::new();
    }

    // Update access counts for injected memories
    let _ = conn.execute_batch(&format!(
        "UPDATE agent_memories SET access_count = access_count + 1, last_accessed_at = {} \
         WHERE user_id = {} AND id IN (SELECT id FROM agent_memories WHERE {} ORDER BY importance DESC, updated_at DESC LIMIT {})",
        now, user_id, scope_clause.replace("m.", ""), max_memories
    ));

    format!(
        "[Your memories — {} loaded. Use memory_recall(query) for more, memory_save(...) to remember new things.]
{}",
        lines.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        let mut m = HashMap::new();
        for (k, v) in pairs {
            m.insert(*k, v.to_string());
        }
        m
    }

    #[test]
    fn plain_variable() {
        let out = substitute("Hello {{name}}", &ctx(&[("name", "Sean")]));
        assert_eq!(out, "Hello Sean");
    }

    #[test]
    fn quoted_default_used_when_missing() {
        let out = substitute(
            r#"Hi {{name|default:"friend"}}"#,
            &ctx(&[]),
        );
        assert_eq!(out, "Hi friend");
    }

    #[test]
    fn quoted_default_ignored_when_present() {
        let out = substitute(
            r#"Hi {{name|default:"friend"}}"#,
            &ctx(&[("name", "Sean")]),
        );
        assert_eq!(out, "Hi Sean");
    }

    #[test]
    fn bare_default() {
        let out = substitute("humor={{humor_level|default:3}}", &ctx(&[]));
        assert_eq!(out, "humor=3");
    }

    #[test]
    fn empty_value_falls_through_to_default() {
        let out = substitute(
            r#"{{name|default:"anon"}}"#,
            &ctx(&[("name", "")]),
        );
        assert_eq!(out, "anon");
    }

    #[test]
    fn no_match_leaves_empty_when_no_default() {
        let out = substitute("start {{missing}} end", &ctx(&[]));
        assert_eq!(out, "start  end");
    }

    #[test]
    fn whitespace_tolerance() {
        let out = substitute(
            r#"{{   name   |   default : "x"   }}"#,
            &ctx(&[]),
        );
        assert_eq!(out, "x");
    }

    #[test]
    fn multiple_vars() {
        let out = substitute(
            "Hi {{first}} {{last}}",
            &ctx(&[("first", "Jane"), ("last", "Doe")]),
        );
        assert_eq!(out, "Hi Jane Doe");
    }

    #[test]
    fn base_context_populates_commons() {
        let c = base_context(Some("Sean"), Some("bio goes here"), "Peter", "Peter", 4);
        assert_eq!(c.get("user_first_name").map(String::as_str), Some("Sean"));
        assert_eq!(c.get("personality_doc").map(String::as_str), Some("bio goes here"));
        assert_eq!(c.get("agent_name").map(String::as_str), Some("Peter"));
        assert_eq!(c.get("humor_level").map(String::as_str), Some("4"));
        assert!(c.contains_key("current_tax_year"));
    }
}
