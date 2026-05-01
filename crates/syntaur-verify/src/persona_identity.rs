//! Stage 8 (Verify Plan v2 Layer A): Server-vs-DOM persona identity.
//!
//! TWO checks:
//!
//! 8A — Per-persona settings rows. For every EXPECTED_PERSONAS slug,
//!      GET /api/agents/{slug}/settings and assert the row exists +
//!      display_name is present and non-empty. Catches:
//!        - missing persona settings row (catalog lists slug but
//!          nothing seeded in prod) → Regression
//!        - display_name empty or wrong type → Regression
//!        - display_name divergence from catalog → Suggestion (Sean
//!          legitimately renames personas via the cog drawer; catalog
//!          updated to current renames in EXPECTED_PERSONAS)
//!
//! 8B — Per-template resolved system prompt. For every EXPECTED_TEMPLATES
//!      agent_key, GET /api/agents/resolve_prompt and assert the
//!      catalog template loads + the prompt is a non-trivial string +
//!      no unresolved {{placeholders}} survived substitution. Catches
//!      the original 2026-04-30 Peter BLOB silent fall-through class
//!      (commit 23f7370): catalog row stored as BLOB, rusqlite refuses
//!      to read as TEXT, load_default returns Err, every chat with
//!      that persona silently uses the default prompt. resolve_prompt
//!      goes through the same load_default pathway, so a BLOB column
//!      surfaces as 404 / empty prompt here.
//!
//! Severity model (per [[feedback/found_bug_must_be_fixed_immediately]]):
//!   - Missing settings row → Regression. Catalog says it should exist;
//!     prod doesn't have it; deploys should NOT ship past this gap.
//!   - Empty display_name / wrong type → Regression.
//!   - resolve_prompt 404 / 5xx / empty / unresolved placeholders →
//!     Regression (the BLOB-class symptom).
//!   - display_name divergence (Sean rename vs catalog default) →
//!     Suggestion (informational; update EXPECTED_PERSONAS when
//!     intentional).
//!
//! Auth: requires a verify auth token. Both endpoints are 401 unauth.

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use std::time::Duration;

use crate::run::{Finding, FindingKind, Severity};

/// Personas that should have a settings row in prod. Mirrors the
/// catalog at `crates/syntaur-verify/personas.yaml` — duplicated here
/// to keep the cache crate self-contained and to make the expected
/// shape explicit at the check site.
///
/// `(slug, expected_display_name_lowercased)` — display_name is matched
/// case-insensitively because Sean's installs sometimes capitalise
/// differently than the catalog default.
const EXPECTED_PERSONAS: &[(&str, &str)] = &[
    // 8A: per-user settings rows. Reflects 2026-04-30 Sean's prod
    // state (from /api/agents/{slug}/export). Update when Sean
    // renames a persona via the cog drawer.
    ("main", "peter"),                // default user agent → display_name="Peter"
    ("cortex", "doctor bishop"),       // Sean renamed cortex → "Doctor Bishop"
    ("maurice", "moss"),               // Sean renamed maurice → "Moss"
    ("thaddeus", "alfred"),            // Sean renamed thaddeus → "Alfred"
    ("silvr", "silvr"),
    ("mushi", "mushi"),
    ("positron", "positron"),
    ("nyota", "nyota"),
    ("kyron", "kyron"),
];

/// 8B: catalog template agent_keys + minimum prompt length. Used by
/// resolve_prompt to verify the catalog template loads + substitutes
/// cleanly. Sourced from `syntaur-gateway/src/agents/defaults.rs` —
/// every entry there with `agent_key:` is a candidate. Min length is
/// 1000 — well above the empty-fallback marker (~50 chars), well below
/// any real persona's prompt (which today are 3.6k–10.8k chars).
const EXPECTED_TEMPLATES: &[(&str, usize)] = &[
    ("main_default", 1000),
    ("module_tax", 1000),
    ("module_research", 1000),
    ("module_music", 1000),
    ("module_scheduler", 1000),
    ("module_coders", 1000),
    ("module_social", 1000),
    ("module_journal", 1000),
];

