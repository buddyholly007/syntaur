//! Reviewer client — file-queue handoff to a live Claude Code session.
//!
//! Phase 2 originally POSTed each screenshot to OpenRouter's
//! `anthropic/claude-opus-4` for paid review. Sean's actual workflow
//! runs syntaur-verify from inside Claude Code (which IS Opus 4.7), so
//! paying OpenRouter to call Opus on his behalf was redundant. The
//! binary now writes a review-request packet next to each screenshot
//! and blocks until the running session writes back a response file.
//! No API spend, full Opus quality, but a run is bound to an
//! interactive reviewer.
//!
//! Wire format — for every `analyze_module_with_source` call the
//! binary writes:
//!
//!   `<screenshot>.review-req.json` — prompt + module context + path
//!                                    list to source files
//!   `<screenshot>.review-resp.json`— Opus findings (the reviewer
//!                                    writes this)
//!
//! Source files are referenced by path rather than embedded so the
//! reviewer can use its native Read tool — the request stays small
//! and the reviewer sees fresh file contents even after intervening
//! edits.
//!
//! The reviewer (i.e. you, reading this in a Claude Code session)
//! drains the queue with:
//!
//!   `find ~/.syntaur-verify/runs -name '*.review-req.json' -newer …`
//!
//! For each request: open the screenshot, read any listed source
//! files, return findings in the response JSON. The binary picks up
//! the response file automatically.
//!
//! The struct name `OpusClient` is preserved from Phase 2 so callers
//! in `syntaur_verify.rs` don't need to change. "Opus" still names
//! the reviewer's model — only the transport changed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::run::{Finding, FindingEdit, FindingKind, Severity};

/// How long to wait for a response file before failing the call.
/// Configurable via `SYNTAUR_VERIFY_REVIEW_TIMEOUT_SECS`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(900);

/// How often to poll for the response file. Cheap stat() on local fs;
/// no need for inotify wiring.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct OpusClient {
    timeout: Duration,
}

impl OpusClient {
    /// Phase 2 name — kept so `syntaur_verify.rs` doesn't need to be
    /// touched. Resolves the timeout from env (no vault interaction
    /// any more).
    pub fn from_vault() -> Result<Self> {
        Self::new()
    }

    pub fn new() -> Result<Self> {
        let timeout = std::env::var("SYNTAUR_VERIFY_REVIEW_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TIMEOUT);
        Ok(Self { timeout })
    }

    /// Phase 2a shim — no source context, no edits requested.
    pub async fn analyze_module(
        &self,
        module_slug: &str,
        url: &str,
        screenshot_path: &Path,
        changed_paths: &[String],
    ) -> Result<Vec<Finding>> {
        self.analyze_module_with_source(
            module_slug,
            url,
            screenshot_path,
            changed_paths,
            &[],
            /* request_edits = */ false,
        )
        .await
    }

    /// Write a review-request packet next to the screenshot and block
    /// until a response file shows up. Source files are sent as
    /// `(path, body)` tuples; the body is recorded verbatim in the
    /// request packet — but the reviewer is also free to re-Read the
    /// path, which yields fresher content after auto-fix iterations.
    pub async fn analyze_module_with_source(
        &self,
        module_slug: &str,
        url: &str,
        screenshot_path: &Path,
        changed_paths: &[String],
        source_files: &[(String, String)],
        request_edits: bool,
    ) -> Result<Vec<Finding>> {
        let (req_path, resp_path) = req_resp_paths(screenshot_path)?;

        let req = ReviewRequest {
            module_slug: module_slug.to_string(),
            url: url.to_string(),
            screenshot_path: screenshot_path.to_path_buf(),
            changed_paths: changed_paths.to_vec(),
            source_files: source_files
                .iter()
                .map(|(p, b)| SourceFile {
                    path: p.clone(),
                    body: b.clone(),
                })
                .collect(),
            request_edits,
            instructions: review_instructions(request_edits),
            response_schema: response_schema_hint(request_edits),
            written_at: Utc::now(),
        };

        // Atomic write: serialize → write tmp → rename. Avoids the
        // reviewer racing on a half-written file.
        let body = serde_json::to_vec_pretty(&req).context("serialize review request")?;
        let tmp = req_path.with_extension("json.tmp");
        std::fs::write(&tmp, &body)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &req_path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), req_path.display()))?;

        // Stderr is the channel a Claude Code session is most likely
        // to be tailing; logging through `log::info!` would also work
        // but the path-on-its-own-line form here is grep-friendly.
        eprintln!(
            "[verify] awaiting review · {}",
            req_path.display()
        );
        eprintln!(
            "[verify]   module={module_slug}  url={url}  shot={}",
            screenshot_path.display()
        );

        let resp_body = wait_for_response(&resp_path, self.timeout).await?;
        let parsed: ReviewResponse = serde_json::from_slice(&resp_body).with_context(|| {
            format!("parse review response from {}", resp_path.display())
        })?;

        let now = Utc::now();
        let findings = parsed
            .findings
            .into_iter()
            .map(|f| {
                let (severity, kind) = match f.kind.as_str() {
                    "regression" => (Severity::Regression, FindingKind::Other),
                    "improvement" => (Severity::Suggestion, FindingKind::Improvement),
                    _ => (Severity::Suggestion, FindingKind::Other),
                };
                let mut detail = f.detail;
                if let Some(fix) = f.suggested_fix {
                    if !fix.is_empty() {
                        detail.push_str("\n  suggested fix: ");
                        detail.push_str(&fix);
                    }
                }
                // Phase 2b policy carry-over: only regressions carry
                // edits. If the reviewer attached edits to an
                // improvement we drop them — auto-fix is for
                // breakage, not style tweaks.
                let edits = match severity {
                    Severity::Regression => f.edits.filter(|v| !v.is_empty()),
                    Severity::Suggestion => None,
                };
                Finding {
                    module_slug: module_slug.to_string(),
                    kind,
                    severity,
                    title: f.title,
                    detail,
                    artifact: Some(screenshot_path.to_path_buf()),
                    captured_at: now,
                    edits,
                    persona: None,
                }
            })
            .collect();

        Ok(findings)
    }
}

