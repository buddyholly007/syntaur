//! Canary stage — re-probe Mac Mini /health after a delay so
//! binaries that crash 30s after startup (DB init race, MQTT
//! reconnect loop blow-up, late-bound config errors) get caught
//! BEFORE rsync'ing to TrueNAS.
//!
//! Simple logic: sleep N seconds, hit /health again. If the gateway
//! died in the meantime, curl fails → stage fails → pipeline aborts
//! → TrueNAS is never touched. If healthy, move on.

use anyhow::Result;
use std::process::Command;

use crate::pipeline::StageContext;

/// Canary window — long enough to catch first-minute crashers,
/// short enough not to blow up total deploy time.
pub const CANARY_SECS: u64 = 45;

pub fn run(ctx: &StageContext) -> Result<()> {
    if ctx.opts.dry_run || ctx.opts.social_only {
        return Ok(());
    }
    log::info!(
        ">> canary: sleep {CANARY_SECS}s then re-probe Mac Mini /health (catches delayed-crash bugs)"
    );
    std::thread::sleep(std::time::Duration::from_secs(CANARY_SECS));
    let output = Command::new("ssh")
        .args([
            "-n",
            &ctx.cfg.mac_mini,
            "curl -sf --max-time 5 http://127.0.0.1:18789/health",
        ])
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        anyhow::bail!(
            "canary: Mac Mini /health failed after {CANARY_SECS}s — gateway probably crashed post-startup. \
             Check /tmp/syntaur-gateway.log on Mac Mini. TrueNAS not touched."
        );
    }
    log::info!("   canary ✓ (gateway survived {CANARY_SECS}s under load)");
    Ok(())
}
