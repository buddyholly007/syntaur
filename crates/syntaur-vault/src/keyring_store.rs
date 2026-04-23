//! OS keychain passphrase cache.
//!
//! Wraps the `keyring` crate so the CLI has a simple `save` / `fetch`
//! / `clear` surface. Keyring is best-effort: if the host has no
//! keyring daemon (headless claudevm, CI runners, …), each function
//! returns an error that the caller translates into "fall back to
//! interactive prompt."
//!
//! Service + account names are stable so `gnome-keyring-daemon`
//! presents a readable row under "Passwords and Keys" → "syntaur" →
//! "syntaur-vault."

use anyhow::{anyhow, Context, Result};

const SERVICE: &str = "syntaur-vault";

fn account_for(vault_path: &std::path::Path) -> String {
    // Include the vault path so multiple vaults on the same host
    // don't collide in the keyring. Most users have one, but a test
    // run + a real vault shouldn't overwrite each other's entry.
    format!("vault:{}", vault_path.display())
}

/// Store the passphrase in the OS keyring for this vault. Overwrites
/// any existing entry. Errors bubble up with the underlying cause so
/// the caller can tell "no keyring daemon" apart from "user denied
/// permission."
pub fn save(vault_path: &std::path::Path, passphrase: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account_for(vault_path))
        .map_err(|e| anyhow!("keyring entry init: {e}"))?;
    entry
        .set_password(passphrase)
        .context("storing passphrase in OS keyring")?;
    Ok(())
}

/// Fetch the passphrase for this vault. Returns `None` if no entry
/// was stored (distinct from errors that indicate the keyring itself
/// is unreachable).
pub fn fetch(vault_path: &std::path::Path) -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE, &account_for(vault_path))
        .map_err(|e| anyhow!("keyring entry init: {e}"))?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        // v3 of the keyring crate reports "no entry" via this specific
        // variant; anything else is a real reach-the-keyring failure.
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("keyring fetch: {e}")),
    }
}

/// Remove the stored passphrase for this vault. Succeeds (silently)
/// if nothing was stored.
pub fn clear(vault_path: &std::path::Path) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account_for(vault_path))
        .map_err(|e| anyhow!("keyring entry init: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("keyring clear: {e}")),
    }
}
