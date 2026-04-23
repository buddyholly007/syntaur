//! Deploy-level guards: PID file (prevent concurrent deploys), CI
//! status gate (warn if release-sign.yml most-recent failed), and
//! Cargo.lock drift detection (force rebuild if deps changed since
//! last successful deploy).

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// PID file guard — refuse to start if another syntaur-ship is running.
/// Returns the PidLock guard which removes the file on Drop.
pub struct PidLock {
    path: std::path::PathBuf,
}

impl PidLock {
    pub fn try_acquire(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join("deploy.pid");
        if let Ok(content) = std::fs::read_to_string(&path) {
            let content = content.trim();
            if !content.is_empty() {
                if let Ok(pid) = content.parse::<u32>() {
                    // Check if that PID is still alive.
                    let alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
                    if alive {
                        anyhow::bail!(
                            "another syntaur-ship is running (PID {pid}); if stuck, \
                             remove {} manually after confirming the process is dead",
                            path.display()
                        );
                    } else {
                        log::warn!(
                            "[pid-lock] stale PID {pid} in {}; claiming lock",
                            path.display()
                        );
                    }
                }
            }
        }
        let my_pid = std::process::id().to_string();
        std::fs::write(&path, &my_pid)?;
        Ok(Self { path })
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Warn if the most recent release-sign.yml run for any tag FAILED.
/// This catches the v0.5.0-style case where the tag ships but CI
/// artifacts never published, leaving GH Releases stale.
///
/// Returns Ok(()) always — this is a warning, not a hard gate. The
/// tool logs the findings; deploy proceeds. A hard gate would require
/// a GH token for re-running the workflow, out of scope for Phase 5.
pub fn check_ci_status() -> Result<()> {
    let out = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            "https://api.github.com/repos/buddyholly007/syntaur/actions/workflows/release-sign.yml/runs?per_page=1",
        ])
        .output();
    let Ok(out) = out else {
        log::debug!("[ci-gate] GitHub API unreachable; skipping CI check");
        return Ok(());
    };
    if !out.status.success() {
        return Ok(());
    }
    let json: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let runs = json["workflow_runs"].as_array();
    let Some(runs) = runs else { return Ok(()) };
    let Some(latest) = runs.first() else { return Ok(()) };
    let conclusion = latest["conclusion"].as_str().unwrap_or("");
    let tag = latest["head_branch"].as_str().unwrap_or("?");
    let url = latest["html_url"].as_str().unwrap_or("");
    if conclusion == "failure" {
        log::warn!(
            "[ci-gate] ⚠ last release-sign run for {tag} FAILED — GitHub Releases may be \
             behind repo HEAD. Users installing via install.sh will pull the previous version. {url}"
        );
    } else {
        log::info!("[ci-gate] last release-sign run for {tag}: {conclusion}");
    }
    Ok(())
}

/// Compute SHA-256 of Cargo.lock. Tool uses this to detect dep shifts
/// that should force a rebuild even if --skip-build is passed.
pub fn cargo_lock_sha(workspace: &Path) -> Result<String> {
    let path = workspace.join("Cargo.lock");
    let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(crate::state::sha256_hex(&data))
}
