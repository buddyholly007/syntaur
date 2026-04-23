//! Manifest data model + YAML (de)serialization.
//!
//! The whole file is deserialized in one shot. Manifests are small
//! (hundreds of lines, tops) and re-read on every command — no need
//! for incremental loading.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version. Bump on breaking shape changes.
    pub version: u32,
    /// Keyed by stable short name — `gaming-pc`, `claudevm`, etc.
    pub hosts: BTreeMap<String, Host>,
    /// Keyed by stable short name — `syntaur-gateway`, etc.
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        let m: Manifest = serde_yaml::from_str(&body)
            .with_context(|| format!("parsing manifest {}", path.display()))?;
        if m.version != 1 {
            anyhow::bail!(
                "manifest version {} unsupported (this tool reads v1)",
                m.version
            );
        }
        Ok(m)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = serde_yaml::to_string(self).context("serializing manifest")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    /// Human-readable display name — shown in `host <name>` output.
    pub display_name: String,
    /// One or more functional roles. Used for `list --role X` filters.
    #[serde(default)]
    pub roles: Vec<HostRole>,
    /// Free-form OS / platform note.
    #[serde(default)]
    pub os: String,
    /// LAN address. Kept as a string so YAML edits can note CIDR / port / comment.
    pub address: String,
    /// Alternative names this host is known by at the OS level.
    /// Matched case-insensitively against the current hostname so a
    /// box whose `uname -n` returns "LinuxGamingPC" can still be
    /// resolved to the clean manifest key `gaming-pc`.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Optional secondary addresses (Tailscale, VPN, etc.).
    #[serde(default)]
    pub alt_addresses: BTreeMap<String, String>,
    /// How this host is reachable from each other host. Missing entry
    /// means "unknown/unreachable — don't route here from that host."
    /// Key is the other host's short name, or `"*"` as a default fall-through.
    #[serde(default)]
    pub reachable_from: BTreeMap<String, Reach>,
    /// SSH config if this host accepts ssh. Absent means no SSH (IoT
    /// device, HTTP-only service host, etc.).
    #[serde(default)]
    pub ssh: Option<SshConfig>,
    /// Operational status. `active` is the default.
    #[serde(default)]
    pub status: HostStatus,
    /// Free-form notes — deploy caveats, known gotchas, anything.
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostStatus {
    Active,
    Decommissioned,
    Planned,
    Maintenance,
}

impl Default for HostStatus {
    fn default() -> Self {
        HostStatus::Active
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostRole {
    ClaudeCodeHost,
    AlwaysOn,
    JumpHost,
    GpuHost,
    Prod,
    Test,
    Storage,
    DockerHost,
    HomeAssistant,
    VoiceAssistant,
    TrafficControl,
    Decommissioned,
    Nvr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reach {
    /// `direct` = same L2, just ssh/curl to the address.
    /// `via` = hop through another host named in the `jump` field.
    pub kind: ReachKind,
    /// When `kind == Via`, which host to jump through.
    #[serde(default)]
    pub jump: Option<String>,
    /// Optional comment explaining WHY this route was picked.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReachKind {
    /// Direct L2/L3 — plain `ssh user@host`, no ProxyJump.
    Direct,
    /// Hop through `jump`.
    Via,
    /// Route is known-bad or policy-forbidden.
    Forbidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    pub user: String,
    /// `key` (default, ~/.ssh/id_ed25519) or `password`.
    #[serde(default = "default_ssh_auth")]
    pub auth: String,
    /// If a fallback password exists, its vault key — NEVER the
    /// password itself. Looked up at invoke time via syntaur-vault.
    #[serde(default)]
    pub fallback_password_vault_key: Option<String>,
}

fn default_ssh_auth() -> String {
    "key".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    /// Host short-name this service runs on.
    pub host: String,
    pub port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    /// For HTTP(S): path to probe (e.g. `/health`). Ignored for TCP-only.
    #[serde(default)]
    pub path: String,
    /// Short description of what the service does.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub notes: String,
}

fn default_protocol() -> Protocol {
    Protocol::Http
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    Http,
    Https,
    Ssh,
    Tcp,
    Udp,
    Mqtt,
    Grpc,
}
