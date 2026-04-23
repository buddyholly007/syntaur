//! Single source of truth for Sean's LAN topology.
//!
//! This crate does NOT store secrets. IPs, ports, usernames, and the
//! "from host X reach host Y via jump Z" rules only. Passphrases,
//! tokens, API keys go in `syntaur-vault` (the per-host encrypted
//! store).
//!
//! Motivation: Sean kept catching routing mistakes where a Claude
//! session on claudevm would `ssh sean@192.168.1.239` (direct, broken
//! — claudevm isn't on the same L2 as TrueNAS) instead of the correct
//! `ssh -J root@192.168.1.3 truenas_admin@192.168.1.239`. Same with
//! "always through the HA mini-PC" being a decision that lived in
//! memory notes but not in code. This crate puts that rule in one
//! file, read by syntaur-topo the CLI + (later) the deploy-guard
//! hook + syntaur-ship config.

pub mod manifest;
pub mod resolve;

pub use manifest::{Host, HostRole, Manifest, Protocol, Service, SshConfig};
pub use resolve::{PathSpec, ReachabilityError};

/// Default manifest path. Override via `SYNTAUR_TOPO_PATH`.
pub fn default_manifest_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SYNTAUR_TOPO_PATH") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home).join("vault").join("syntaur-topology.yaml")
}

/// Detect the current host's short name from /etc/hostname. Falls
/// back to the raw OS value if no canonicalization is possible.
pub fn current_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .map(|s| s.split('.').next().unwrap_or(&s).to_lowercase())
        .unwrap_or_else(|| "unknown".into())
}

/// Resolve an OS hostname to the canonical manifest key.
///
/// Lookup order:
///   1. Exact match against `hosts.<name>`.
///   2. Case-insensitive match against any `aliases` entry on a host.
///   3. Returns the raw hostname back so the caller can surface a
///      "not in manifest" error with the actual OS name.
pub fn resolve_manifest_key(m: &manifest::Manifest, raw: &str) -> String {
    if m.hosts.contains_key(raw) {
        return raw.to_string();
    }
    let raw_lc = raw.to_lowercase();
    for (name, host) in &m.hosts {
        if host.aliases.iter().any(|a| a.to_lowercase() == raw_lc) {
            return name.clone();
        }
    }
    raw.to_string()
}
