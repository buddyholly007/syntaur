//! claude-coord broker integration (Phase 4).
//!
//! Broadcasts deploy state changes to other Claude Code sessions so
//! they can hold off pushes mid-deploy. Phase 5 adds a real file lock;
//! Phase 4 just emits info messages.
//!
//! Telegram is explicitly NOT used — Sean is phasing out Telegram. The
//! broker + vault journal are the notification surfaces.

use anyhow::Result;
use std::process::Command;

use crate::config::Config;

/// POST a `kind: info` message to the broker. `to: *` broadcasts.
pub fn broadcast_info(cfg: &Config, body: &str) -> Result<()> {
    post_msg(cfg, "*", "info", body)
}

/// POST a `kind: intent` message. Used at deploy-start; other sessions
/// see it and can hold pushes.
pub fn broadcast_intent(cfg: &Config, body: &str) -> Result<()> {
    post_msg(cfg, "*", "intent", body)
}

fn post_msg(cfg: &Config, to: &str, kind: &str, body: &str) -> Result<()> {
    let url = format!("{}/msg", cfg.coord_broker_url);
    let payload = serde_json::json!({
        "from": cfg.coord_session,
        "to": to,
        "kind": kind,
        "body": body,
    })
    .to_string();
    let out = Command::new("curl")
        .args([
            "-sf",
            "-X",
            "POST",
            &url,
            "-H",
            "content-type: application/json",
            "-d",
            &payload,
            "--max-time",
            "5",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            log::debug!(
                "[coord] broker post {kind} returned non-zero: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            Ok(()) // Non-fatal — broker being down shouldn't abort deploy.
        }
        Err(e) => {
            log::debug!("[coord] broker unreachable: {e}");
            Ok(())
        }
    }
}
