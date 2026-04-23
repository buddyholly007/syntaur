//! Gaming PC viewer relaunch — kill old process, re-exec pointing at
//! fresh prod URL. Mirrors deploy.sh line 245.

use anyhow::{Context, Result};
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    let script = format!(
        "bash -c 'pkill -x syntaur-viewer 2>/dev/null; sleep 1; \
         env SYNTAUR_URL=http://192.168.1.239:18789 DISPLAY=:0 \
         XAUTHORITY=/home/sean/.Xauthority setsid nohup \
         /home/sean/.local/bin/syntaur-viewer > /tmp/syntaur-viewer.log 2>&1 < /dev/null & disown'"
    );
    log::info!(">> viewer relaunch on {}", ctx.cfg.viewer_host);
    if ctx.opts.dry_run {
        return Ok(());
    }
    let status = Command::new("ssh")
        .args([&ctx.cfg.viewer_host, &script])
        .status()
        .context("viewer relaunch ssh")?;
    // Don't fail the whole deploy if viewer relaunch fails — that's a
    // usability nicety, not a correctness property. Just log.
    if !status.success() {
        log::warn!("[viewer] relaunch ssh exited {status}; ignoring");
    }
    Ok(())
}
