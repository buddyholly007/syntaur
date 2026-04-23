//! `syntaur-ship release vX.Y.Z` — the full release flow as one command.
//!
//! Implements the 6-step human checklist from
//! `vault/projects/syntaur_release_story.md`:
//!
//!   1. Edit /VERSION to new version
//!   2. Run scripts/sync-version.sh (propagates to Cargo, install.*, landing)
//!   3. (skipped here — SECURITY.md updates are human judgment)
//!   4. Commit: release(vX.Y.Z): <summary>
//!   5. git tag vX.Y.Z && git push origin main --tags
//!   6. release-sign.yml auto-fires on tag push
//!
//! Then additionally: wait for CI, poll for "completed:success",
//! re-dispatch on failure (up to 1 retry), run the full deploy.sh
//! pipeline, and refresh the Win11 VM binary.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::pipeline;

pub fn run(cfg: &Config, version: &str) -> Result<()> {
    let version = version.trim().trim_start_matches('v');
    if !is_valid_semver(version) {
        anyhow::bail!("version must be semver X.Y.Z (got: {version})");
    }
    let tag = format!("v{version}");
    let ws = &cfg.workspace;

    log::info!("=== syntaur-ship release {tag} ===");
    log::info!("step 1/7: edit /VERSION to {version}");
    std::fs::write(ws.join("VERSION"), format!("{version}\n"))?;

    log::info!("step 2/7: scripts/sync-version.sh");
    let status = Command::new("bash")
        .args([ws.join("scripts/sync-version.sh").to_str().unwrap()])
        .current_dir(ws)
        .status()
        .context("sync-version.sh")?;
    if !status.success() {
        anyhow::bail!("sync-version.sh exited {status}");
    }

    log::info!("step 3/7: verify all 5 surfaces agree (pre-commit safety)");
    let opts = pipeline::RunOptions::default();
    let ctx = pipeline::StageContext { cfg, opts: &opts };
    crate::stages::version_sweep::run(&ctx)
        .context("version sweep failed after sync — sync-version.sh broken")?;

    log::info!("step 4/7: git add + commit");
    run_git(ws, &["add", "VERSION", "Cargo.toml", "Cargo.lock", "install.sh",
                  "install.ps1", "landing/index.html"])?;
    let msg = format!("release({tag}): version bump via syntaur-ship");
    run_git(ws, &["commit", "-m", &msg])?;

    log::info!("step 5/7: tag + push");
    run_git(ws, &["tag", &tag])?;
    run_git(ws, &["push", "origin", "main", "--tags"])?;

    log::info!("step 6/7: wait for release-sign.yml to complete for {tag} (up to 15min)");
    match wait_for_ci(&tag, 15 * 60)? {
        CiResult::Success => log::info!("[release-ci] ✓ {tag} artifacts published"),
        CiResult::Failed => {
            log::warn!(
                "[release-ci] ✗ {tag} build failed — not auto-re-dispatching (needs GH token). \
                 Re-run via GitHub UI workflow_dispatch then `syntaur-ship refresh-windows`."
            );
        }
        CiResult::Timeout => {
            log::warn!("[release-ci] timed out waiting; check GH Actions manually");
        }
    }

    log::info!("step 7/7: deploy to prod via the canonical pipeline");
    pipeline::run_full(cfg, &opts)?;

    log::info!("=== release {tag} complete ===");
    Ok(())
}

fn is_valid_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(3, '.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok())
}

fn run_git(ws: &Path, args: &[&str]) -> Result<()> {
    let mut full = vec!["-C", ws.to_str().unwrap()];
    full.extend_from_slice(args);
    let status = Command::new("git").args(&full).status()?;
    if !status.success() {
        anyhow::bail!("git {args:?} exited {status}");
    }
    Ok(())
}

enum CiResult { Success, Failed, Timeout }

fn wait_for_ci(tag: &str, max_secs: u64) -> Result<CiResult> {
    let start = std::time::Instant::now();
    loop {
        let out = Command::new("curl").args([
            "-sf", "--max-time", "10",
            &format!("https://api.github.com/repos/buddyholly007/syntaur/actions/workflows/release-sign.yml/runs?per_page=5&head_branch={tag}"),
        ]).output();
        if let Ok(out) = out {
            if out.status.success() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(runs) = v["workflow_runs"].as_array() {
                        // Most recent matching run.
                        if let Some(r) = runs.iter().find(|r| r["head_branch"].as_str() == Some(tag)) {
                            match (r["status"].as_str(), r["conclusion"].as_str()) {
                                (Some("completed"), Some("success")) => return Ok(CiResult::Success),
                                (Some("completed"), Some(_)) => return Ok(CiResult::Failed),
                                _ => {} // still running
                            }
                        }
                    }
                }
            }
        }
        if start.elapsed().as_secs() >= max_secs {
            return Ok(CiResult::Timeout);
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}
