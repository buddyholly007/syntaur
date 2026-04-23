//! Pipeline orchestrator (Phase 2 — adds snapshot + rollback wiring).
//!
//! Stage order for `run_full`:
//!   preflight → snapshot (NEW) → build → mac_mini → git_push →
//!   truenas (now with .prev + auto-rollback) → viewer.

use anyhow::Result;
use chrono::Utc;

use crate::config::Config;
use crate::stages;
use crate::state::{self, DeployStamp};

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub dry_run: bool,
    pub skip_build: bool,
    pub skip_mac: bool,
    pub skip_git: bool,
    pub social_only: bool,
}

pub struct StageContext<'a> {
    pub cfg: &'a Config,
    pub opts: &'a RunOptions,
}

pub fn run_full(cfg: &Config, opts: &RunOptions) -> Result<()> {
    log::info!(
        "=== syntaur-ship pipeline starting (dry_run={} skip_build={} skip_mac={} skip_git={} social_only={}) ===",
        opts.dry_run, opts.skip_build, opts.skip_mac, opts.skip_git, opts.social_only,
    );
    let ctx = StageContext { cfg, opts };

    stages::preflight::run(&ctx)?;
    // Phase 3a: version sweep BEFORE build — abort deploy if the 5
    // public version surfaces disagree. Cheap local file reads; no
    // network. Fix at source + re-run rather than shipping drift.
    stages::version_sweep::run(&ctx)?;
    // Phase 2: snapshot BEFORE any TrueNAS writes. If any later stage
    // fails we still have a restore point.
    let snapshot_name = stages::snapshot::run(&ctx)?;
    if !opts.skip_build {
        stages::build::run(&ctx)?;
    } else {
        log::warn!("[preflight] --skip-build set; reusing existing target/release binaries");
    }
    if !opts.social_only {
        if !opts.skip_mac {
            stages::mac_mini::run(&ctx)?;
        } else {
            log::warn!("[mac_mini] --skip-mac set; skipping smoke (emergency only)");
        }
        if !opts.skip_git {
            stages::git_push::run(&ctx)?;
        } else {
            log::warn!("[git_push] --skip-git set; not propagating to origin");
        }
    }
    stages::truenas::run(&ctx)?;
    if !opts.social_only {
        stages::viewer::run(&ctx)?;
    }
    // Phase 3a: post-deploy version audit on live prod. Warns (doesn't
    // abort) — prod is already live at this point; drift here means
    // repair at source + redeploy, not roll back.
    let _ = stages::version_audit::run(&ctx);

    if !opts.dry_run {
        let mut stamp = build_stamp(cfg, opts)?;
        stamp.pre_deploy_snapshot = Some(snapshot_name);
        state::write_stamp(&cfg.state_dir, &stamp)?;
        log::info!(
            "✓ deploy stamp written: version={} git_head={} gateway_sha={}",
            stamp.version,
            &stamp.git_head[..stamp.git_head.len().min(10)],
            &stamp.gateway_sha256[..10]
        );
    }

    println!();
    println!("deploy complete: {}", cfg.health_url);
    Ok(())
}

fn build_stamp(cfg: &Config, opts: &RunOptions) -> Result<DeployStamp> {
    use std::path::Path;
    let ws = &cfg.workspace;
    let gateway_bin = ws.join("target/release/syntaur-gateway");
    let mace_bin = ws.join("target/release/mace");
    let sm_bin = cfg
        .social_manager
        .join("target/release/rust-social-manager");

    let git_head = run_capture("git", &["-C", ws.to_str().unwrap(), "rev-parse", "HEAD"])?
        .trim()
        .to_string();
    let version = read_version_file(ws)?;

    let mut skip_flags = Vec::new();
    if opts.skip_build { skip_flags.push("skip-build".into()); }
    if opts.skip_mac { skip_flags.push("skip-mac".into()); }
    if opts.skip_git { skip_flags.push("skip-git".into()); }
    if opts.social_only { skip_flags.push("social-only".into()); }

    Ok(DeployStamp {
        deployed_at: Utc::now(),
        git_head,
        version,
        gateway_sha256: state::sha256_file(&gateway_bin)?,
        mace_sha256: if mace_bin.exists() { Some(state::sha256_file(&mace_bin)?) } else { None },
        social_manager_sha256: if Path::new(&sm_bin).exists() {
            Some(state::sha256_file(&sm_bin)?)
        } else { None },
        pre_deploy_snapshot: None,
        deploy_session: cfg.coord_session.clone(),
        skip_flags,
    })
}

