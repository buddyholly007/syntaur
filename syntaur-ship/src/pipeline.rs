//! Pipeline orchestrator (Phase 2 — adds snapshot + rollback wiring).
//!
//! Stage order for `run_full`:
//!   preflight → snapshot (NEW) → build → mac_mini → git_push →
//!   truenas (now with .prev + auto-rollback) → viewer.

use anyhow::Result;
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
    pub skip_build: bool,
    pub skip_mac: bool,
    pub skip_git: bool,
    pub social_only: bool,
    /// Override the blocking CI-failure gate. Use when deploying
    /// DESPITE known CI failures (e.g. a CVE that's upstream-only and
    /// can't be fixed until an unrelated release cadence). Logged into
    /// the journal as an explicit skip-flag so it's auditable.
    pub force_ci_drift: bool,
}

pub struct StageContext<'a> {
    pub cfg: &'a Config,
    pub opts: &'a RunOptions,
}

pub fn run_full(cfg: &Config, opts: &RunOptions) -> Result<()> {
    let start = Instant::now();
    log::info!(
        "=== syntaur-ship pipeline starting (dry_run={} skip_build={} skip_mac={} skip_git={} social_only={}) ===",
        opts.dry_run, opts.skip_build, opts.skip_mac, opts.skip_git, opts.social_only,
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
    // the current HEAD is failing. Override via --force-ci-drift.
    // Catches the class of "I didn't notice CI was red" that had me
    // deploy past 5 consecutive cargo-audit failures before Sean
    // flagged the email notifications. Runs in dry-run too so
    // `syntaur-ship check` catches CI drift before the real run.
    let head = run_capture(
        "git",
        &["-C", cfg.workspace.to_str().unwrap(), "rev-parse", "HEAD"],
    )
    .unwrap_or_default()
    .trim()
    .to_string();
    let pre_failures = crate::ci_audit::run(&head);
    if !pre_failures.is_empty() {
        if opts.force_ci_drift || opts.dry_run {
            let tag = if opts.dry_run { "[ci-gate] (dry-run)" } else { "[ci-gate]" };
            log::warn!(
                "{tag} ⚠ {} CI workflow(s) failing on HEAD {}:",
                pre_failures.len(),
                &head[..head.len().min(10)]
            );
            for f in &pre_failures {
                log::warn!("   ✗ {f}");
            }
            if !opts.dry_run {
                log::warn!("[ci-gate] proceeding anyway because --force-ci-drift is set");
            }
        } else {
            let list = pre_failures
                .iter()
                .map(|f| format!("  ✗ {f}"))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "CI gate: {} workflow(s) failing on HEAD {}:\n{list}\n\nFix them or re-run with --force-ci-drift (logged).",
                pre_failures.len(),
                &head[..head.len().min(10)]
            );
        }
    } else {
        log::info!(
            "[ci-gate] ✓ all CI workflows passing on HEAD {}",
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
    let result = run_full_inner(cfg, opts, &ctx);
    let duration_ms = start.elapsed().as_millis();

    // Write journal entry regardless of outcome (unless dry-run).
    if !opts.dry_run {
        let outcome = if result.is_ok() { "success" } else { "aborted" };
        let (failed_stage, failure_reason) = match &result {
            Ok(()) => (None, None),
            Err(e) => (Some("unknown".into()), Some(format!("{e:#}"))),
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
                    pre_deploy_snapshot: None,
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

fn run_full_inner(cfg: &Config, opts: &RunOptions, ctx: &StageContext) -> Result<()> {
    // Phase 5: Cargo.lock drift — if deps changed since last successful
    // deploy, --skip-build is unsafe and we silently force a rebuild.
    // Prevents the class of bug where a stale binary ships after an
    // upstream dep bump.
    let skip_build_effective = if opts.skip_build {
        let current_sha = guards::cargo_lock_sha(&cfg.workspace).ok();
        let last_sha = state::read_stamp(&cfg.state_dir).ok().flatten()
            .and_then(|s| s.cargo_lock_sha256);
        match (current_sha, last_sha) {
            (Some(c), Some(l)) if c == l => true,
            (Some(_), Some(_)) => {
                log::warn!("[cargo-lock] drift detected since last deploy — overriding --skip-build, rebuilding");
                false
            }
            _ => true, // No prior stamp; honor --skip-build.
        }
    } else {
        false
    };

    stages::preflight::run(ctx)?;
    // Phase 3a: version sweep BEFORE build — abort deploy if the 5
    // public version surfaces disagree. Cheap local file reads; no
    // network. Fix at source + re-run rather than shipping drift.
    stages::version_sweep::run(ctx)?;
    // Backup-freshness gate: refuse to deploy if no independent
    // TrueNAS snapshot in the last 24h. Catches silently-broken
    // backup/replication tasks. Override via SYNTAUR_SHIP_ALLOW_STALE_BACKUP=1.
    stages::backup_freshness::run(ctx)?;
    // Phase 2: snapshot BEFORE any TrueNAS writes. If any later stage
    // fails we still have a restore point.
    let snapshot_name = stages::snapshot::run(ctx)?;
    if !skip_build_effective {
        stages::build::run(ctx)?;
    } else {
        log::warn!("[preflight] --skip-build honored; reusing existing target/release binaries");
    }
    if !opts.social_only {
        if !opts.skip_mac {
            stages::mac_mini::run(ctx)?;
        } else {
            log::warn!("[mac_mini] --skip-mac set; skipping smoke (emergency only)");
        }
        // Canary: re-probe Mac Mini /health after 45s to catch
        // delayed-crash bugs before rsync'ing to TrueNAS.
        stages::canary::run(ctx)?;
        if !opts.skip_git {
            stages::git_push::run(ctx)?;
        } else {
            log::warn!("[git_push] --skip-git set; not propagating to origin");
        }
    }
    stages::truenas::run(ctx)?;
    if !opts.social_only {
        stages::viewer::run(ctx)?;
    }
    // Phase 3a: post-deploy version audit on live prod. Warns (doesn't
    // abort) — prod is already live at this point; drift here means
    // repair at source + redeploy, not roll back.
    let _ = stages::version_audit::run(ctx);

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

    let mut skip_flags = Vec::new();
    if opts.skip_build { skip_flags.push("skip-build".into()); }
    if opts.skip_mac { skip_flags.push("skip-mac".into()); }
    if opts.skip_git { skip_flags.push("skip-git".into()); }
    if opts.social_only { skip_flags.push("social-only".into()); }
    if opts.force_ci_drift { skip_flags.push("force-ci-drift".into()); }

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
    if opts.skip_build { v.push("skip-build".into()); }
    if opts.skip_mac { v.push("skip-mac".into()); }
    if opts.skip_git { v.push("skip-git".into()); }
    if opts.social_only { v.push("social-only".into()); }
    if opts.force_ci_drift { v.push("force-ci-drift".into()); }
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
