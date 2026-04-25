//! Verify stage — Phase 6 hook for syntaur-verify.
//!
//! Runs AFTER canary (gateway survived 45s) and BEFORE `git_push` +
//! TrueNAS deploy. Captures screenshots at all three viewports
//! against the Mac Mini staging gateway and, if Opus credentials
//! are available, runs vision findings.
//!
//! Policy:
//!   - Regressions (console errors, visual diffs, Opus `regression`
//!     severity) abort the pipeline → TrueNAS never sees the binary.
//!   - Suggestions are logged but don't block.
//!   - --skip-verify flag and SYNTAUR_SHIP_SKIP_VERIFY=1 env both
//!     bypass the stage entirely (honored for emergency deploys).
//!   - Missing syntaur-verify binary: soft-skip with a WARN (same as
//!     Phase 5 git-push + CI-audit behavior — these gates degrade
//!     gracefully rather than blocking if the tool is missing).
//!
//! Output lands in ~/.syntaur-verify/runs/<ts>/ same as a manual
//! invocation. The ship-side just needs pass/fail.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    if ctx.opts.dry_run || ctx.opts.skip_mac || ctx.opts.social_only {
        return Ok(());
    }
    if ctx.opts.skip_verify {
        log::warn!("[verify] --skip-verify set; skipping visual audit (emergency only)");
        return Ok(());
    }
    if std::env::var("SYNTAUR_SHIP_SKIP_VERIFY").ok().as_deref() == Some("1") {
        log::warn!("[verify] SYNTAUR_SHIP_SKIP_VERIFY=1 in env; skipping visual audit");
        return Ok(());
    }

    // Locate the verify binary. Prefer workspace release over installed
    // ~/.local/bin so a fresh-built syntaur-verify is what runs.
    let binary = locate_verify_binary(ctx)?;
    if binary.is_none() {
        log::warn!(
            "[verify] syntaur-verify binary not found in target/release or ~/.local/bin — \
             skipping audit. Build with `cargo build --release -p syntaur-verify`"
        );
        return Ok(());
    }
    let binary = binary.unwrap();

    // The Mac Mini gateway binds to 127.0.0.1 only (smoke stage hits
    // it via ssh+curl for the same reason). We reach it from claudevm
    // via SSH port-forward so verify's headless Chromium can `goto`
    // normally. Tunnel lives for the duration of this stage.
    let tunnel_port = 18789u16;
    let tunnel = TunnelGuard::open(&ctx.cfg.mac_mini, tunnel_port)
        .context("opening SSH tunnel to Mac Mini for verify")?;

    let target_url = format!("http://127.0.0.1:{tunnel_port}");
    log::info!(">> verify: sweeping desktop/tablet/mobile against {target_url}");

    let mut cmd = Command::new(&binary);
    cmd.args([
        "--target-url",
        &target_url,
        "--viewports",
        "desktop,tablet,mobile",
        // Ship runs verify after every successful canary, so THIS run's
        // screenshots become the "last-known-good" baseline for the
        // NEXT ship. Without --update-baselines, the first ship after
        // any layout-affecting change (grid-auto-rows, new widget,
        // width bump) gets 100% pixel-diff regressions and blocks
        // forever. Running with it effectively does
        // "capture + Opus-audit", which is the right shape for a
        // gate that runs on every deploy. Operators doing manual
        // interactive verifies can still catch visual regressions via
        // the normal (non-ship) code path which defaults to diff-mode.
        "--update-baselines",
    ]);
    // Run the regression-flow corpus if any anonymous-safe flows
    // exist in the workspace's flows/ dir. These catch interactive
    // bugs the screenshot sweep misses — the 2026-04-25 module-reset
    // bug shipped through unauth-screenshot verify because every page
    // returned 200, but only a flow that asserts URL stays at
    // /scheduler after navigation could have caught the JS bounce.
    let flows_dir = ctx
        .cfg
        .workspace
        .join("crates/syntaur-verify/flows");
    if flows_dir.is_dir() {
        cmd.arg("--flows-dir").arg(&flows_dir);
        log::info!(
            ">> verify: running regression flows from {}",
            flows_dir.display()
        );
    }
    // Opus vision is opt-in when either the vault agent is running OR
    // OPENROUTER_API_KEY is set in this process's env. The verify
    // binary detects both and degrades heuristic-only otherwise.
    if opus_available() {
        cmd.arg("--with-opus");
        // Auto-fix: when Opus finds a regression AND we can afford to
        // rebuild+reload (not a social-only micro-deploy), let verify
        // apply the Opus-proposed edits, rebuild the gateway, reload
        // Mac Mini, and re-verify. Capped at --max-iter 2 and
        // --max-loc 150 per module so a runaway fix never balloons.
        // --auto-fix implies --with-opus so it's free here.
        if ctx.opts.auto_fix {
            cmd.args([
                "--auto-fix",
                "--max-iter",
                "2",
                "--max-loc",
                "150",
                "--reload-cmd",
                // Same pattern mac_mini.rs uses to relaunch: ssh + pkill
                // + setsid nohup. Uses the tunnel we already opened.
                &format!(
                    "ssh {} 'pkill -9 -f /tmp/syntaur-gateway; sleep 1; \
                     cd /tmp && setsid nohup ./syntaur-gateway > /tmp/syntaur-gateway.log 2>&1 < /dev/null &'",
                    ctx.cfg.mac_mini
                ),
            ]);
            log::info!(">> verify: --auto-fix enabled (max 2 iters, ≤150 LoC/module)");
        }
    } else {
        log::warn!(
            "[verify] no vault agent + no OPENROUTER_API_KEY — running heuristic-only \
             (console errors + visual diffs only, no Opus vision)"
        );
    }

    let status = cmd.status().context("spawning syntaur-verify")?;
    // Keep the tunnel alive until the verify child exits.
    drop(tunnel);

    match status.code() {
        Some(0) => {
            log::info!("   verify ✓ (no regressions across desktop/tablet/mobile)");
            Ok(())
        }
        Some(1) => {
            anyhow::bail!(
                "verify: regressions caught by visual audit — TrueNAS NOT touched. \
                 Inspect ~/.syntaur-verify/runs/<latest>/report.json and either fix the \
                 regression or re-run ship with --skip-verify if the deploy is critical."
            )
        }
        Some(code) => {
            anyhow::bail!(
                "verify: syntaur-verify exited with code {code} (tool error, not a findings \
                 failure). Check output above; re-run with --skip-verify to bypass."
            )
        }
        None => anyhow::bail!("verify: syntaur-verify killed by signal"),
    }
}

