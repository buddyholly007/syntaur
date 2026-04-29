//! Pre-flight documentation-claim audit. Walks `docs/` for HTML comments
//! tagging verifiable claims and asserts each against the workspace.
//!
//! Motivation: the 2026-04-29 external security review caught two doc
//! drifts that had been live for weeks — `threat-model.md` said
//! "applies to v0.4.x" while `VERSION` said 0.5.9, and claimed HSTS was
//! emitting when the gate was silently broken so HSTS never appeared on
//! prod. A human reviewer had to cross-reference docs against reality.
//! Encoding the cross-references makes them automatic.
//!
//! Claim grammar (single-line HTML comments, anywhere in any `.md`
//! under `docs/`):
//!
//! ```text
//!   <!-- syntaur-doc-claim applies_to_version || PREFIX -->
//!   <!-- syntaur-doc-claim code_grep || FILE || NEEDLE -->
//!   <!-- syntaur-doc-claim code_no_match || FILE || NEEDLE -->
//! ```
//!
//! - `applies_to_version PREFIX` — `/VERSION` must start with `PREFIX`
//!   (e.g. `0.5` matches `0.5.9` and `0.5.0` but not `0.4.7`).
//! - `code_grep FILE NEEDLE` — file at `FILE` (relative to workspace)
//!   must contain `NEEDLE` as a substring.
//! - `code_no_match FILE NEEDLE` — same, must NOT contain.
//!
//! `NEEDLE` is a plain substring match. Keep it small and unique. If
//! the surrounding code is refactored such that the substring no
//! longer appears verbatim, the audit fails — that's the point: the
//! doc claim was tied to an implementation detail, and now both must
//! be re-validated.
//!
//! Stage runs after `preflight` and `version_sweep`, BEFORE build,
//! snapshot, or any deploy work. Cheap local file reads only.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::pipeline::StageContext;

const MARKER: &str = "<!-- syntaur-doc-claim";
const TRAIL: &str = "-->";
const SEP: &str = "||";

#[derive(Debug)]
enum Claim {
    AppliesToVersion(String),
    CodeGrep { file: PathBuf, needle: String },
    CodeNoMatch { file: PathBuf, needle: String },
}

#[derive(Debug)]
struct Located {
    claim: Claim,
    source_doc: PathBuf,
    line_no: usize,
}

pub fn run(ctx: &StageContext) -> Result<()> {
    let ws = &ctx.cfg.workspace;
    let docs_root = ws.join("docs");
    if !docs_root.exists() {
        log::info!("[doc-audit] docs/ missing, skipping");
        return Ok(());
    }
    log::info!("[doc-audit] scanning docs/ for tagged claims");

    let mut found: Vec<Located> = Vec::new();
    walk_md(&docs_root, &mut |path, content| {
        // Track fenced code blocks. Lines inside ``` ... ``` are
        // documentation examples, not real claims — the audit skips
        // them so docs can show users how to write a claim without
        // tripping the audit on the example.
        let mut in_fence = false;
        for (i, line) in content.lines().enumerate() {
            // Strip leading whitespace + optional Markdown blockquote
            // prefix (` > `) before checking for the fence delimiter,
            // so blockquoted ``` ``` examples are also treated as code.
            let bare = line.trim_start();
            let bare = bare.strip_prefix('>').map(str::trim_start).unwrap_or(bare);
            if bare.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            if let Some(claim) = parse_line(line) {
                found.push(Located {
                    claim,
                    source_doc: path.to_path_buf(),
                    line_no: i + 1,
                });
            }
        }
    })?;

    if found.is_empty() {
        log::info!("[doc-audit] no tagged claims found — skipping");
        return Ok(());
    }
    log::info!("[doc-audit] verifying {} tagged claim(s)", found.len());

    let version = std::fs::read_to_string(ws.join("VERSION"))
        .context("read /VERSION")?
        .trim()
        .to_string();

    let mut failures = Vec::new();
    for c in &found {
        if let Err(msg) = verify(&c.claim, &version, ws) {
            failures.push(format!(
                "  {}:{} — {msg}",
                c.source_doc
                    .strip_prefix(ws)
                    .unwrap_or(&c.source_doc)
                    .display(),
                c.line_no
            ));
        }
    }

    if !failures.is_empty() {
        let mut msg = format!(
            "doc-audit caught {} stale doc claim(s):\n",
            failures.len()
        );
        for f in &failures {
            msg.push('\n');
            msg.push_str(f);
        }
        msg.push_str(
            "\n\nUpdate the doc to reflect current state, OR fix the code so the claim is true.",
        );
        anyhow::bail!(msg);
    }

    log::info!(
        "[doc-audit] ✓ all {} tagged claim(s) match workspace state",
        found.len()
    );
    Ok(())
}

