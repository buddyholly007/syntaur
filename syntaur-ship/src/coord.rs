//! claude-coord broker integration (Phase 4 + Phase 5).
//!
//! Phase 4: info/intent broadcast.
//! Phase 5: file-lock acquisition on openclaw-workspace/Cargo.lock
//!          for the duration of a deploy. Other sessions editing that
//!          file while we hold the lock get blocked by the broker's
//!          PreToolUse hook.

use anyhow::Result;
use std::process::Command;

use crate::config::Config;

pub fn broadcast_info(cfg: &Config, body: &str) -> Result<()> {
    post_msg(cfg, "*", "info", body)
}

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
        .args(["-sf", "-X", "POST", &url, "-H", "content-type: application/json",
               "-d", &payload, "--max-time", "5"])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(_) | Err(_) => Ok(()), // Non-fatal — broker being down shouldn't abort deploy.
    }
}

/// Try to acquire a broker lock on `file_path` for `ttl_secs` seconds.
/// Returns true if acquired, false if another session holds it.
///
/// On success, the lock must be released via `release_lock` when the
/// deploy is done (success or failure). If not released explicitly,
/// the broker auto-expires it after the TTL.
pub fn try_lock(cfg: &Config, file_path: &str, intent: &str, ttl_secs: i64) -> Result<LockResult> {
    let url = format!("{}/lock", cfg.coord_broker_url);
    let payload = serde_json::json!({
        "session": cfg.coord_session,
        "file_path": file_path,
        "intent": intent,
        "ttl_secs": ttl_secs,
    })
    .to_string();
    let out = Command::new("curl")
        .args(["-s", "-X", "POST", &url, "-H", "content-type: application/json",
               "-d", &payload, "--max-time", "5"])
        .output()?;
    if !out.status.success() {
        // Broker unreachable → proceed without lock (degraded mode).
        log::warn!("[coord] broker unreachable while trying lock; proceeding without lock");
        return Ok(LockResult::BrokerUnavailable);
    }
    let body: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    if body["acquired"].as_bool().unwrap_or(false) {
        Ok(LockResult::Acquired {
            ttl_secs: body["ttl_secs"].as_i64().unwrap_or(ttl_secs),
        })
    } else {
        Ok(LockResult::HeldByOther {
            holder: body["holder"].as_str().unwrap_or("unknown").to_string(),
            intent: body["intent"].as_str().unwrap_or("").to_string(),
            expires_in_secs: body["expires_in_secs"].as_i64().unwrap_or(0),
        })
    }
}

pub fn release_lock(cfg: &Config, file_path: &str) -> Result<()> {
    let url = format!("{}/unlock", cfg.coord_broker_url);
    let payload = serde_json::json!({
        "session": cfg.coord_session,
        "file_path": file_path,
    })
    .to_string();
    let _ = Command::new("curl")
        .args(["-sf", "-X", "POST", &url, "-H", "content-type: application/json",
               "-d", &payload, "--max-time", "5"])
        .output();
    Ok(())
}

#[derive(Debug)]
pub enum LockResult {
    Acquired { ttl_secs: i64 },
    HeldByOther { holder: String, intent: String, expires_in_secs: i64 },
    BrokerUnavailable,
}
