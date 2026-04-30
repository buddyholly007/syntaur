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

    // Refresh mtimes on every gateway+mace .rs file before cargo runs.
    //
    // Why: git-checkout preserves the committed file's mtime, which can
    // be older than the previously built artifact in target/. cargo's
    // incremental check uses mtime, so it can think "source is older
    // than build artifact, nothing to do" even when source content
    // changed. We hit this on 2026-04-24 — a Cortex prompt-string edit
    // (commit 32c1184) was correctly committed + checked out but
    // didn't end up in the binary because cargo's freshness check
    // passed. The user-visible effect: ship reports SUCCESS but the
    // change isn't actually deployed.
    //
    // Doing this for the two crates we explicitly build below.
    // Cheap (touch on a few hundred .rs files is sub-second),
    // bulletproof (fresh mtimes guarantee cargo recompiles their
    // compilation units), preserves the dep cache (axum/tokio/etc.
    // stay built).
    if !ctx.opts.dry_run {
        for crate_rel in ["syntaur-gateway", "mace"] {
            let crate_path = ctx.cfg.workspace.join(crate_rel);
            if !crate_path.is_dir() {
                continue;
            }
            log::info!(">> touching .rs files under {}", crate_path.display());
            let touch_status = Command::new("find")
                .arg(&crate_path)
                .args(["-name", "*.rs", "-not", "-path", "*/target/*"])
                .arg("-exec")
                .arg("touch")
                .arg("{}")
                .arg("+")
                .status();
            if let Err(e) = touch_status {
                log::warn!("[build] mtime refresh failed (non-fatal): {e}");
            }
        }
    }

    // Resolve the cargo --features list from (in priority order):
    //   1. SYNTAUR_BUILD_FEATURES env (CSV)
    //   2. ~/.syntaur/ship/features.txt (one feature per line, # comments)
    // Empty / missing → no --features flag (public default).
    //
    // This exists because the Peter persona work shipped 2026-04-30 is
    // gated behind `#[cfg(feature = "personal-peter")]` on
    // syntaur-gateway. A bare `cargo build -p syntaur-gateway` excludes
    // PROMPT_PETER + the LOCAL_ONLY_DEFAULTS PETER row, AND triggers the
    // `cfg(not(feature = "personal-peter"))` cleanup branch that
    // ACTIVELY DELETES the main_peter_local row at startup. Sean's prod
    // would lose the persona on first restart. Caught by peer review at
    // ship time before the bad binary landed; this gate prevents the
    // same regression on every future ship.
    let build_features = resolve_build_features(&ctx.cfg);
    let mut build_args: Vec<String> = vec![
        "build".into(), "--release".into(),
        "-p".into(), "syntaur-gateway".into(),
        "-p".into(), "mace".into(),
    ];
    if !build_features.is_empty() {
        build_args.push("--features".into());
        build_args.push(build_features.clone());
        log::info!(
            ">> cd {ws} && cargo build --release -p syntaur-gateway -p mace --features {build_features}"
        );
    } else {
        log::info!(">> cd {ws} && cargo build --release -p syntaur-gateway -p mace");
    }
    if !ctx.opts.dry_run {
        let status = Command::new(&cargo)
            .args(&build_args)
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

/// Resolve cargo --features CSV from env override + local config file.
/// Empty string means "build with default feature set" (public Syntaur).
fn resolve_build_features(cfg: &crate::config::Config) -> String {
    if let Ok(v) = std::env::var("SYNTAUR_BUILD_FEATURES") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    let features_file = cfg.state_dir.join("features.txt");
    let Ok(text) = std::fs::read_to_string(&features_file) else {
        return String::new();
    };
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(",")
}
