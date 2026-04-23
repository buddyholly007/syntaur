//! Git push stage — `git push origin main` from the workspace.

use anyhow::{Context, Result};
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    log::info!(">> git -C {} push origin main", ctx.cfg.workspace.display());
    if ctx.opts.dry_run {
        return Ok(());
    }
    let status = Command::new("git")
        .args(["-C", ctx.cfg.workspace.to_str().unwrap(), "push", "origin", "main"])
        .status()
        .context("git push")?;
    if !status.success() {
        anyhow::bail!("git push exited {status}");
    }
    Ok(())
}
