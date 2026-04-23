//! Central configuration — loaded from `~/.syntaur/ship/config.toml` with
//! env overrides + hard-coded production defaults.
//!
//! Defaults mirror the current `deploy.sh` so Phase 1 is drop-in
//! compatible. Everything that today is magic strings in the bash
//! script is promoted to a struct field here so future phases can
//! evolve without touching the shell.

use anyhow::{Context, Result};
use std::path::PathBuf;

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

impl Config {
    pub fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("$HOME not set")?;
        let home = PathBuf::from(home);

        // Defaults match deploy.sh at 2026-04-23 (commit ~2027c41).
        // TODO Phase 4: allow overrides from ~/.syntaur/ship/config.toml.
        Ok(Self {
            workspace: home.join("openclaw-workspace"),
            social_manager: home.join("rust-social-manager"),
            mac_mini: "sean@192.168.1.58".into(),
            truenas_user: "truenas_admin".into(),
            truenas_ip: "192.168.1.239".into(),
            truenas_jump: "root@192.168.1.3".into(),
            bin_dir: "/mnt/cherry_family_nas/syntaur/bin".into(),
            viewer_host: "sean@192.168.1.69".into(),
            health_url: "http://192.168.1.239:18789/health".into(),
            state_dir: home.join(".syntaur/ship"),
            coord_broker_url: "http://192.168.1.150:19879".into(),
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
