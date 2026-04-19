//! Encrypted key-value store for secrets — Phase 3.1 of the 2026-04-19
//! security remediation plan.
//!
//! Every value is AES-256-GCM encrypted with the master key in
//! `~/.syntaur/master.key` (the same key the OAuth-token store already
//! uses). The vault file itself is plaintext JSON at
//! `~/.syntaur/vault.json` — a flat object mapping secret names to their
//! `enc:<hex-nonce-ciphertext>` blobs. Plaintext JSON wrapping means a
//! corrupted single value doesn't destroy the whole vault, and the key
//! names themselves are browsable without decryption.
//!
//! File permissions: 0600 owner-only. Startup refuses to load a vault
//! with wider permissions.
//!
//! CLI surface, via `syntaur-gateway vault <cmd>`:
//!
//!   vault set <name> [value]        set a secret (prompts if value omitted)
//!   vault get <name>                print decrypted value to stdout
//!   vault list                      list secret names (not values)
//!   vault delete <name>             remove a secret
//!   vault import <env-file-path>    bulk-import KEY=VALUE lines
//!   vault rotate                    re-encrypt everything under current master key
//!
//! Config interpolation (`{{vault.NAME}}` → decrypted value) is tracked
//! separately in the config loader — not this module's concern.

use aes_gcm::{Aes256Gcm, Key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const VAULT_FILE: &str = "vault.json";

/// On-disk shape. Plaintext JSON, each value is an `enc:…` blob.
#[derive(Serialize, Deserialize, Default, Clone)]
struct VaultFile {
    /// Format version; bump on breaking layout changes.
    #[serde(default = "default_version")]
    version: u32,
    /// Secret name → encrypted blob. BTreeMap keeps the file diff-friendly
    /// (sorted key order) so reviewing changes in git is easier.
    secrets: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    1
}

pub struct Vault {
    key: Key<Aes256Gcm>,
    path: PathBuf,
    file: VaultFile,
}

impl Vault {
    /// Open or create a vault at `data_dir/vault.json`. Uses the master
    /// key from `data_dir/master.key`. Creates the file on first use
    /// with 0600 permissions. Refuses to open a vault with wider perms.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let key = crate::crypto::load_or_create_key(data_dir)?;
        let path = data_dir.join(VAULT_FILE);

        let file = if path.exists() {
            Self::assert_permissions(&path)?;
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("read vault: {e}"))?;
            if bytes.is_empty() {
                VaultFile::default()
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|e| format!("parse vault.json: {e}"))?
            }
        } else {
            VaultFile::default()
        };

        let mut v = Self { key, path, file };
        if !v.path.exists() {
            v.persist()?;
        }
        Ok(v)
    }

    fn assert_permissions(path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path)
                .map_err(|e| format!("stat vault: {e}"))?;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(format!(
                    "vault file {} has mode {:o}; must be 0600 (owner-only). Fix with: chmod 600 {}",
                    path.display(),
                    mode,
                    path.display()
                ));
            }
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.file)
            .map_err(|e| format!("serialize vault: {e}"))?;
        // Atomic-ish write: write to a temp file, then rename.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())
            .map_err(|e| format!("write vault tmp: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("chmod vault tmp: {e}"))?;
        }
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("rename vault: {e}"))?;
        Ok(())
    }

    /// Set a secret. Replaces the existing value if any.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let encrypted = crate::crypto::encrypt(&self.key, value)?;
        self.file.secrets.insert(name.to_string(), encrypted);
        self.persist()
    }

    /// Get a secret. Returns None if the key doesn't exist.
    pub fn get(&self, name: &str) -> Result<Option<String>, String> {
        let Some(stored) = self.file.secrets.get(name) else {
            return Ok(None);
        };
        crate::crypto::decrypt(&self.key, stored).map(Some)
    }

    /// List all secret names (values stay encrypted).
    pub fn list_keys(&self) -> Vec<String> {
        self.file.secrets.keys().cloned().collect()
    }

    /// Delete a secret. Returns whether a secret was actually removed.
    pub fn delete(&mut self, name: &str) -> Result<bool, String> {
        let removed = self.file.secrets.remove(name).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// Re-encrypt every entry under the current master key. Useful after
    /// a master-key rotation — callers should rotate `master.key` first,
    /// then call this to re-wrap all values under the new key material.
    /// Entries that fail to decrypt (e.g. under a previous key) are
    /// reported but not removed.
    pub fn rotate(&mut self) -> Result<RotateReport, String> {
        let mut ok = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let entries: Vec<(String, String)> = self
            .file
            .secrets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, stored) in entries {
            match crate::crypto::decrypt(&self.key, &stored) {
                Ok(plain) => match crate::crypto::encrypt(&self.key, &plain) {
                    Ok(reenc) => {
                        self.file.secrets.insert(name, reenc);
                        ok += 1;
                    }
                    Err(_) => failed.push(name),
                },
                Err(_) => failed.push(name),
            }
        }
        self.persist()?;
        Ok(RotateReport { rotated: ok, failed })
    }

    /// Bulk-import `KEY=VALUE` lines from an env file. Blank lines and
    /// `# comments` are skipped. Existing keys are overwritten.
    pub fn import_env_file(&mut self, path: &Path) -> Result<ImportReport, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut imported = 0usize;
        let mut skipped: Vec<String> = Vec::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some(eq) = line.find('=') else {
                skipped.push(format!("line {}: no '='", line_no + 1));
                continue;
            };
            let key = line[..eq].trim().to_string();
            let mut value = line[eq + 1..].trim().to_string();
            // Strip surrounding quotes if present
            if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                value = value[1..value.len() - 1].to_string();
            }
            if key.is_empty() {
                skipped.push(format!("line {}: empty key", line_no + 1));
                continue;
            }
            self.set(&key, &value)?;
            imported += 1;
        }
        Ok(ImportReport { imported, skipped })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Export all secrets decrypted. Used by the CLI `export` subcommand
    /// to bootstrap an external password manager. The caller is
    /// responsible for handling the plaintext (redirect to file,
    /// shred temp, etc.) — this method does NOT print anything.
    ///
    /// Returns (name, decrypted_value) pairs in sorted order (the
    /// BTreeMap ordering preserves that for free).
    pub fn dump_plaintext(&self) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::with_capacity(self.file.secrets.len());
        for (name, stored) in &self.file.secrets {
            let value = crate::crypto::decrypt(&self.key, stored)
                .map_err(|e| format!("decrypt {name}: {e}"))?;
            out.push((name.clone(), value));
        }
        Ok(out)
    }
}

