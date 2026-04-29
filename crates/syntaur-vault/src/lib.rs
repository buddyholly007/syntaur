//! Encrypted secret storage for Sean's personal tokens + API keys.
//!
//! The core library is split into:
//!
//! - [`Vault`] — the plaintext data model (entries + metadata).
//! - [`file`] — on-disk format (magic + salt + nonce + AEAD ciphertext).
//! - [`crypto`] — argon2id KDF + ChaCha20-Poly1305 AEAD.
//! - [`agent`] — Unix-socket JSON protocol; the daemon owns the derived
//!   key so it never leaves the process once loaded.
//!
//! Future Syntaur gateway hardening will link this crate directly and
//! read its own secrets from the same vault format. For now only the
//! binary in this crate consumes it.

// agent uses UnixListener + daemonize (unix-only). Gated so the lib
// can still compile on Windows CI for the syntaur-gateway transitive
// dep. The CLI bin in src/bin/ is not part of any build target on
// Windows so it does not need its own cfg gates.
#[cfg(unix)]
pub mod agent;
pub mod crypto;
pub mod file;
pub mod import;
#[cfg(unix)]
pub mod keyring_store;
pub mod vault;

#[cfg(unix)]
pub use agent::{AgentRequest, AgentResponse, Status};
pub use vault::{Entry, Vault};

/// Protocol + on-disk format version. Bump on any breaking change.
pub const FORMAT_VERSION: u8 = 1;

/// Default on-disk location for the vault blob. Lives in the
/// NFS-shared `~/vault/` dir so gaming PC + claudevm see the same
/// encrypted payload. The agent on each host caches its own derived
/// key in process memory; edits from one host are visible to the
/// other on the next `get` call (agent re-reads the file each time).
///
/// Override with `SYNTAUR_VAULT_PATH` env var.
pub fn default_vault_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SYNTAUR_VAULT_PATH") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home).join("vault").join("syntaur-vault.enc")
}

/// Default Unix socket for the agent. Per-host (NOT in the NFS dir)
/// so each host runs its own agent process with its own cached key.
pub fn default_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SYNTAUR_VAULT_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home).join(".syntaur").join("vault.sock")
}

/// Default pidfile path for the agent.
pub fn default_pidfile_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home).join(".syntaur").join("vault.pid")
}