fn verify(claim: &Claim, version: &str, ws: &Path) -> std::result::Result<(), String> {
    match claim {
        Claim::AppliesToVersion(prefix) => {
            if version.starts_with(prefix.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "applies_to_version: doc claims prefix {prefix:?} but VERSION='{version}'"
                ))
            }
        }
        Claim::CodeGrep { file, needle } => {
            let abs = ws.join(file);
            let text = std::fs::read_to_string(&abs)
                .map_err(|e| format!("read {}: {e}", abs.display()))?;
            if text.contains(needle.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "code_grep: {:?} has no occurrence of {needle:?}",
                    file.display()
                ))
            }
        }
        Claim::CodeNoMatch { file, needle } => {
            let abs = ws.join(file);
            let text = std::fs::read_to_string(&abs)
                .map_err(|e| format!("read {}: {e}", abs.display()))?;
            if !text.contains(needle.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "code_no_match: {:?} still contains {needle:?}",
                    file.display()
                ))
            }
        }
    }
}

fn parse_line(line: &str) -> Option<Claim> {
    // Tolerate leading whitespace + Markdown blockquote prefix `> `.
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix('>').map(str::trim_start).unwrap_or(trimmed);
    let after_marker = trimmed.strip_prefix(MARKER)?.trim();
    let body = after_marker
        .rsplit_once(TRAIL)
        .map(|(b, _)| b.trim())
        .unwrap_or(after_marker.trim());
    let parts: Vec<&str> = body.split(SEP).map(str::trim).collect();
    let kind = parts.first()?;
    match *kind {
        "applies_to_version" => {
            let prefix = parts.get(1).filter(|p| !p.is_empty())?;
            Some(Claim::AppliesToVersion((*prefix).to_string()))
        }
        "code_grep" => {
            let file = parts.get(1).filter(|p| !p.is_empty())?;
            let needle = parts.get(2).filter(|n| !n.is_empty())?;
            Some(Claim::CodeGrep {
                file: PathBuf::from(*file),
                needle: (*needle).to_string(),
            })
        }
        "code_no_match" => {
            let file = parts.get(1).filter(|p| !p.is_empty())?;
            let needle = parts.get(2).filter(|n| !n.is_empty())?;
            Some(Claim::CodeNoMatch {
                file: PathBuf::from(*file),
                needle: (*needle).to_string(),
            })
        }
        _ => None,
    }
}

fn walk_md(root: &Path, visit: &mut dyn FnMut(&Path, &str)) -> Result<()> {
    fn rec(p: &Path, visit: &mut dyn FnMut(&Path, &str)) -> Result<()> {
        if p.is_dir() {
            for entry in std::fs::read_dir(p).with_context(|| p.display().to_string())? {
                let entry = entry?;
                rec(&entry.path(), visit)?;
            }
            return Ok(());
        }
        if p.extension().and_then(|e| e.to_str()) != Some("md") {
            return Ok(());
        }
        let text = std::fs::read_to_string(p).with_context(|| p.display().to_string())?;
        visit(p, &text);
        Ok(())
    }
    rec(root, visit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_applies_to_version() {
        let c = parse_line("<!-- syntaur-doc-claim applies_to_version || 0.5 -->").unwrap();
        match c {
            Claim::AppliesToVersion(p) => assert_eq!(p, "0.5"),
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn parses_code_grep_with_path_and_needle() {
        let c = parse_line(
            "<!-- syntaur-doc-claim code_grep || src/security.rs || req.headers().get(\"host\") -->",
        )
        .unwrap();
        match c {
            Claim::CodeGrep { file, needle } => {
                assert_eq!(file, PathBuf::from("src/security.rs"));
                assert_eq!(needle, r#"req.headers().get("host")"#);
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn parses_code_no_match() {
        let c = parse_line(
            "<!-- syntaur-doc-claim code_no_match || foo.rs || forbidden_pattern -->",
        )
        .unwrap();
        match c {
            Claim::CodeNoMatch { file, needle } => {
                assert_eq!(file, PathBuf::from("foo.rs"));
                assert_eq!(needle, "forbidden_pattern");
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn ignores_unrelated_html_comments() {
        assert!(parse_line("<!-- regular comment -->").is_none());
        assert!(parse_line("<!-- syntaur-doc-claim unknown_kind || foo -->").is_none());
        assert!(parse_line("plain text").is_none());
    }

    #[test]
    fn tolerates_blockquote_prefix() {
        let c = parse_line("> <!-- syntaur-doc-claim applies_to_version || 0.5 -->").unwrap();
        match c {
            Claim::AppliesToVersion(p) => assert_eq!(p, "0.5"),
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn verify_applies_to_version_match() {
        let r = verify(
            &Claim::AppliesToVersion("0.5".into()),
            "0.5.9",
            Path::new("/tmp"),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn verify_applies_to_version_drift() {
        let r = verify(
            &Claim::AppliesToVersion("0.4".into()),
            "0.5.9",
            Path::new("/tmp"),
        );
        assert!(r.is_err());
    }
}
