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
            &ctx(&[("first", "Uncle"), ("last", "Iroh")]),
        );
        assert_eq!(out, "Hi Uncle Iroh");
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
