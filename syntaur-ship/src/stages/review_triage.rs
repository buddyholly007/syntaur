//! Review-triage gate: when an external code-review report is present
//! in the workspace, every finding it contains MUST have a logged
//! verdict + evidence pair before deploy is allowed to continue.
//!
//! Motivation: on 2026-04-30 a Gemini review of `shared.rs` flagged
//! five issues; one ("modules load path") was waved off as
//! "preexisting/unrelated" without verification. Sean's standing rule
//! `feedback/never_dismiss_specific_code_review_finding_as_noise.md`
//! treats that pattern as a bug class — encoded here as structural
//! enforcement so the dismissal becomes impossible without a logged
//! verdict.
//!
//! Drop a `<workspace>/.syntaur/reviews/<reviewer>-<timestamp>.md`
//! file with one finding per fenced block:
//!
//!     ```finding
//!     id: shared-1
//!     reviewer: gemini
//!     file: src/pages/shared.rs
//!     line: 1307
//!     claim: innerHTML used to inject server-rendered HTML; XSS if any
//!            page emits PreEscaped(user_input).
//!     verdict: TRUE | FALSE | DEPENDENT
//!     evidence: src/pages/shared.rs:1307 quote → "liveMain.innerHTML = newMain.innerHTML;"
//!     resolution: fix-in-place | tracked: <vault/projects/...> | wont-fix: <reason>
//!     ```
//!
//! Verdict + evidence + resolution are all REQUIRED. Any finding
//! missing any of those three fields aborts the deploy with a concrete
//! pointer to the offending finding. There is no `--skip-review`
//! flag; v0.6.5 removed every `--skip-*` override on the principle
//! that every prior emergency-bypass shipped a regression we paid for
//! later.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::pipeline::StageContext;

#[derive(Debug)]
struct Finding {
    id: String,
    fields: std::collections::HashMap<String, String>,
}

const REQUIRED_FIELDS: &[&str] = &[
    "id",
    "reviewer",
    "file",
    "claim",
    "verdict",
    "evidence",
    "resolution",
];

const VALID_VERDICTS: &[&str] = &["TRUE", "FALSE", "DEPENDENT"];

