//! TrueNAS stage with Phase 2 preservation layer.
//!
//! Flow per deploy:
//!   1. Scope whitelist — hard-enforce that every rsync destination
//!      path is inside cfg.bin_dir. Any violation = immediate abort.
//!      Prevents the class of bugs where a typo'd dst path could
//!      overwrite a family photo.
//!   2. For each of the 3 binaries:
//!      - `cp <current> <current>.prev-<timestamp>` (last 3 retained)
//!      - rsync new binary over top
//!   3. `docker restart syntaur`
//!   4. Retry prod /health up to 20s.
//!   5. If /health never comes back, **AUTO-ROLLBACK**: mv .prev-<latest>
//!      back to <current>, docker restart, verify again.
//!
//! The ZFS snapshot (from stages::snapshot) runs BEFORE this stage,
//! giving us a second rollback layer for data corruption scenarios
//! that .prev binaries can't fix.

use anyhow::{Context, Result};
use chrono::Utc;
use std::process::Command;

use crate::pipeline::StageContext;

pub const RETAIN_PREV: usize = 3;

/// Binaries deployed to TrueNAS. Each gets its own .prev retention.
pub const BINARIES: &[(&str, &str, bool)] = &[
    // (local-path-relative-to-workspace, truenas-name, from-social-manager-repo)
    ("target/release/syntaur-gateway", "rust-openclaw", false),
    ("target/release/mace", "mace", false),
    ("target/release/rust-social-manager", "rust-social-manager", true),
];

pub fn run(ctx: &StageContext) -> Result<()> {
    let cfg = ctx.cfg;
    let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();

    for (local_rel, dst_name, from_sm) in BINARIES {
        if ctx.opts.social_only && !*from_sm {
            continue;
        }
        let src_root = if *from_sm { &cfg.social_manager } else { &cfg.workspace };
        let src = src_root.join(local_rel);
        if !src.exists() {
            log::warn!("[truenas] skipping {dst_name} — local {} missing", src.display());
            continue;
        }
        // Scope whitelist enforcement.
        let dst_path = format!("{}/{}", cfg.bin_dir, dst_name);
        enforce_scope(cfg, &dst_path)?;

        // Stash .prev before overwrite (so rollback is instant).
        stash_prev(ctx, dst_name, &ts)?;

        push_to_truenas(cfg, &src, dst_name, ctx.opts.dry_run)?;
    }

    if !ctx.opts.dry_run {
        prune_old_prev(ctx)?;
    }

    // Deploy the binaries to auxiliary hosts (non-fatal).
    let mace_bin = cfg.workspace.join("target/release/mace");
    if !ctx.opts.social_only && mace_bin.exists() && !ctx.opts.dry_run {
        for host in ["sean@192.168.1.35", "sean@192.168.1.69"] {
            let _ = Command::new("sh")
                .args([
                    "-c",
                    &format!(
                        "ssh {host} 'mkdir -p $HOME/bin' && rsync -az {src} {host}:$HOME/bin/mace",
                        src = mace_bin.display()
                    ),
                ])
                .status();
        }
    }

    log::info!(">> docker restart syntaur on {}", cfg.truenas_ip);
    if !ctx.opts.dry_run {
        docker_restart(cfg)?;
    }

    log::info!(">> waiting for prod /health");
    if !ctx.opts.dry_run {
        if !health_loop(cfg, 20) {
            log::error!("!! prod /health unreachable after 20s — attempting auto-rollback");
            try_auto_rollback(ctx, &ts)?;
            anyhow::bail!("prod /health never came up; auto-rollback executed — investigate Mac Mini + TrueNAS logs");
        }
    }
    Ok(())
}

/// Refuse any destination outside cfg.bin_dir. This is the firewall
/// that prevents a typo from overwriting user data.
fn enforce_scope(cfg: &crate::config::Config, dst_path: &str) -> Result<()> {
    let bin_dir = cfg.bin_dir.trim_end_matches('/');
    if !dst_path.starts_with(&format!("{bin_dir}/")) {
        anyhow::bail!(
            "SCOPE VIOLATION: destination {dst_path} is outside whitelist {bin_dir}/ — refusing to touch TrueNAS. \
             This is the guard that prevents overwriting user files; if you need to write elsewhere, do it by hand, not through syntaur-ship."
        );
    }
    // Prohibit any `--delete` or `..` that could escape.
    if dst_path.contains("..") || dst_path.contains("--delete") {
        anyhow::bail!("SCOPE VIOLATION: suspect tokens in {dst_path}");
    }
    Ok(())
}

fn stash_prev(ctx: &StageContext, dst_name: &str, ts: &str) -> Result<()> {
    let cfg = ctx.cfg;
    let remote_dst = format!("{}/{}", cfg.bin_dir, dst_name);
    let remote_prev = format!("{remote_dst}.prev-{ts}");
    // `cp -a` preserves mode/times — and SKIP if the current binary
    // doesn't exist (fresh deploy scenario).
    let script = format!(
        "if [ -e {remote_dst:?} ]; then cp -a {remote_dst:?} {remote_prev:?} && echo stashed; else echo no-prior; fi"
    );
    log::info!(">> TrueNAS: stash .prev for {dst_name}");
    if ctx.opts.dry_run {
        return Ok(());
    }
    let mut args = cfg.truenas_ssh_args();
    args.push(script);
    let out = Command::new("ssh").args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "stash .prev for {dst_name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Log the remote script's decision so auto-rollback readers can
    // tell which binaries have .prev stashes and which don't.
    let remote_out = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match remote_out.as_str() {
        "stashed" => log::info!("   ↳ stashed {dst_name}.prev-{ts}"),
        "no-prior" => log::info!("   ↳ no prior {dst_name} — no rollback target for this binary"),
        other => log::warn!("   ↳ unexpected stash output for {dst_name}: {other}"),
    }
    Ok(())
}