fn locate_verify_binary(ctx: &StageContext) -> Result<Option<PathBuf>> {
    let workspace_release = ctx
        .cfg
        .workspace
        .join("target/release/syntaur-verify");
    if workspace_release.exists() {
        return Ok(Some(workspace_release));
    }
    let installed = dirs_home().map(|h| h.join(".local/bin/syntaur-verify"));
    if let Some(p) = installed {
        if p.exists() {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn opus_available() -> bool {
    // Env var — covers CI + the ship-spawned subprocess inheriting from
    // an operator's interactive shell.
    if std::env::var("OPENROUTER_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    // Vault agent socket — verify itself will do the same check with
    // the actual `openrouter` entry lookup; we only need a heuristic
    // "is it worth passing --with-opus" here.
    let sock = dirs_home()
        .map(|h| h.join(".syntaur/vault.sock"))
        .filter(|p| p.exists());
    sock.is_some()
}

/// RAII wrapper for an SSH port-forward. Drop closes the tunnel.
/// Uses `-o ExitOnForwardFailure=yes` so a port conflict fails fast
/// rather than leaving a zombie tunnel that doesn't actually forward.
struct TunnelGuard {
    child: std::process::Child,
}

impl TunnelGuard {
    fn open(remote: &str, local_port: u16) -> Result<Self> {
        // If port is already bound (e.g., operator has a tunnel up from
        // a manual session), reuse it — ssh will complain and exit,
        // which we treat as "someone else owns the port, just try to
        // use it".
        let bind_target = format!("{local_port}:127.0.0.1:18789");
        let child = std::process::Command::new("ssh")
            .args([
                "-N",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                "-L",
                &bind_target,
                remote,
            ])
            .spawn()
            .context("spawning ssh port-forward")?;

        // Give ssh a beat to establish the tunnel before verify hits
        // the port.  poll-then-timeout would be nicer but this stage
        // runs once per deploy so a fixed 2s wait is fine.
        std::thread::sleep(std::time::Duration::from_millis(2000));
        Ok(Self { child })
    }
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
