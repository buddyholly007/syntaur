//! Pipeline orchestrator. Runs stages in order, aborts on first failure.
//!
//! Phase 1 wires the existing deploy.sh stages behind the same stage
//! interface later phases will extend. The `run_full` function is the
//! no-flag default — equivalent to `./deploy.sh`.
//!
//! Later phases slot in additional stages (snapshot, version sweep,
//! canary, win11) by extending the vector in `run_full`. The stage
//! order is: preflight → snapshot → build → mac_mini → git_push →
//! truenas → canary → version_audit → viewer → win11 → journal.
//!
//! Every stage implements the same contract: given an `&Context` and
//! the `RunOptions`, it returns `Ok(())` on success or `Err(anyhow)`
//! on failure (which aborts the whole pipeline).

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

    // Phase 1 stages (parity with deploy.sh):
    //   preflight → build → mac_mini → git_push → truenas → viewer.
    // Phase 2 inserts snapshot before mac_mini; Phase 3 inserts
    // version_audit after truenas; Phase 4 appends journal.

    stages::preflight::run(&ctx)?;
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

    if !opts.dry_run {
        let stamp = build_stamp(cfg, opts)?;
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
        mace_sha256: if mace_bin.exists() {
            Some(state::sha256_file(&mace_bin)?)
        } else { None },
        social_manager_sha256: if Path::new(&sm_bin).exists() {
            Some(state::sha256_file(&sm_bin)?)
        } else { None },
        pre_deploy_snapshot: None, // set by Phase 2 snapshot stage
        deploy_session: cfg.coord_session.clone(),
        skip_flags,
    })
}

fn read_version_file(ws: &std::path::Path) -> Result<String> {
    let p = ws.join("VERSION");
    let s = std::fs::read_to_string(&p)
        .map_err(|e| anyhow::anyhow!("read {}: {}", p.display(), e))?;
    Ok(s.trim().to_string())
}

/// Run a command, capture stdout as string.
pub fn run_capture(prog: &str, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("spawn {prog}: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "{} {:?} exited {} — stderr: {}",
            prog,
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── stub entry points filled in by later phases ────────────────────────

pub fn run_status(_cfg: &Config) -> Result<()> {
    println!("status: not yet implemented (Phase 4)");
    Ok(())
}

pub fn run_rollback(_cfg: &Config, _zfs: Option<&str>) -> Result<()> {
    anyhow::bail!("rollback: not yet implemented (Phase 2)")
}

pub fn run_release(_cfg: &Config, _version: &str) -> Result<()> {
    anyhow::bail!("release: not yet implemented (Phase 6)")
}

pub fn run_refresh_windows(_cfg: &Config) -> Result<()> {
    anyhow::bail!("refresh-windows: not yet implemented (Phase 6)")
}

pub fn run_version_sweep(_cfg: &Config) -> Result<()> {
    anyhow::bail!("version-sweep: not yet implemented (Phase 3)")
}

pub fn run_snapshot_list(_cfg: &Config) -> Result<()> {
    anyhow::bail!("snapshot-list: not yet implemented (Phase 2)")
}

pub fn run_journal(_cfg: &Config, _last: usize) -> Result<()> {
    anyhow::bail!("journal: not yet implemented (Phase 4)")
}

pub fn run_verify_stamp(_cfg: &Config, _path: Option<&str>) -> Result<()> {
    anyhow::bail!("verify-stamp: not yet implemented (Phase 7)")
}
