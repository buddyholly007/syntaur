//! Post-deploy CI audit — polls GitHub Actions for the workflow runs
//! at the currently-deployed commit and surfaces any failures.
//!
//! Replaces the old narrowly-scoped release-sign-only check in
//! `guards::check_ci_status`. The bug that motivated this: on
//! 2026-04-23 a `rustls-webpki` CVE advisory published Apr 22 caused
//! `cargo-audit.yml` to start failing on every push. The tool polled
//! only release-sign, so I was silent about cargo-audit failures for
//! 8 consecutive deploys.
//!
//! **Two distinct sets** of workflows:
//!
//! - **DEPLOY_GATING** — workflows whose pass status is required to
//!   ship. A failure here aborts the pipeline. Cargo Audit, CodeQL,
//!   Version consistency check are all here because each represents a
//!   real product-safety claim (no published CVEs in deps, no static
//!   security regressions, version surfaces agree).
//!
//! - **INFORMATIONAL** — surfaced as warnings in the deploy log but
//!   don't block ship. Examples: Docker — Nightly Base Refresh
//!   (publishes a side-channel container image to GHCR; the actual
//!   prod path is rsync to TrueNAS, never GHCR). A failure here is
//!   real and gets logged, but it doesn't gate shipping the binary.
//!
//! Why not just block on every red workflow? Because v0.6.5 made the
//! gate fix-or-block (no `--force-ci-drift` override). If a side-channel
//! GHCR publish fails for an environmental reason — registry rate
//! limit, GHA cache corruption, base-image transient — there is no
//! 'fix the deploy code' that helps; the right action is to investigate
//! the GHCR push separately while shipping the prod TrueNAS binary.
//! Without this split, a chronic Docker-nightly failure (which has
//! happened) freezes ALL prod shipping. With the split, Docker-nightly
//! gets investigated as its own thing and prod ships unblocked when
//! the deploy-critical workflows are green.
//!
//! When a workflow's category is wrong, edit DEPLOY_GATING below — do
//! not add a runtime override flag. The whole v0.6.5 thesis is that
//! every override flag we ever added shipped a regression we paid for
//! later. The category lives at code-review time, not deploy time.

use std::process::Command;

/// Workflows whose green status is REQUIRED to ship. Edit at code-review
/// time when adding a new safety-critical workflow; never override at
/// runtime.
pub const DEPLOY_GATING_WORKFLOWS: &[&str] = &[
    "Cargo Audit",
    "CodeQL",
    "Version consistency check",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Aborts the pipeline.
    Blocking,
    /// Logged, doesn't block ship.
    Informational,
}

pub fn run(git_head: &str) -> Vec<FailedRun> {
    let url = format!(
        "https://api.github.com/repos/buddyholly007/syntaur/actions/runs?head_sha={git_head}&per_page=20"
    );
    let out = Command::new("curl")
        .args(["-sf", "--max-time", "15", &url])
        .output();
    let Ok(out) = out else {
        log::debug!("[ci-audit] GitHub API unreachable");
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let json: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let runs = match json["workflow_runs"].as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut failures: Vec<FailedRun> = Vec::new();
    let mut seen_workflows: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in runs {
        let name = r["name"].as_str().unwrap_or("").to_string();
        if seen_workflows.contains(&name) {
            continue;
        }
        seen_workflows.insert(name.clone());

        let status = r["status"].as_str().unwrap_or("");
        let conclusion = r["conclusion"].as_str().unwrap_or("");
        // Treat any non-success terminal state as a failure: failure,
        // timed_out, cancelled, action_required, stale. "neutral" and
        // "skipped" are intentional non-failures and don't count.
        let is_failure = status == "completed"
            && matches!(
                conclusion,
                "failure" | "timed_out" | "cancelled" | "action_required" | "stale"
            );
        if is_failure {
            let severity = if DEPLOY_GATING_WORKFLOWS.contains(&name.as_str()) {
                Severity::Blocking
            } else {
                Severity::Informational
            };
            failures.push(FailedRun {
                name,
                url: r["html_url"].as_str().unwrap_or("").to_string(),
                run_id: r["id"].as_u64().unwrap_or(0),
                severity,
            });
        }
    }
    failures
}

/// Returns just the deploy-blocking failures (subset of `run()`).
pub fn blocking(failures: &[FailedRun]) -> Vec<&FailedRun> {
    failures.iter().filter(|f| f.severity == Severity::Blocking).collect()
}

#[derive(Debug)]
pub struct FailedRun {
    pub name: String,
    pub url: String,
    pub run_id: u64,
    pub severity: Severity,
}

impl std::fmt::Display for FailedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Blocking => "BLOCKING",
            Severity::Informational => "info",
        };
        write!(f, "[{}] {}  ({})", tag, self.name, self.url)
    }
}

/// Log the failures in-line with the deploy summary.
pub fn log_failures(failures: &[FailedRun], head: &str) {
    if failures.is_empty() {
        log::info!("[ci-audit] ✓ all workflows passing for {head}");
        return;
    }
    let blocking_count = failures.iter().filter(|f| f.severity == Severity::Blocking).count();
    let info_count = failures.len() - blocking_count;
    log::warn!(
        "[ci-audit] ⚠ {} CI workflow(s) failing for {head} ({} blocking, {} informational):",
        failures.len(),
        blocking_count,
        info_count
    );
    for f in failures {
        match f.severity {
            Severity::Blocking => log::warn!("   ✗ {f}"),
            Severity::Informational => log::info!("   · {f}"),
        }
    }
}