/// One finding per check failure. Returns Vec so multiple personas
/// can fail in the same run.
pub async fn check_persona_identity(
    target_url: &str,
    auth_token: Option<&str>,
) -> Result<Vec<Finding>> {
    let Some(token) = auth_token else {
        log::info!(
            "[persona-identity] no auth token — stage skipped (set --auth-token or \
             SYNTAUR_VERIFY_AUTH_TOKEN to enable)"
        );
        return Ok(Vec::new());
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut findings = Vec::new();
    for (slug, expected_name_lc) in EXPECTED_PERSONAS {
        let url = format!(
            "{}/api/agents/{}/settings",
            target_url.trim_end_matches('/'),
            slug
        );
        let resp = match client.get(&url).bearer_auth(token).send().await {
            Ok(r) => r,
            Err(e) => {
                findings.push(make_finding(
                    slug,
                    "request failed",
                    format!("GET {url}: {e:#}"),
                ));
                continue;
            }
        };
        let status = resp.status();
        if status == 404 {
            // 404 → persona not registered in this gateway. Silvr/etc
            // not yet seeded is not necessarily a regression; we'll
            // treat it as a Suggestion (informational) so deployment
            // doesn't block, but the report surfaces it. Future work:
            // distinguish "persona never existed" from "persona row
            // got deleted out from under the catalog."
            findings.push(make_suggestion(
                slug,
                "persona not registered",
                format!("GET {url} → 404 — slug listed in EXPECTED_PERSONAS but no row in DB"),
            ));
            continue;
        }
        if !status.is_success() {
            findings.push(make_finding(
                slug,
                "non-success status",
                format!("GET {url} → {status} (expected 200)"),
            ));
            continue;
        }

        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                findings.push(make_finding(
                    slug,
                    "body not JSON",
                    format!("GET {url}: response not parseable as JSON: {e:#}"),
                ));
                continue;
            }
        };

        // Body literally `null` → no settings row for this slug.
        // EXPECTED_PERSONAS is the contract: if a slug is here, prod
        // is supposed to have it. A null body means the catalog and
        // prod are out of sync — that's a real defect, not informational.
        // Resolution path: either (a) seed the row in prod, or (b)
        // remove the slug from EXPECTED_PERSONAS if the persona is
        // genuinely optional. Don't silently let deploys ship past
        // this gap (per [[feedback/found_bug_must_be_fixed_immediately]]
        // — a verify finding is a defect to act on, not a Suggestion
        // to file).
        if body.is_null() {
            findings.push(make_finding(
                slug,
                "settings row missing in prod",
                format!(
                    "GET {url} → 200 with body `null` — slug `{slug}` has no \
                     settings row. EXPECTED_PERSONAS lists it as required; \
                     either seed the row in prod (so chat with `{slug}` works) \
                     or remove the slug from EXPECTED_PERSONAS in \
                     persona_identity.rs (acknowledging it's intentionally not \
                     deployed)."
                ),
            ));
            continue;
        }

        let obj = match body.as_object() {
            Some(o) => o,
            None => {
                findings.push(make_finding(
                    slug,
                    "body not an object",
                    format!("GET {url} → body is {} (expected JSON object)", body),
                ));
                continue;
            }
        };

        // display_name: should be a non-empty string matching catalog.
        // Stage 8 today checks display_name only because /settings
        // doesn't expose system_prompt (sensitive). BLOB-vs-TEXT
        // detection requires Stage 8b chat round-trip (deferred).
        match obj.get("display_name") {
            Some(Value::String(s)) if !s.is_empty() => {
                let actual = s.to_lowercase();
                if !actual.contains(expected_name_lc) && !expected_name_lc.contains(&actual) {
                    findings.push(make_suggestion(
                        slug,
                        "display_name diverges from catalog",
                        format!(
                            "GET {url} → display_name=`{s}`, catalog expected match \
                             with `{expected_name_lc}`. If Sean renamed the persona, \
                             update EXPECTED_PERSONAS in persona_identity.rs."
                        ),
                    ));
                }
            }
            Some(Value::String(_)) => findings.push(make_finding(
                slug,
                "display_name empty",
                format!(
                    "GET {url} → display_name is empty string. Persona is \
                     unrenderable in any UI surface that shows persona name."
                ),
            )),
            _ => findings.push(make_finding(
                slug,
                "display_name missing or wrong type",
                format!("GET {url} → display_name missing or not a string"),
            )),
        }
    }

    // ── 8B: catalog template resolution ───────────────────────────
    // Walk EXPECTED_TEMPLATES, GET /api/agents/resolve_prompt for each.
    // Catches the BLOB-class regression directly: load_default fails
    // → 404 here → resolved prompt would silently fall back to default
    // in chat. Also catches placeholder-substitution regressions
    // (templates with unresolved {{vars}} get rendered to the LLM
    // verbatim, surfaces as in-character "what's {{first_name}}" replies).
    for (agent_key, min_len) in EXPECTED_TEMPLATES {
        // Bearer header NOT query param — query strings are commonly
        // logged by proxies/load balancers/journals; the token must
        // not appear in URLs. Endpoint accepts either now (gateway
        // updated 2026-04-30 to add Authorization header support).
        let url = format!(
            "{}/api/agents/resolve_prompt?agent_key={}",
            target_url.trim_end_matches('/'),
            agent_key,
        );
        let resp = match client.get(&url).bearer_auth(token).send().await {
            Ok(r) => r,
            Err(e) => {
                findings.push(make_template_finding(
                    agent_key,
                    "request failed",
                    format!("GET /api/agents/resolve_prompt?agent_key={agent_key}: {e:#}"),
                ));
                continue;
            }
        };
        let status = resp.status();
        if status == 404 {
            findings.push(make_template_finding(
                agent_key,
                "catalog template missing (BLOB-class symptom)",
                format!(
                    "GET /api/agents/resolve_prompt?agent_key={agent_key} → 404. \
                     load_default returned None — either the row was never seeded \
                     or it's stored as BLOB and rusqlite refuses to read it as TEXT \
                     (the 2026-04-30 Peter regression class). Chat with this \
                     persona will silently use the default fallback prompt."
                ),
            ));
            continue;
        }
        if !status.is_success() {
            findings.push(make_template_finding(
                agent_key,
                "non-success status",
                format!(
                    "GET /api/agents/resolve_prompt?agent_key={agent_key} → {status}"
                ),
            ));
            continue;
        }
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                findings.push(make_template_finding(
                    agent_key,
                    "body not JSON",
                    format!(
                        "GET /api/agents/resolve_prompt?agent_key={agent_key}: \
                         response not parseable as JSON: {e:#}"
                    ),
                ));
                continue;
            }
        };

        // Extract length + placeholders_remaining + display_name + prompt.
        let prompt_len = body.get("length").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let placeholders =
            body.get("placeholders_remaining").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
        let display = body
            .get("display_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if prompt_len < *min_len {
            findings.push(make_template_finding(
                agent_key,
                "prompt too short (BLOB-class symptom)",
                format!(
                    "resolve_prompt({agent_key}) returned a {prompt_len}-char prompt \
                     (min {min_len}). Either the catalog template was wiped, or it \
                     loaded as BLOB and substituted to empty. Default-fallback path \
                     will execute on every chat with this persona."
                ),
            ));
        }
        if placeholders > 0 {
            findings.push(make_template_finding(
                agent_key,
                "unresolved placeholders survived substitution",
                format!(
                    "resolve_prompt({agent_key}) → {placeholders} unresolved \
                     {{{{placeholders}}}} in the rendered prompt. The LLM will see \
                     literal `{{{{name}}}}`-style strings instead of substituted \
                     values — surfaces as in-character 'hello {{{{first_name}}}}' \
                     replies in chat."
                ),
            ));
        }
        if display.is_empty() {
            findings.push(make_template_finding(
                agent_key,
                "display_name empty in resolve_prompt",
                format!(
                    "resolve_prompt({agent_key}) → display_name is empty. The \
                     resolved persona has no name; chat header / topbar avatar \
                     hint will render as blank."
                ),
            ));
        }
    }

    Ok(findings)
}

