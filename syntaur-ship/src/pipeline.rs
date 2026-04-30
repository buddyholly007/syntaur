//! Pipeline orchestrator (Phase 2 — adds snapshot + rollback wiring).
//!
//! Stage order for `run_full`:
//!   preflight → snapshot (NEW) → build → mac_mini → git_push →
//!   truenas (now with .prev + auto-rollback) → viewer.

use anyhow::{Context, Result};
use chrono::Utc;
use std::time::Instant;

use crate::config::Config;
use crate::coord::{self, LockResult};
use crate::guards;
use crate::journal::{self, JournalEntry};
use crate::stages;
use crate::state::{self, DeployStamp};

/// Broker file lock is scoped to the file every commit touches —
/// Cargo.lock. Any session editing that file while we hold the lock
/// gets blocked by the broker's PreToolUse hook.
const LOCK_FILE: &str = "openclaw-workspace/Cargo.lock";
/// Phase 5: deploys typically take 5-15 min; 30min TTL is generous.
const LOCK_TTL_SECS: i64 = 1800;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub dry_run: bool,
    /// Deploy only `rust-social-manager`, leaving the gateway running.
    /// Partial-deploy SCOPE, not a quality bypass — every stage that
    /// runs runs in full.
    pub social_only: bool,
    /// When Opus catches a regression during verify, let it propose
    /// edits, rebuild the gateway, reload Mac Mini, and re-verify
    /// (capped at 2 iters, ≤150 LoC/module per iteration). Off by
    /// default — opt-in via `--auto-fix` so a pipeline operator is
    /// always in the loop when unattended code gets written to source.
    pub auto_fix: bool,
}

pub struct StageContext<'a> {
    pub cfg: &'a Config,
    pub opts: &'a RunOptions,
}

