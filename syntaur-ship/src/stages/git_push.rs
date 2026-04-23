//! Git push stage — `git push origin main` from the workspace.

use anyhow::{Context, Result};
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    log::info!(">> git -C {} push origin main", ctx.cfg.workspace.display());
    if ctx.opts.dry_run {
        return Ok(());
    }
    // `SYNTAUR_SHIP_OVERRIDE=1` is the contract the pre-push hook
    // (Phase 8) watches for. Without this, syntaur-ship blocks its
    // own git push because the deploy-stamp on disk still points at
    // the PREVIOUS deploy — mid-pipeline the stamp hasn't been
    // updated yet. Setting the override here is safe: this code
    // path only runs from inside the pipeline, after preflight /
    // CI gate / Mac Mini smoke / canary have all passed.
    let status = Command::new("git")
        .env("SYNTAUR_SHIP_OVERRIDE", "1")
        .args(["-C", ctx.cfg.workspace.to_str().unwrap(), "push", "origin", "main"])
        .status()
        .context("git push")?;
    if !status.success() {
        anyhow::bail!("git push exited {status}");
    }
    Ok(())
}
