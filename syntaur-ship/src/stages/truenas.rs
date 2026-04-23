//! TrueNAS stage — rsync binaries to bind-mount dir via jump host,
//! then `docker restart syntaur`, then wait for prod /health.
//!
//! Phase 1 ports deploy.sh lines 190-241. Phase 2 wraps this with the
//! ZFS snapshot + .prev retention + auto-rollback layer.

use anyhow::{Context, Result};
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    let cfg = ctx.cfg;
    let ws = &cfg.workspace;
    let sm = &cfg.social_manager;

    if !ctx.opts.social_only {
        push_to_truenas(cfg, &ws.join("target/release/syntaur-gateway"), "rust-openclaw", ctx.opts.dry_run)?;
        let mace_bin = ws.join("target/release/mace");
        if mace_bin.exists() {
            push_to_truenas(cfg, &mace_bin, "mace", ctx.opts.dry_run)?;
        }
        // Auxiliary: keep a local copy on openclawprod/gaming-pc for mace
        // CLI use. Non-fatal — wrap in `|| true` behavior.
        if !ctx.opts.dry_run {
            let _ = Command::new("sh").args(["-c",
                &format!(
                    "ssh sean@192.168.1.35 'mkdir -p $HOME/bin' && rsync -az {src} sean@192.168.1.35:$HOME/bin/mace",
                    src = mace_bin.display()
                )
            ]).status();
            let _ = Command::new("sh").args(["-c",
                &format!(
                    "ssh {viewer} 'mkdir -p $HOME/bin' && rsync -az {src} {viewer}:$HOME/bin/mace",
                    viewer = cfg.viewer_host, src = mace_bin.display()
                )
            ]).status();
        }
    }

    let sm_bin = sm.join("target/release/rust-social-manager");
    if sm_bin.exists() {
        push_to_truenas(cfg, &sm_bin, "rust-social-manager", ctx.opts.dry_run)?;
    }

    log::info!(">> docker restart syntaur on {}", cfg.truenas_ip);
    if !ctx.opts.dry_run {
        let mut args = vec!["-J".to_string(), cfg.truenas_jump.clone()];
        args.push(format!("{}@{}", cfg.truenas_user, cfg.truenas_ip));
        args.push("docker restart syntaur".into());
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let status = Command::new("ssh").args(&args_refs).status().context("docker restart")?;
        if !status.success() {
            anyhow::bail!("docker restart syntaur exited {status}");
        }
    }

    log::info!(">> waiting for prod /health");
    if !ctx.opts.dry_run {
        for i in 1..=20 {
            let out = Command::new("curl")
                .args(["-sf", "--max-time", "3", &cfg.health_url])
                .output();
            if let Ok(o) = out {
                if o.status.success() && !o.stdout.is_empty() {
                    log::info!("   prod /health OK after ~{}s", i - 1);
                    return Ok(());
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        anyhow::bail!("prod /health unreachable after 20s — TrueNAS container issue; rollback pending Phase 2");
    }
    Ok(())
}

fn push_to_truenas(
    cfg: &crate::config::Config,
    src: &std::path::Path,
    dst_name: &str,
    dry: bool,
) -> Result<()> {
    let dst = format!(
        "{}@{}:{}/{}",
        cfg.truenas_user, cfg.truenas_ip, cfg.bin_dir, dst_name
    );
    log::info!(">> rsync {} → {}", src.display(), dst);
    if dry {
        return Ok(());
    }
    // The jump-host SSH shape that deploy.sh uses: -e 'ssh -J <jump>'.
    let status = Command::new("rsync")
        .args([
            "-az",
            "-e",
            &cfg.truenas_rsync_ssh(),
            src.to_str().unwrap(),
            &dst,
        ])
        .status()
        .context("rsync to truenas")?;
    if !status.success() {
        anyhow::bail!("rsync {dst_name} to TrueNAS exited {status}");
    }
    Ok(())
}
