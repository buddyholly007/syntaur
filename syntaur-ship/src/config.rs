//! Central configuration — resolved from the syntaur-topo manifest
//! at `~/vault/syntaur-topology.yaml`, with env-var escape hatches
//! for every field.
//!
//! Before 2026-04-23 this file hardcoded a handful of IPs / users
//! that also lived in `deploy.sh`, `~/.local/bin/deploy-guard.py`,
//! and the topology note in memory. Every time a host moved, three
//! files needed edits and memory kept drifting. The topo manifest
//! is now the single source of truth; this config just reads it.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use syntaur_topo_core::{default_manifest_path, manifest::Manifest};

#[derive(Debug, Clone)]
pub struct Config {
    /// Local checkout path (claudevm) — absolute path to
    /// `~/openclaw-workspace`.
    pub workspace: PathBuf,
    /// Secondary repo that also deploys to TrueNAS (`rust-social-manager`).
    pub social_manager: PathBuf,

    /// Mac Mini SSH target ("user@host").
    pub mac_mini: String,

    /// TrueNAS SSH target via jump host.
    pub truenas_user: String,
    pub truenas_ip: String,
    pub truenas_jump: String,

    /// Bind-mounted binary directory inside TrueNAS host FS — docker
    /// container sees this as /app/bin/*.
    pub bin_dir: String,

    /// Gaming PC viewer SSH target.
    pub viewer_host: String,

    /// Prod health URL — tool hits this after docker restart.
    pub health_url: String,

    /// Tool state directory on claudevm. Contains deploy-stamp.json +
    /// lock files + cached CI status.
    pub state_dir: PathBuf,

    /// claude-coord broker endpoint (Phase 5).
    pub coord_broker_url: String,

    /// This tool's session name for broker registration.
    pub coord_session: String,

    /// Vault directory — append-only deploy journal lives in
    /// `<vault>/deploys/YYYY-MM.jsonl` (Phase 4).
    pub vault_dir: PathBuf,
}

/// Helpers that pull a value from the manifest for a specific host.
fn host_addr(m: &Manifest, key: &str) -> Result<String> {
    Ok(m.hosts
        .get(key)
        .ok_or_else(|| anyhow!("host `{key}` not in topo manifest — check ~/vault/syntaur-topology.yaml"))?
        .address
        .clone())
}

fn host_ssh_user(m: &Manifest, key: &str) -> Result<String> {
    Ok(m.hosts
        .get(key)
        .and_then(|h| h.ssh.as_ref())
        .ok_or_else(|| anyhow!("host `{key}` has no ssh config in topo manifest"))?
        .user
        .clone())
}

fn ssh_target(m: &Manifest, key: &str) -> Result<String> {
    Ok(format!("{}@{}", host_ssh_user(m, key)?, host_addr(m, key)?))
}

fn service_endpoint(m: &Manifest, svc_name: &str, path: &str) -> Result<String> {
    let svc = m
        .services
        .get(svc_name)
        .ok_or_else(|| anyhow!("service `{svc_name}` not in topo manifest"))?;
    let addr = host_addr(m, &svc.host)?;
    Ok(format!("http://{}:{}{}", addr, svc.port, path))
}

/// Each Config field has an env override so a temporary misrouting
/// (e.g. testing a new TrueNAS IP) doesn't need a manifest edit.
fn env_or<F>(env_key: &str, fallback: F) -> Result<String>
where
    F: FnOnce() -> Result<String>,
{
    match std::env::var(env_key) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => fallback(),
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("$HOME not set")?;
        let home = PathBuf::from(home);

        let manifest_path = default_manifest_path();
        let m = Manifest::load(&manifest_path).with_context(|| {
            format!(
                "loading topo manifest {} — syntaur-ship config depends on it",
                manifest_path.display()
            )
        })?;

        // truenas: reach claudevm → truenas is `via ha-minipc` in the
        // manifest. We extract the pieces rather than using the
        // resolve::ssh_path helper so downstream code can keep using
        // the existing "-J jump user@ip" shape.
        let truenas_user = env_or("SYNTAUR_TRUENAS_USER", || host_ssh_user(&m, "truenas"))?;
        let truenas_ip = env_or("SYNTAUR_TRUENAS_IP", || host_addr(&m, "truenas"))?;
        let truenas_jump = env_or("SYNTAUR_TRUENAS_JUMP", || {
            Ok(format!(
                "{}@{}",
                host_ssh_user(&m, "ha-minipc")?,
                host_addr(&m, "ha-minipc")?
            ))
        })?;

        let mac_mini = env_or("SYNTAUR_MAC_MINI", || ssh_target(&m, "mac-mini"))?;
        let viewer_host = env_or("SYNTAUR_VIEWER_HOST", || ssh_target(&m, "gaming-pc"))?;

        let health_url = env_or("SYNTAUR_HEALTH_URL", || {
            service_endpoint(&m, "syntaur-gateway", "/health")
        })?;
        let coord_broker_url = env_or("SYNTAUR_COORD_BROKER_URL", || {
            service_endpoint(&m, "claude-coord-broker", "")
        })?;

        Ok(Self {
            workspace: home.join("openclaw-workspace"),
            social_manager: home.join("rust-social-manager"),
            mac_mini,
            truenas_user,
            truenas_ip,
            truenas_jump,
            // bin_dir is filesystem-internal to TrueNAS; doesn't fit
            // the host/service schema. Keep as-is (env-overridable).
            bin_dir: std::env::var("SYNTAUR_BIN_DIR")
                .unwrap_or_else(|_| "/mnt/cherry_family_nas/syntaur/bin".into()),
            viewer_host,
            health_url,
            state_dir: home.join(".syntaur/ship"),
            coord_broker_url,
            coord_session: std::env::var("CLAUDE_COORD_SESSION")
                .unwrap_or_else(|_| "syntaur-ship".into()),
            vault_dir: home.join("vault"),
        })
    }

    /// Build an `ssh -J jump user@ip` prefix as a vec of args, suitable
    /// for `Command::args()`.
    pub fn truenas_ssh_args(&self) -> Vec<String> {
        vec![
            "-J".into(),
            self.truenas_jump.clone(),
            format!("{}@{}", self.truenas_user, self.truenas_ip),
        ]
    }

    /// TrueNAS rsync `-e` parameter — note the quoting: this becomes a
    /// single argv entry passed to rsync as `-e`, so the internal
    /// `ssh -J ...` is one shell token.
    pub fn truenas_rsync_ssh(&self) -> String {
        format!("ssh -J {}", self.truenas_jump)
    }
}