fn read_version_file(ws: &std::path::Path) -> Result<String> {
    let p = ws.join("VERSION");
    Ok(std::fs::read_to_string(&p)
        .map_err(|e| anyhow::anyhow!("read {}: {}", p.display(), e))?
        .trim()
        .to_string())
}

pub fn run_capture(prog: &str, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("spawn {prog}: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "{} {:?} exited {} — stderr: {}",
            prog, args, out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── Phase 2 subcommands ────────────────────────────────────────────────

pub fn run_rollback(cfg: &Config, zfs: Option<&str>) -> Result<()> {
    let opts = RunOptions::default();
    let ctx = StageContext { cfg, opts: &opts };
    match zfs {
        Some(snap) => {
            log::warn!("ZFS rollback requested: {snap}");
            stages::snapshot::rollback(&ctx, snap)?;
            // Restart the container after ZFS rollback since the
            // binary on disk may have changed state.
            log::info!(">> docker restart syntaur after ZFS rollback");
            let mut args = cfg.truenas_ssh_args();
            args.push("docker restart syntaur".into());
            std::process::Command::new("ssh").args(&args).status()?;
        }
        None => {
            log::info!("Binary rollback: restoring latest .prev-* for each binary on TrueNAS");
            stages::truenas::manual_binary_rollback(&ctx)?;
        }
    }
    Ok(())
}

pub fn run_snapshot_list(cfg: &Config) -> Result<()> {
    let opts = RunOptions::default();
    let ctx = StageContext { cfg, opts: &opts };
    println!("=== ZFS snapshots (syntaur-ship pre-deploy) ===");
    let snaps = stages::snapshot::list(&ctx)?;
    if snaps.is_empty() {
        println!("   (none)");
    } else {
        for s in &snaps {
            println!("   {s}");
        }
    }
    println!();
    println!("=== .prev binaries on TrueNAS ===");
    let prevs = stages::truenas::list_prev_binaries(&ctx)?;
    if prevs.is_empty() {
        println!("   (none)");
    } else {
        for p in &prevs {
            println!("   {p}");
        }
    }
    Ok(())
}

// ── Later-phase stubs ──────────────────────────────────────────────────

pub fn run_status(_cfg: &Config) -> Result<()> {
    println!("status: not yet implemented (Phase 4)");
    Ok(())
}

pub fn run_release(_cfg: &Config, _version: &str) -> Result<()> {
    anyhow::bail!("release: not yet implemented (Phase 6)")
}

pub fn run_refresh_windows(_cfg: &Config) -> Result<()> {
    anyhow::bail!("refresh-windows: not yet implemented (Phase 6)")
}

pub fn run_version_sweep(cfg: &Config) -> Result<()> {
    let opts = RunOptions::default();
    let ctx = StageContext { cfg, opts: &opts };
    stages::version_sweep::run(&ctx)?;
    // Also hit the live prod surfaces (non-fatal — informational).
    let audit_opts = RunOptions::default();
    let audit_ctx = StageContext { cfg, opts: &audit_opts };
    let _ = stages::version_audit::run(&audit_ctx);
    Ok(())
}

pub fn run_journal(_cfg: &Config, _last: usize) -> Result<()> {
    anyhow::bail!("journal: not yet implemented (Phase 4)")
}

pub fn run_verify_stamp(_cfg: &Config, _path: Option<&str>) -> Result<()> {
    anyhow::bail!("verify-stamp: not yet implemented (Phase 7)")
}