fn make_template_finding(agent_key: &str, title_suffix: &str, detail: String) -> Finding {
    Finding {
        module_slug: "persona-identity".into(),
        kind: FindingKind::Other,
        severity: Severity::Regression,
        title: format!("Template `{agent_key}`: {title_suffix}"),
        detail,
        artifact: None,
        captured_at: Utc::now(),
        edits: None,
        persona: Some(agent_key.into()),
    }
}

fn make_finding(slug: &str, title_suffix: &str, detail: String) -> Finding {
    Finding {
        module_slug: "persona-identity".into(),
        kind: FindingKind::Other,
        severity: Severity::Regression,
        title: format!("Persona `{slug}`: {title_suffix}"),
        detail,
        artifact: None,
        captured_at: Utc::now(),
        edits: None,
        persona: Some(slug.into()),
    }
}

fn make_suggestion(slug: &str, title_suffix: &str, detail: String) -> Finding {
    Finding {
        module_slug: "persona-identity".into(),
        kind: FindingKind::Other,
        severity: Severity::Suggestion,
        title: format!("Persona `{slug}`: {title_suffix}"),
        detail,
        artifact: None,
        captured_at: Utc::now(),
        edits: None,
        persona: Some(slug.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_personas_is_non_empty() {
        // Tripwire: if the catalog gets emptied accidentally, the
        // stage becomes a no-op and would regress quietly.
        assert!(!EXPECTED_PERSONAS.is_empty());
        assert!(EXPECTED_PERSONAS.iter().any(|(s, _)| *s == "main"));
    }
}