// ── Export format helpers ───────────────────────────────────────────────────
//
// Phase 3.1 product-track: give operators (and future Syntaur users) a
// one-shot way to move vault contents into a password manager they
// already use. Each format writes to stdout so piping + shred are the
// caller's job.

pub fn export_env_file(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str("# Syntaur vault export — .env format\n");
    out.push_str("# Generated by `syntaur-gateway vault export env`.\n");
    out.push_str("# Values are UNENCRYPTED — treat this file as secret.\n\n");
    for (k, v) in entries {
        // Quote value if it contains shell-unsafe characters.
        let needs_quote = v.contains(|c: char| c == ' ' || c == '\t' || c == '"' || c == '\'' || c == '#' || c == '\n');
        if needs_quote {
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
            out.push_str(&format!("{}=\"{}\"\n", k, escaped));
        } else {
            out.push_str(&format!("{}={}\n", k, v));
        }
    }
    out
}

pub fn export_csv(entries: &[(String, String)]) -> String {
    // CSV with header. Password-manager-compatible (1Password, LastPass,
    // Bitwarden all accept the "name,password" minimum).
    let mut out = String::from("name,username,password,url,notes\n");
    let escape = |s: &str| -> String {
        if s.contains(|c: char| c == ',' || c == '"' || c == '\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for (k, v) in entries {
        out.push_str(&format!(
            "Syntaur: {},,{},,{}\n",
            escape(k),
            escape(v),
            "Imported from syntaur-vault",
        ));
    }
    out
}

pub fn export_json(entries: &[(String, String)]) -> String {
    let map: std::collections::BTreeMap<String, String> = entries.iter().cloned().collect();
    serde_json::to_string_pretty(&map).unwrap_or_else(|_| "{}".to_string())
}

/// Bitwarden unencrypted JSON import format. Bitwarden accepts the
/// exported-JSON shape directly via File → Import Data → Bitwarden
/// (json). Each vault entry becomes a Login item with name + password.
pub fn export_bitwarden_json(entries: &[(String, String)]) -> String {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|(name, value)| {
            serde_json::json!({
                "type": 1, // 1 = login
                "name": format!("Syntaur: {}", name),
                "notes": format!("Imported from syntaur-vault on {}", chrono::Utc::now().format("%Y-%m-%d")),
                "favorite": false,
                "login": {
                    "username": null,
                    "password": value,
                    "uris": [],
                    "totp": null
                },
                "collectionIds": null
            })
        })
        .collect();

    let doc = serde_json::json!({
        "encrypted": false,
        "folders": [],
        "items": items,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// 1Password CSV format. 1Password's "Import → CSV" step accepts a
/// minimum of title + password columns; we also include "notes".
pub fn export_1password_csv(entries: &[(String, String)]) -> String {
    let mut out = String::from("Title,Website,Username,Password,Notes\n");
    let escape = |s: &str| -> String {
        if s.contains(|c: char| c == ',' || c == '"' || c == '\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for (k, v) in entries {
        out.push_str(&format!(
            "{},,,{},Imported from syntaur-vault\n",
            escape(&format!("Syntaur: {}", k)),
            escape(v),
        ));
    }
    out
}

/// KeePass / KeePassXC CSV format. Also the most portable fallback —
/// most password managers accept this shape.
pub fn export_keepass_csv(entries: &[(String, String)]) -> String {
    let mut out = String::from("\"Group\",\"Title\",\"Username\",\"Password\",\"URL\",\"Notes\"\n");
    let escape = |s: &str| -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    };
    for (k, v) in entries {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            escape("Syntaur"),
            escape(k),
            escape(""),
            escape(v),
            escape(""),
            escape("Imported from syntaur-vault"),
        ));
    }
    out
}

#[derive(Debug)]
pub struct RotateReport {
    pub rotated: usize,
    pub failed: Vec<String>,
}

#[derive(Debug)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: Vec<String>,
}

/// Resolve `{{vault.NAME}}` references in a config string, substituting
/// decrypted vault values. Returns the original string if nothing matches.
///
/// Missing keys are left as the literal placeholder + a warning log so
/// operators can spot unresolved references without losing the config.
pub fn resolve_refs(input: &str, vault: &Vault) -> String {
    if !input.contains("{{vault.") {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{vault.") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 8..]; // skip "{{vault."
        let Some(end) = tail.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &tail[..end];
        match vault.get(name) {
            Ok(Some(v)) => out.push_str(&v),
            _ => {
                log::warn!("[vault] reference {{vault.{}}} unresolved in config; leaving literal", name);
                out.push_str(&rest[start..start + 8 + end + 2]);
            }
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    out
}