pub fn run_full(cfg: &Config, opts: &RunOptions) -> Result<()> {
    let start = Instant::now();
    log::info!(
        "=== syntaur-ship pipeline starting (dry_run={} social_only={} auto_fix={}) ===",
        opts.dry_run, opts.social_only, opts.auto_fix,
    );

    // Phase 5: PID guard + broker lock + CI gate. Acquired BEFORE any
    // work starts; released on drop of _pid / via `defer_release` at
    // the end of this function.
    let _pid = if !opts.dry_run {
        Some(guards::PidLock::try_acquire(&cfg.state_dir)?)
    } else {
        None
    };

    // Broker lock on Cargo.lock — blocks concurrent Edits by other
    // sessions for the deploy's duration.
    let mut lock_acquired = false;
    if !opts.dry_run {
        match coord::try_lock(
            cfg,
            LOCK_FILE,
            &format!(
                "syntaur-ship deploy by {}",
                cfg.coord_session
            ),
            LOCK_TTL_SECS,
        )? {
            LockResult::Acquired { ttl_secs } => {
                log::info!("[coord-lock] acquired {LOCK_FILE} (TTL {ttl_secs}s)");
                lock_acquired = true;
            }
            LockResult::HeldByOther { holder, intent, expires_in_secs } => {
                anyhow::bail!(
                    "coord lock on {LOCK_FILE} held by session '{holder}' ({intent}); expires in {expires_in_secs}s. \
                     Wait for that session to finish or coordinate with them."
                );
            }
            LockResult::BrokerUnavailable => {
                log::warn!("[coord-lock] broker unreachable; proceeding without lock (degraded)");
            }
        }
    }

    // Pre-deploy BLOCKING CI gate: refuse to deploy if any workflow on
    // the current HEAD is failing. v0.6.5: --force-ci-drift is GONE.
    // Every prior emergency that needed it turned out to be either a
    // bug we should have fixed inline OR a transient we should have
    // retried. Runs in dry-run too so `syntaur-ship check` catches CI
    // drift before the real run.
    // Critical: if git rev-parse HEAD fails, an empty string flows
    // into ci_audit::run, which silently reports "no failures" because
    // the GitHub API returns no runs for an empty SHA. That bypasses
    // the CI gate entirely. Must be fatal — there is no safe fallback.
    let head = run_capture(
        "git",
        &["-C", cfg.workspace.to_str().unwrap(), "rev-parse", "HEAD"],
    )
    .context("ci-gate: failed to read HEAD via `git rev-parse`. Without a HEAD, the CI audit is meaningless. Fix the git environment and re-run.")?
    .trim()
    .to_string();
    if head.is_empty() {
        anyhow::bail!("ci-gate: `git rev-parse HEAD` returned empty output");
    }
    let pre_failures = crate::ci_audit::run(&head);
    let blocking_failures = crate::ci_audit::blocking(&pre_failures);
    crate::ci_audit::log_failures(&pre_failures, &head[..head.len().min(10)]);
    if !blocking_failures.is_empty() && !opts.dry_run {
        let list = blocking_failures
            .iter()
            .map(|f| format!("  ✗ {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "CI gate: {} deploy-gating workflow(s) failing on HEAD {}:\n{list}\n\nFix the underlying issue, then re-run. \
             (--force-ci-drift was removed in v0.6.5: every prior 'emergency' override masked a real bug. \
             To change which workflows are deploy-gating, edit ci_audit::DEPLOY_GATING_WORKFLOWS.)",
            blocking_failures.len(),
            &head[..head.len().min(10)]
        );
    }

    let ctx = StageContext { cfg, opts };

    if !opts.dry_run {
        let git_head = run_capture("git", &["-C", cfg.workspace.to_str().unwrap(), "rev-parse", "--short", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let _ = coord::broadcast_intent(
            cfg,
            &format!(
                "syntaur-ship deploy starting — HEAD={} session={}. ETA 10min. Other sessions should hold git pushes to openclaw-workspace.",
                git_head, cfg.coord_session
            ),
        );
    }

    // Wrap the whole pipeline in a closure so we can emit a journal
    // entry + broker notification on BOTH success and failure.
    // Threaded `snapshot_holder`: written by run_full_inner the moment
    // the ZFS snapshot is created, so the failure-path journal entry
    // can record the recovery point even if a later stage aborts.
    let mut snapshot_holder: Option<String> = None;
    let result = run_full_inner(cfg, opts, &ctx, &mut snapshot_holder);
    let duration_ms = start.elapsed().as_millis();

    // Write journal entry regardless of outcome (unless dry-run).
    if !opts.dry_run {
        let outcome = if result.is_ok() { "success" } else { "aborted" };
        let (failed_stage, failure_reason) = match &result {
            Ok(()) => (None, None),
            Err(e) => {
                // Outermost anyhow context is the .context("<stage>")
                // applied at the call site in run_full_inner. The full
                // chain is preserved in failure_reason via {:#}.
                let stage = e.to_string();
                (Some(stage), Some(format!("{e:#}")))
            }
        };
        // Read back the stamp we wrote (on success path) for the journal.
        let stamp_opt = state::read_stamp(&cfg.state_dir).unwrap_or(None);
        let entry = match stamp_opt {
            Some(s) if result.is_ok() => JournalEntry {
                timestamp: s.deployed_at,
                outcome: outcome.into(),
                version: s.version,
                git_head: s.git_head,
                gateway_sha256: Some(s.gateway_sha256),
                pre_deploy_snapshot: s.pre_deploy_snapshot,
                deploy_session: s.deploy_session,
                skip_flags: s.skip_flags,
                failed_stage,
                failure_reason,
                duration_ms,
            },
            _ => {
                // Failure path — build a minimal entry.
                let git_head = run_capture("git", &["-C", cfg.workspace.to_str().unwrap(), "rev-parse", "HEAD"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let version = read_version_file(&cfg.workspace).unwrap_or_default();
                JournalEntry {
                    timestamp: Utc::now(),
                    outcome: outcome.into(),
                    version,
                    git_head,
                    gateway_sha256: None,
                    pre_deploy_snapshot: snapshot_holder.clone(),
                    deploy_session: cfg.coord_session.clone(),
                    skip_flags: collect_skip_flags(opts),
                    failed_stage,
                    failure_reason,
                    duration_ms,
                }
            }
        };
        if let Err(e) = journal::append(&cfg.vault_dir, &entry) {
            log::warn!("[journal] append failed: {e}");
        }
        let msg = match &result {
            Ok(()) => format!(
                "✓ syntaur-ship: v{} deployed to prod in {:.1}s (session {})",
                entry.version, duration_ms as f64 / 1000.0, cfg.coord_session
            ),
            Err(e) => format!(
                "✗ syntaur-ship: deploy ABORTED after {:.1}s — {}",
                duration_ms as f64 / 1000.0,
                format!("{e:#}").chars().take(200).collect::<String>()
            ),
        };
        let _ = coord::broadcast_info(cfg, &msg);
    }

    // Phase 5: release the broker lock on completion (success or
    // failure). `_pid` drops here too, removing the PID file.
    if lock_acquired {
        let _ = coord::release_lock(cfg, LOCK_FILE);
        log::debug!("[coord-lock] released {LOCK_FILE}");
    }

    result
}

fn run_full_inner(
    cfg: &Config,
    opts: &RunOptions,
    ctx: &StageContext,
    snapshot_out: &mut Option<String>,
) -> Result<()> {
    // Each .context("<stage>") makes the stage name the outermost
    // anyhow message, so on failure run_full can extract failed_stage
    // by reading e.to_string(). Replaces the prior hardcoded "unknown".
    stages::preflight::run(ctx).context("preflight")?;
    stages::review_triage::run(ctx).context("review_triage")?;
    stages::version_sweep::run(ctx).context("version_sweep")?;
    stages::doc_audit::run(ctx).context("doc_audit")?;
    stages::backup_freshness::run(ctx).context("backup_freshness")?;
    let snapshot_name = stages::snapshot::run(ctx).context("snapshot")?;
    // Record snapshot for the journal/rollback path BEFORE any later
    // stage can fail — so an aborted deploy still surfaces the recovery
    // point.
    *snapshot_out = Some(snapshot_name.clone());
    stages::build::run(ctx).context("build")?;
    if !opts.social_only {
        stages::mac_mini::run(ctx).context("mac_mini")?;
        stages::canary::run(ctx).context("canary")?;
        stages::verify::run(ctx).context("verify")?;
        stages::git_push::run(ctx).context("git_push")?;
    }
    stages::truenas::run(ctx).context("truenas")?;
    if !opts.social_only {
        stages::viewer::run(ctx).context("viewer")?;
    }
    // v0.6.5: version_audit promoted from warn-only to fatal.
    stages::version_audit::run(ctx).context("version_audit")?;

    // Post-deploy CI audit: poll ALL workflows on the current HEAD and
    // surface failures. Replaces the old narrow release-sign-only
    // check. Motivated by the rustls-webpki/cargo-audit miss: the tool
    // was silent about 5+ consecutive cargo-audit failures because it
    // only polled one workflow. Now every red workflow on this HEAD
    // appears in the deploy output, so ending a session with a failing
    // CI is visible instead of hidden.
    let head = run_capture(
        "git",
        &["-C", ctx.cfg.workspace.to_str().unwrap(), "rev-parse", "HEAD"],
    )
    .unwrap_or_default()
    .trim()
    .to_string();
    let ci_failures = crate::ci_audit::run(&head);
    crate::ci_audit::log_failures(&ci_failures, &head[..head.len().min(10)]);

    // Phase 6: refresh the Win11 nightly-tester binary so overnight
    // tests hit the just-deployed version. Non-fatal — prod already up.
    if !opts.social_only {
        let _ = stages::win11::run(ctx);
    }

    if !opts.dry_run {
        let mut stamp = build_stamp(cfg, opts)?;
        stamp.pre_deploy_snapshot = Some(snapshot_name);
        state::write_stamp(&cfg.state_dir, &stamp)?;
        // Phase 7: cosign-sign the stamp if a local key pair exists.
        // First-run setup: `cd ~/.syntaur/ship && cosign generate-key-pair`
        // Non-fatal if cosign is missing or not configured.
        let _ = crate::stamp_sign::sign_stamp(&cfg.state_dir);
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

    // v0.6.5: --skip-build / --skip-mac / --skip-git / --skip-verify /
    // --force-ci-drift were removed. Only social-only is a real "scope"
    // flag (it changes which artifacts ship, not whether stages run).
    let mut skip_flags = Vec::new();
    if opts.social_only { skip_flags.push("social-only".into()); }

    let cargo_lock_sha256 = guards::cargo_lock_sha(&cfg.workspace).ok();
    Ok(DeployStamp {
        deployed_at: Utc::now(),
        git_head,
        version,
        gateway_sha256: state::sha256_file(&gateway_bin)?,
        cargo_lock_sha256,
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
            let status = std::process::Command::new("ssh").args(&args).status()
                .context("spawn ssh for docker restart")?;
            if !status.success() {
                anyhow::bail!(
                    "docker restart syntaur failed on TrueNAS (ssh exit {}); container may be down — verify with `ssh truenas docker ps` before retrying",
                    status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
                );
            }
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

// ── Phase 4 subcommands ────────────────────────────────────────────────

pub fn run_status(cfg: &Config) -> Result<()> {
    println!("=== syntaur-ship status ===\n");

    // Local deploy stamp (last successful deploy).
    match state::read_stamp(&cfg.state_dir)? {
        Some(s) => {
            let age = Utc::now().signed_duration_since(s.deployed_at);
            println!(
                "last successful deploy: v{} git={} @ {} ({} ago)",
                s.version,
                &s.git_head[..s.git_head.len().min(10)],
                s.deployed_at.format("%Y-%m-%d %H:%M UTC"),
                human_duration(age)
            );
            println!("  gateway sha256: {}", &s.gateway_sha256[..16]);
            if let Some(snap) = &s.pre_deploy_snapshot {
                println!("  pre-deploy snapshot: {snap}");
            }
            if !s.skip_flags.is_empty() {
                println!("  skip flags: {}", s.skip_flags.join(", "));
            }
        }
        None => println!("last successful deploy: (none recorded yet)"),
    }

    // Live prod. claudevm has no direct route to TrueNAS (.239) —
    // hop through the gaming-PC jump host SSH and curl from there.
    println!();
    let ssh_cmd = format!("curl -sf --max-time 5 {}", cfg.health_url);
    let mut ssh_args = cfg.truenas_ssh_args();
    ssh_args.push(ssh_cmd);
    let prod_info = std::process::Command::new("ssh").args(&ssh_args).output();
    match prod_info {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                println!(
                    "prod ({}): v{} uptime={}s agents={} providers={}",
                    cfg.health_url,
                    v["version"].as_str().unwrap_or("?"),
                    v["uptime_secs"].as_i64().unwrap_or(0),
                    v["agents"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["providers"].as_array().map(|a| a.len()).unwrap_or(0),
                );
            }
        }
        _ => println!("prod ({}): unreachable (jump-proxied)", cfg.health_url),
    }

    // Local /VERSION for reference.
    if let Ok(v) = std::fs::read_to_string(cfg.workspace.join("VERSION")) {
        println!("local HEAD VERSION: {}", v.trim());
    }

    // Head commit + ahead-of-last-deploy count.
    if let Ok(head) = run_capture(
        "git",
        &["-C", cfg.workspace.to_str().unwrap(), "rev-parse", "--short", "HEAD"],
    ) {
        println!("local HEAD commit:   {}", head.trim());
    }

    // Recent journal tail.
    println!();
    println!("=== recent deploys ===");
    let recents = journal::read_recent(&cfg.vault_dir, 5).unwrap_or_default();
    if recents.is_empty() {
        println!("  (no journal entries yet)");
    } else {
        for e in &recents {
            println!(
                "  {} {:<8} v{} {} {}ms{}",
                e.timestamp.format("%Y-%m-%d %H:%M UTC"),
                e.outcome,
                e.version,
                &e.git_head[..e.git_head.len().min(10)],
                e.duration_ms,
                if e.skip_flags.is_empty() { String::new() } else { format!(" [{}]", e.skip_flags.join(",")) },
            );
        }
    }

    Ok(())
}

fn collect_skip_flags(opts: &RunOptions) -> Vec<String> {
    let mut v = Vec::new();
    if opts.social_only { v.push("social-only".into()); }
    v
}

fn human_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

pub fn run_release(cfg: &Config, version: &str) -> Result<()> {
    crate::release::run(cfg, version)
}

pub fn run_refresh_windows(cfg: &Config) -> Result<()> {
    let opts = RunOptions::default();
    let ctx = StageContext { cfg, opts: &opts };
    stages::win11::run(&ctx)
}

pub fn run_version_sweep(cfg: &Config) -> Result<()> {
    let opts = RunOptions::default();
    let ctx = StageContext { cfg, opts: &opts };
    stages::version_sweep::run(&ctx)?;
    let _ = stages::version_audit::run(&ctx);
    Ok(())
}

pub fn run_journal(cfg: &Config, last: usize) -> Result<()> {
    let entries = journal::read_recent(&cfg.vault_dir, last)?;
    if entries.is_empty() {
        println!("(no deploys recorded in {}/deploys/)", cfg.vault_dir.display());
        return Ok(());
    }
    for e in &entries {
        println!(
            "{} {:<8} v{} {} {}ms {}{}",
            e.timestamp.format("%Y-%m-%d %H:%M UTC"),
            e.outcome,
            e.version,
            &e.git_head[..e.git_head.len().min(10)],
            e.duration_ms,
            if e.pre_deploy_snapshot.is_some() { "📸" } else { "  " },
            if e.skip_flags.is_empty() { String::new() } else { format!(" [{}]", e.skip_flags.join(",")) },
        );
        if let Some(reason) = &e.failure_reason {
            println!("    ↳ {}", reason.chars().take(200).collect::<String>());
        }
    }
    Ok(())
}

pub fn run_verify_stamp(cfg: &Config, path: Option<&str>) -> Result<()> {
    let stamp_path = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => cfg.state_dir.join("deploy-stamp.json"),
    };
    // Default pubkey location — either alongside the stamp (tool's
    // own key) or committed into the repo for third-party verification.
    let pubkey = cfg.state_dir.join("cosign.pub");
    let pubkey = if pubkey.exists() {
        pubkey
    } else {
        cfg.workspace.join("syntaur-ship/cosign.pub")
    };
    crate::stamp_sign::verify_stamp(&stamp_path, &pubkey)
}

pub fn run_hooks_install(cfg: &Config) -> Result<()> {
    let hooks_src = cfg.workspace.join("syntaur-ship/hooks");
    let hooks_dst = cfg.workspace.join(".git/hooks");
    if !hooks_dst.exists() {
        anyhow::bail!("{}/.git/hooks missing — is this a git repo?", cfg.workspace.display());
    }
    for name in ["pre-commit", "pre-push"] {
        let src = hooks_src.join(name);
        let dst = hooks_dst.join(name);
        if !src.exists() {
            log::warn!("[hooks] skipping {name} — {} missing", src.display());
            continue;
        }
        std::fs::copy(&src, &dst)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dst)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dst, perms)?;
        }
        log::info!("[hooks] installed {} → {}", src.display(), dst.display());
    }
    println!("✓ git hooks installed in {}", hooks_dst.display());
    println!("  Override with `SYNTAUR_SHIP_OVERRIDE=1 git commit/push ...` when needed.");
    Ok(())
}
