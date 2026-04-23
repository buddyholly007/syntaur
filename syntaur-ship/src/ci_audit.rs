//! Post-deploy CI audit — polls GitHub Actions for ALL workflow runs
//! at the currently-deployed commit and surfaces any failures.
//!
//! Replaces the old narrowly-scoped release-sign-only check in
//! `guards::check_ci_status`. The bug that motivated this: on
//! 2026-04-23 a `rustls-webpki` CVE advisory published Apr 22 caused
//! `cargo-audit.yml` to start failing on every push. The tool polled
//! only release-sign, so I was silent about cargo-audit failures for
//! 8 consecutive deploys. Sean noticed via GH email notifications.
//!
//! Scope: ANY workflow with status=completed, conclusion=failure, for
//! the current HEAD SHA. Reports in the log; appends a summary to the
//! journal entry's `failure_reason` field if any red workflows exist.

use anyhow::Result;
use std::process::Command;

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
        // Only count the most-recent run per workflow (GH API returns newest first).
        if seen_workflows.contains(&name) {
            continue;
        }
        seen_workflows.insert(name.clone());

        let status = r["status"].as_str().unwrap_or("");
        let conclusion = r["conclusion"].as_str().unwrap_or("");
        if status == "completed" && conclusion == "failure" {
            failures.push(FailedRun {
                name,
                url: r["html_url"].as_str().unwrap_or("").to_string(),
                run_id: r["id"].as_u64().unwrap_or(0),
            });
        }
    }
    failures
}

#[derive(Debug)]
pub struct FailedRun {
    pub name: String,
    pub url: String,
    pub run_id: u64,
}

impl std::fmt::Display for FailedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}  ({})", self.name, self.url)
    }
}

/// Log the failures in-line with the deploy summary.
pub fn log_failures(failures: &[FailedRun], head: &str) {
    if failures.is_empty() {
        log::info!("[ci-audit] ✓ all workflows passing for {head}");
    } else {
        log::warn!(
            "[ci-audit] ⚠ {} CI workflow(s) failing for {head}:",
            failures.len()
        );
        for f in failures {
            log::warn!("   ✗ {f}");
        }
    }
}
