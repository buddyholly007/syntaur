//! Build stage — runs `cargo build --release -p syntaur-gateway -p mace`
//! in the workspace, then `cargo build --release` in rust-social-manager.
//! Mirrors deploy.sh lines 77-83.
//!
//! The build inherits the user's cargo env; we source `~/.cargo/env`
//! via the inherited shell env. `run_stream` is deliberately a straight
//! `std::process::Command` pass-through so Sean sees the same `cargo`
//! output he'd see running it by hand.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use crate::pipeline::StageContext;

/// Resolve `cargo` to an absolute path. Non-interactive SSH shells on
/// claudevm don't source ~/.cargo/env so `cargo` isn't on PATH; we
/// prefer ~/.cargo/bin/cargo, fall back to `cargo` (works in login
/// shells), bail with a clear message if neither resolves.
fn cargo_bin() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin/cargo");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("cargo")
}

pub fn run(ctx: &StageContext) -> Result<()> {
    let ws = ctx.cfg.workspace.to_str().unwrap();
    let sm = ctx.cfg.social_manager.to_str().unwrap();
    let cargo = cargo_bin();

    log::info!(">> cd {ws} && cargo build --release -p syntaur-gateway -p mace");
    if !ctx.opts.dry_run {
        let status = Command::new(&cargo)
            .args(["build", "--release", "-p", "syntaur-gateway", "-p", "mace"])
            .current_dir(&ctx.cfg.workspace)
            .status()
            .context("cargo build gateway+mace")?;
        if !status.success() {
            anyhow::bail!("cargo build (workspace) exited {status}");
        }
    }

    if ctx.cfg.social_manager.join("Cargo.toml").exists() {
        log::info!(">> cd {sm} && cargo build --release");
        if !ctx.opts.dry_run {
            let status = Command::new(&cargo)
                .args(["build", "--release"])
                .current_dir(&ctx.cfg.social_manager)
                .status()
                .context("cargo build rust-social-manager")?;
            if !status.success() {
                anyhow::bail!("cargo build (rust-social-manager) exited {status}");
            }
        }
    } else {
        log::warn!("[build] {} not present; skipping social-manager build", sm);
    }

    Ok(())
}
