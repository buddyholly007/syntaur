//! Deploy stamp + state file management.
//!
//! A deploy stamp is the tool's record of a successful deploy: what
//! HEAD SHA went live, what binary SHA was on prod, when, by which
//! operator session. It's written to
//! `~/.syntaur/ship/deploy-stamp.json` on claudevm and consulted by:
//!
//! - git pre-commit hook (Phase 8) to refuse commits past last deploy
//! - `syntaur-ship status` (Phase 4) for "is prod current?"
//! - external verification scripts / auditors
//!
//! Phase 1 writes an unsigned stamp. Phase 7 wraps each stamp in a
//! cosign signature bundle.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStamp {
    /// ISO-8601 UTC instant when deploy finished successfully.
    pub deployed_at: DateTime<Utc>,
    /// Git commit SHA that was built.
    pub git_head: String,
    /// Parsed from `cargo pkgid` / `/VERSION` at deploy time.
    pub version: String,
    /// SHA-256 of the `syntaur-gateway` binary that was rsynced to prod.
    pub gateway_sha256: String,
    /// SHA-256 of the mace binary, if deployed.
    pub mace_sha256: Option<String>,
    /// SHA-256 of the rust-social-manager binary, if deployed.
    pub social_manager_sha256: Option<String>,
    /// Optional ZFS snapshot name created before deploy (Phase 2).
    pub pre_deploy_snapshot: Option<String>,
    /// Which syntaur-ship session wrote this stamp.
    pub deploy_session: String,
    /// The `--skip-*` flags in effect (empty list for the happy path).
    pub skip_flags: Vec<String>,
    /// SHA-256 of Cargo.lock at deploy time. Phase 5 uses this to
    /// detect dependency drift — if Cargo.lock differs from the last
    /// successful deploy's stamp, --skip-build is silently ignored
    /// (deps shifted, rebuild required).
    #[serde(default)]
    pub cargo_lock_sha256: Option<String>,
}

pub fn stamp_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("deploy-stamp.json")
}

pub fn read_stamp(state_dir: &Path) -> Result<Option<DeployStamp>> {
    let p = stamp_path(state_dir);
    if !p.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&p)
        .with_context(|| format!("read {}", p.display()))?;
    let stamp: DeployStamp = serde_json::from_str(&s)
        .with_context(|| format!("parse {}", p.display()))?;
    Ok(Some(stamp))
}

pub fn write_stamp(state_dir: &Path, stamp: &DeployStamp) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("mkdir {}", state_dir.display()))?;
    let p = stamp_path(state_dir);
    let tmp = p.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(stamp)?;
    std::fs::write(&tmp, s).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p)
        .with_context(|| format!("atomic rename {}", p.display()))?;
    Ok(())
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(sha256_hex(&data))
}