pub fn run(ctx: &StageContext) -> Result<()> {
    let reviews_dir = ctx.cfg.workspace.join(".syntaur/reviews");
    if !reviews_dir.exists() {
        log::debug!(
            "[review-triage] no reviews directory at {}; nothing to triage",
            reviews_dir.display()
        );
        return Ok(());
    }

    let mut total_files = 0usize;
    let mut total_findings = 0usize;
    let mut total_unresolved = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for entry in fs::read_dir(&reviews_dir).with_context(|| format!("read {}", reviews_dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        total_files += 1;
        let body = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;

        // Skip files that have an explicit "archived" marker among
        // the first non-blank lines (BOM / leading whitespace
        // tolerated). Operator marks a fully-triaged file as archived
        // without deleting it.
        let first_meaningful = body
            .lines()
            .map(|l| l.trim_start_matches('\u{feff}').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("");
        if first_meaningful == "<!-- syntaur-review-archived -->" {
            log::debug!("[review-triage] {} archived; skipping", path.display());
            continue;
        }

        let parsed = parse(&path, &body)
            .with_context(|| format!("malformed finding block in {}", path.display()))?;
        for finding in parsed {
            total_findings += 1;
            if let Err(reason) = validate(&finding) {
                total_unresolved += 1;
                errors.push(format!(
                    "  ✗ {} (finding {}): {}",
                    path.display(),
                    finding.id,
                    reason
                ));
            }
        }
    }

    if total_files == 0 {
        log::info!("[review-triage] reviews dir empty — pass");
        return Ok(());
    }

    if !errors.is_empty() {
        let body = errors.join("\n");
        anyhow::bail!(
            "review-triage: {}/{} finding(s) across {} file(s) lack a verdict+evidence+resolution. \
             Triage them before shipping — open the listed file(s) and complete the missing fields:\n{}\n\n\
             Format help: see syntaur-ship/src/stages/review_triage.rs module doc.",
            total_unresolved,
            total_findings,
            total_files,
            body
        );
    }

    log::info!(
        "[review-triage] ✓ {} finding(s) across {} review file(s) all triaged (verdict+evidence+resolution)",
        total_findings,
        total_files
    );
    Ok(())
}

fn parse(path: &Path, body: &str) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    let mut in_block = false;
    let mut current: Option<Finding> = None;
    // Track the most recent key so we can fold continuation lines
    // (lines without `key:` syntax) into multi-line field values.
    let mut last_key: Option<String> = None;

    for (lineno, line) in body.lines().enumerate() {
        let t = line.trim();
        if t == "```finding" {
            if in_block {
                anyhow::bail!(
                    "{}:{}: nested ```finding block (previous block never closed)",
                    path.display(),
                    lineno + 1
                );
            }
            in_block = true;
            current = Some(Finding {
                id: String::new(),
                fields: std::collections::HashMap::new(),
            });
            last_key = None;
            continue;
        }
        if t == "```" && in_block {
            in_block = false;
            if let Some(mut f) = current.take() {
                if let Some(id) = f.fields.get("id") {
                    f.id = id.clone();
                } else {
                    f.id = format!("(unnamed @ {})", path.display());
                }
                findings.push(f);
            }
            last_key = None;
            continue;
        }
        if !in_block {
            continue;
        }
        // Inside a finding block. Three cases:
        //   1. `key: value`            — start a new field
        //   2. continuation indented   — append to last field's value
        //   3. blank line              — reset continuation tracking
        if t.is_empty() {
            last_key = None;
            continue;
        }
        if let Some(idx) = line.find(':') {
            let key_candidate = line[..idx].trim();
            // A "key:" line must have a syntactically reasonable key:
            // ASCII alphanumeric + underscore, no whitespace, non-empty.
            // Lines like `evidence: src/foo.rs:10 quote → "..."` will
            // match the FIRST colon (after `evidence`), and the rest
            // (including `:10` and any other colons) becomes the value.
            let is_key_line = !key_candidate.is_empty()
                && key_candidate
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if is_key_line {
                if let Some(f) = current.as_mut() {
                    if f.fields.contains_key(key_candidate) {
                        anyhow::bail!(
                            "{}:{}: duplicate field '{}' in finding block",
                            path.display(),
                            lineno + 1,
                            key_candidate
                        );
                    }
                    let value = line[idx + 1..].trim().to_string();
                    f.fields.insert(key_candidate.to_string(), value);
                    last_key = Some(key_candidate.to_string());
                }
                continue;
            }
        }
        // Continuation line — fold into last_key's value.
        if let (Some(key), Some(f)) = (&last_key, current.as_mut()) {
            if let Some(existing) = f.fields.get_mut(key) {
                if !existing.is_empty() {
                    existing.push(' ');
                }
                existing.push_str(t);
            }
        }
    }
    if in_block {
        anyhow::bail!(
            "{}: ```finding block opened but never closed (no matching ``` line)",
            path.display()
        );
    }
    Ok(findings)
}

fn validate(f: &Finding) -> Result<(), String> {
    let mut missing: Vec<&str> = Vec::new();
    for k in REQUIRED_FIELDS {
        match f.fields.get(*k) {
            Some(v) if !v.is_empty() => {}
            _ => missing.push(k),
        }
    }
    if !missing.is_empty() {
        return Err(format!("missing field(s): {}", missing.join(", ")));
    }
    let verdict = f.fields.get("verdict").map(|s| s.as_str()).unwrap_or("");
    if !VALID_VERDICTS.contains(&verdict) {
        return Err(format!(
            "verdict must be one of {:?}, got '{}'",
            VALID_VERDICTS, verdict
        ));
    }
    // Evidence must reference a concrete file:line — no hand-waving.
    // The check rejects `:`-less strings AND `:`-strings that don't
    // have ≥1 digit immediately after a colon (so URLs, "Note: see..."
    // etc. don't satisfy the gate). We don't need a regex crate here —
    // a manual scan is plenty.
    let evidence = f.fields.get("evidence").map(|s| s.as_str()).unwrap_or("");
    let has_file_line_ref = {
        let mut ok = false;
        let bytes = evidence.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                ok = true;
                break;
            }
        }
        ok
    };
    if !has_file_line_ref {
        return Err(format!(
            "evidence must reference a concrete file:line (got '{}')",
            evidence
        ));
    }
    // Resolution must START WITH one of the canonical prefixes,
    // followed by either end-of-string or a separator (space, ':', '—').
    // Accepts: "fix-in-place", "fix-in-place: details", "tracked: path",
    // "wont-fix — explanation". Rejects: "fixedit", "skip-it".
    let resolution = f.fields.get("resolution").map(|s| s.as_str()).unwrap_or("");
    let valid_resolutions = ["fix-in-place", "tracked", "wont-fix"];
    let resolution_ok = valid_resolutions.iter().any(|prefix| {
        if resolution == *prefix {
            return true;
        }
        if !resolution.starts_with(prefix) {
            return false;
        }
        // Char immediately after the prefix must be a recognised
        // separator so "fix-in-placeX" doesn't sneak through.
        let rest = &resolution[prefix.len()..];
        rest.starts_with(':')
            || rest.starts_with(' ')
            || rest.starts_with('\t')
            || rest.starts_with('—') // em dash
            || rest.starts_with('-')
    });
    if !resolution_ok {
        return Err(format!(
            "resolution must start with one of {:?}, got '{}'",
            valid_resolutions, resolution
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p() -> PathBuf {
        PathBuf::from("test.md")
    }

    #[test]
    fn parses_well_formed_finding() {
        let body = "intro\n```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nline: 10\nclaim: x\nverdict: TRUE\nevidence: src/foo.rs:10 quote → \"...\"\nresolution: fix-in-place\n```\n";
        let f = parse(&p(), body).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "a-1");
        assert!(validate(&f[0]).is_ok());
    }

    #[test]
    fn folds_continuation_lines_into_previous_field() {
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: line one\n  line two continuation\nverdict: TRUE\nevidence: src/foo.rs:10\nresolution: fix-in-place\n```";
        let f = parse(&p(), body).unwrap();
        assert_eq!(f[0].fields.get("claim").map(|s| s.as_str()), Some("line one line two continuation"));
    }

    #[test]
    fn rejects_unclosed_block() {
        let body = "```finding\nid: a-1\nclaim: never closed\n";
        let err = parse(&p(), body).unwrap_err();
        assert!(format!("{err}").contains("never closed"));
    }

    #[test]
    fn rejects_duplicate_field() {
        let body = "```finding\nid: a-1\nclaim: first\nclaim: second\n```";
        let err = parse(&p(), body).unwrap_err();
        assert!(format!("{err}").contains("duplicate"));
    }

    #[test]
    fn rejects_missing_evidence() {
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: x\nverdict: FALSE\nresolution: wont-fix: hallucination\n```";
        let f = parse(&p(), body).unwrap();
        assert!(validate(&f[0]).is_err());
    }

    #[test]
    fn rejects_evidence_without_line_ref() {
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: x\nverdict: TRUE\nevidence: looked at it\nresolution: fix-in-place\n```";
        let f = parse(&p(), body).unwrap();
        assert!(validate(&f[0]).is_err());
    }

    #[test]
    fn rejects_evidence_with_url_only() {
        // The :// of a URL has letters after the colon, not digits, so
        // it must NOT satisfy the file:line gate.
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: x\nverdict: TRUE\nevidence: see https://example.com/foo\nresolution: fix-in-place\n```";
        let f = parse(&p(), body).unwrap();
        assert!(validate(&f[0]).is_err());
    }

    #[test]
    fn rejects_unknown_verdict() {
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: x\nverdict: MAYBE\nevidence: src/foo.rs:1\nresolution: fix-in-place\n```";
        let f = parse(&p(), body).unwrap();
        assert!(validate(&f[0]).is_err());
    }

    #[test]
    fn rejects_unknown_resolution() {
        let body = "```finding\nid: a-1\nreviewer: gemini\nfile: src/foo.rs\nclaim: x\nverdict: TRUE\nevidence: src/foo.rs:1\nresolution: skip-it\n```";
        let f = parse(&p(), body).unwrap();
        assert!(validate(&f[0]).is_err());
    }
}