/// `<dir>/<stem>.review-req.json` and `<dir>/<stem>.review-resp.json`,
/// derived from the screenshot path. Co-locating the queue with the
/// screenshots keeps everything for one run in one dir.
fn req_resp_paths(screenshot_path: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = screenshot_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("screenshot has no parent dir: {}", screenshot_path.display()))?;
    let stem = screenshot_path
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("screenshot has no stem: {}", screenshot_path.display()))?
        .to_string_lossy()
        .into_owned();
    Ok((
        parent.join(format!("{stem}.review-req.json")),
        parent.join(format!("{stem}.review-resp.json")),
    ))
}

async fn wait_for_response(path: &Path, timeout: Duration) -> Result<Vec<u8>> {
    let started = Instant::now();
    loop {
        match std::fs::read(path) {
            Ok(b) if !b.is_empty() => return Ok(b),
            // Empty file = reviewer is mid-write; keep polling.
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
        if started.elapsed() > timeout {
            anyhow::bail!(
                "review timed out after {:?} waiting for {}",
                timeout,
                path.display()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn review_instructions(request_edits: bool) -> String {
    let edits_clause = if request_edits {
        "\n\nFor each REGRESSION, you MAY also attach `edits`: an array of \
         {file, old_string, new_string} objects that, applied in order, fix \
         the regression. Rules:\n\
         - Only edit files listed in `source_files`. Do not invent new files.\n\
         - `old_string` must match EXACTLY (bytes, whitespace, indentation) and \
           appear exactly ONCE in the file. Widen the window with surrounding \
           lines until it does.\n\
         - Prefer the smallest edit that fixes the regression. Multiple small \
           edits beat one large one.\n\
         - Keep total new code under ~40 lines per finding.\n\
         - Do NOT attach edits to improvements — auto-fix is for breakage only.\n\
         - If you can see the regression but the source context isn't enough to \
           fix it precisely, omit `edits`. A regression without edits still tells \
           the human what to look at."
    } else {
        ""
    };

    format!(
        "You are auditing one module of the Syntaur web application.\n\n\
         Open the screenshot at `screenshot_path`. Use Read on listed \
         `source_files` if you need code context — bodies are also embedded \
         inline for convenience.\n\n\
         Identify TWO categories of issues:\n\n\
         1. REGRESSIONS — UI elements cut off, overlapping text, unreadable \
         contrast, missing affordances, obvious alignment bugs, broken layouts, \
         mystery floating elements, placeholder/TODO/Lorem ipsum text leaking to \
         users.\n\n\
         2. IMPROVEMENTS — accessibility issues (alt text, tap targets, contrast \
         near WCAG AA), missing loading/empty/error states, unclear interaction \
         patterns, likely-broken mobile layouts.\n\n\
         Be strict on regressions — a first-time user wouldn't forgive them. \
         Be practical on improvements — only surface things with clear user \
         value.{edits_clause}\n\n\
         Write the response as JSON to `<screenshot_stem>.review-resp.json` in \
         the same directory as the request file. The binary is polling for it."
    )
}

fn response_schema_hint(request_edits: bool) -> serde_json::Value {
    if request_edits {
        serde_json::json!({
            "findings": [
                {
                    "kind": "regression | improvement",
                    "title": "short noun phrase (5-10 words)",
                    "detail": "one-sentence explanation + where on the page",
                    "suggested_fix": "optional natural-language description",
                    "edits": [
                        {"file": "path/from/source_files/list",
                         "old_string": "exact unique current text",
                         "new_string": "replacement"}
                    ]
                }
            ]
        })
    } else {
        serde_json::json!({
            "findings": [
                {
                    "kind": "regression | improvement",
                    "title": "short noun phrase",
                    "detail": "one-sentence explanation + where on the page",
                    "suggested_fix": "optional"
                }
            ]
        })
    }
}

// ── on-disk packet shapes ──────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ReviewRequest {
    module_slug: String,
    url: String,
    screenshot_path: PathBuf,
    changed_paths: Vec<String>,
    source_files: Vec<SourceFile>,
    request_edits: bool,
    instructions: String,
    response_schema: serde_json::Value,
    written_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct SourceFile {
    path: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct ReviewResponse {
    findings: Vec<RawFinding>,
}

#[derive(Debug, Deserialize)]
struct RawFinding {
    kind: String,
    title: String,
    detail: String,
    #[serde(default)]
    suggested_fix: Option<String>,
    #[serde(default)]
    edits: Option<Vec<FindingEdit>>,
}
