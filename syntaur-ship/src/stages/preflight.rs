//! Preflight stage — minimal Phase-1 checks.
//!
//! Phase 3 extends this with the full version-sweep. Phase 5 adds
//! Cargo.lock drift detection + CI status gate. Phase 1 just does the
//! absolute minimum: confirm the workspace path is a git repo, confirm
//! key files exist, and warn if the working tree is dirty.

use anyhow::{Context, Result};

use crate::pipeline::{run_capture, StageContext};

pub fn run(ctx: &StageContext) -> Result<()> {
    let ws = &ctx.cfg.workspace;
    if !ws.join(".git").exists() {
        anyhow::bail!("{} is not a git repo — check cfg.workspace", ws.display());
    }
    if !ws.join("Cargo.toml").exists() {
        anyhow::bail!("{}/Cargo.toml missing — broken workspace", ws.display());
    }
    if !ws.join("VERSION").exists() {
        anyhow::bail!("{}/VERSION missing — see projects/syntaur_release_story", ws.display());
    }

    // Warn on dirty working tree; don't fail. Sean frequently has
    // in-progress edits during a deploy (e.g. docstring tweaks), and
    // we don't want the tool to block those. Phase 5 hardens this.
    let status = run_capture("git", &["-C", ws.to_str().unwrap(), "status", "--porcelain"])
        .context("git status")?;
    let dirty = status.lines().count();
    if dirty > 0 && !ctx.opts.dry_run {
        log::warn!(
            "[preflight] git working tree has {dirty} uncommitted change(s); pipeline will deploy whatever gets built"
        );
    }

    let head = run_capture("git", &["-C", ws.to_str().unwrap(), "rev-parse", "--short", "HEAD"])?;
    let version = std::fs::read_to_string(ws.join("VERSION"))?.trim().to_string();
    log::info!("[preflight] workspace ok — HEAD={} VERSION={}", head.trim(), version);

    Ok(())
}