fn prune_old_prev(ctx: &StageContext) -> Result<()> {
    let cfg = ctx.cfg;
    // For each binary, list .prev-* sorted ascending, delete all but last RETAIN_PREV.
    let mut script = String::new();
    for (_, dst_name, _) in BINARIES {
        script.push_str(&format!(
            r#"cd {bindir} && ls -1 {dst}.prev-* 2>/dev/null | sort | head -n -{retain} | xargs -r rm -v; "#,
            bindir = cfg.bin_dir,
            dst = dst_name,
            retain = RETAIN_PREV
        ));
    }
    let mut args = cfg.truenas_ssh_args();
    args.push(script);
    let out = Command::new("ssh").args(&args).output()?;
    if !out.status.success() {
        log::warn!(
            "[truenas] .prev prune had issues (non-fatal): {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
    // NOTE: no `--delete` flag — Phase 2 contract.
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

fn docker_restart(cfg: &crate::config::Config) -> Result<()> {
    let mut args = cfg.truenas_ssh_args();
    args.push("docker restart syntaur".into());
    let status = Command::new("ssh").args(&args).status()?;
    if !status.success() {
        anyhow::bail!("docker restart syntaur exited {status}");
    }
    Ok(())
}

fn health_loop(cfg: &crate::config::Config, max_secs: u64) -> bool {
    // claudevm has no direct route to TrueNAS (.239) — LAN-segmented
    // by design. All /health probes hop through the gaming-PC jump
    // host SSH, which has the route. (False-positive auto-rollback
    // 2026-04-23 was this code using direct curl — matching the fix
    // already applied in version_audit.rs + pipeline.rs::run_status.)
    for _ in 0..max_secs {
        let ssh_cmd = format!("curl -sf --max-time 3 {}", cfg.health_url);
        let mut args = cfg.truenas_ssh_args();
        args.push(ssh_cmd);
        let out = Command::new("ssh").args(&args).output();
        if let Ok(o) = out {
            if o.status.success() && !o.stdout.is_empty() {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    false
}

fn try_auto_rollback(ctx: &StageContext, ts: &str) -> Result<()> {
    log::warn!(">> AUTO-ROLLBACK: restoring .prev-{ts} binaries + docker restart");
    let cfg = ctx.cfg;
    // Restore each binary from its .prev-<ts> stash. If the stash is
    // missing (e.g. this was a fresh deploy and there was no prior
    // binary), leave the current in place and log.
    let mut script = String::new();
    for (_, dst_name, _) in BINARIES {
        let dst = format!("{}/{}", cfg.bin_dir, dst_name);
        let prev = format!("{dst}.prev-{ts}");
        script.push_str(&format!(
            r#"if [ -e {prev:?} ]; then cp -a {prev:?} {dst:?} && echo restored-{dst_name}; fi; "#,
            dst_name = dst_name
        ));
    }
    let mut args = cfg.truenas_ssh_args();
    args.push(script);
    let out = Command::new("ssh").args(&args).output()?;
    let stdout_str = String::from_utf8_lossy(&out.stdout);
    log::info!("   rollback: {}", stdout_str.trim().replace('\n', " | "));

    docker_restart(cfg)?;
    if !health_loop(cfg, 30) {
        log::error!("!! rollback did not restore /health — MANUAL INTERVENTION REQUIRED. Consider `syntaur-ship rollback --zfs <snapshot>`.");
    } else {
        log::info!("   ✓ rollback restored prod /health");
    }
    Ok(())
}

/// Invoked by `syntaur-ship rollback` (no --zfs). Uses the latest
/// .prev-<ts> for each binary.
pub fn manual_binary_rollback(ctx: &StageContext) -> Result<()> {
    let cfg = ctx.cfg;
    let mut script = String::new();
    for (_, dst_name, _) in BINARIES {
        let dst = format!("{}/{}", cfg.bin_dir, dst_name);
        script.push_str(&format!(
            r#"latest=$(ls -1 {dst}.prev-* 2>/dev/null | sort | tail -1); \
               if [ -n "$latest" ]; then cp -a "$latest" {dst:?} && echo "restored {dst_name} from $latest"; \
               else echo "no .prev for {dst_name}, skipped"; fi; "#,
            dst_name = dst_name
        ));
    }
    let mut args = cfg.truenas_ssh_args();
    args.push(script);
    let out = Command::new("ssh").args(&args).output()?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if !out.status.success() {
        anyhow::bail!(
            "manual rollback ssh: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    docker_restart(cfg)?;
    if health_loop(cfg, 30) {
        log::info!("✓ prod /health OK after manual rollback");
        Ok(())
    } else {
        anyhow::bail!("manual rollback: /health did not come back; try --zfs <snap>");
    }
}

/// List .prev binaries on TrueNAS for `syntaur-ship snapshot-list`.
pub fn list_prev_binaries(ctx: &StageContext) -> Result<Vec<String>> {
    let cfg = ctx.cfg;
    let script = format!("ls -1 {}/*.prev-* 2>/dev/null || true", cfg.bin_dir);
    let mut args = cfg.truenas_ssh_args();
    args.push(script);
    let out = Command::new("ssh").args(&args).output()?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}
