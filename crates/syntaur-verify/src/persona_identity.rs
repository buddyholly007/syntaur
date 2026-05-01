//! Stage 8 (Verify Plan v2 Layer A): Server-vs-DOM persona identity.
//!
//! For every persona slug in the catalog, GET /api/agents/{slug}/settings
//! and assert the row exists + display_name matches. Catches:
//!   - missing persona settings row (catalog lists slug but nothing
//!     seeded in prod)
//!   - display_name divergence (catalog stale relative to prod renames)
//!
//! What this stage does NOT catch (yet): the 2026-04-30 Peter BLOB
//! silent fall-through (commit 23f7370). That bug shape was: catalog
//! `system_prompt` BLOB column unreadable by rusqlite, get_user_agent
//! falls through to default, chat returns from wrong persona. The
//! `/api/agents/{slug}/settings` endpoint deliberately does NOT
//! expose `system_prompt` (it's sensitive), so we can't detect the
//! BLOB-vs-TEXT regression server-side. Detecting it requires a chat
//! round-trip under each persona's own auth token: ask "what is your
//! name?" and assert response contains the catalog display_name.
//! That probe lands as Stage 8b once the verify-bot service-account
//! infrastructure provides per-persona tokens (today: blocked).
//!
//! Severity:
//!   - Missing row + missing display_name + divergent display_name →
//!     Suggestion (pre-existing state issues, surfaced informationally).
//!   - Endpoint 5xx, body not JSON, body wrong shape → Regression
//!     (those are server bugs, not state issues).
//!
//! Auth: requires a verify auth token. The endpoint is 401 unauth.

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
    // Reflects 2026-04-30 Sean's prod state (from /api/agents/{slug}/export).
    // Update when Sean renames a persona via the cog drawer.
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
        // Could be: persona never seeded, or persona row deleted under
        // the catalog. Surfaces as Suggestion (informational) — not
        // every catalog entry needs to be seeded in every install.
        // The BLOB silent fall-through Sean hit on 23f7370 looked
        // similar from outside (chat returned wrong-persona content)
        // but is NOT detectable here because the relevant column
        // (system_prompt) isn't on this endpoint. See module docstring.
        if body.is_null() {
            findings.push(make_suggestion(
                slug,
                "settings row not seeded",
                format!(
                    "GET {url} → 200 with body `null` — slug `{slug}` has no \
                     settings row in this gateway. Catalog lists it, prod doesn't \
                     have it. Either seed the row or remove the slug from \
                     EXPECTED_PERSONAS in persona_identity.rs."
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
            Some(Value::String(_)) => findings.push(make_suggestion(
                slug,
                "display_name empty",
                format!("GET {url} → display_name is empty string"),
            )),
            _ => findings.push(make_finding(
                slug,
                "display_name missing or wrong type",
                format!("GET {url} → display_name missing or not a string"),
            )),
        }
    }
    Ok(findings)
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
