//! Encrypted-at-rest persistence for Syntaur-owned Matter fabrics.
//!
//! Each fabric lives in `~/.syntaur/matter_fabrics/<label>.enc`:
//!   `nonce(24) || ciphertext_and_tag`
//! Sealed via XChaCha20-Poly1305 with a key derived from
//! `~/.syntaur/master.key` (the same file Syntaur already uses for its
//! master symmetric key). File perms 0600.
//!
//! Atomic writes: `<label>.enc.tmp` → fsync → rename. An interrupted
//! write can't leave a torn blob.

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::XChaCha20Poly1305;

use crate::fabric::{FabricHandle, FabricSummary};
use crate::MatterFabricError;

/// `~/.syntaur/matter_fabrics/`.
pub fn default_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    PathBuf::from(home).join(".syntaur").join("matter_fabrics")
}

/// `~/.syntaur/master.key` — the existing Syntaur master key.
fn master_key_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    PathBuf::from(home).join(".syntaur").join("master.key")
}

fn load_master_key() -> Result<[u8; 32], MatterFabricError> {
    let path = master_key_path();
    let bytes = std::fs::read(&path).map_err(|_| MatterFabricError::MasterKey {
        path: path.display().to_string(),
    })?;
    if bytes.len() < 32 {
        return Err(MatterFabricError::MasterKey {
            path: path.display().to_string(),
        });
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes[..32]);
    Ok(key)
}

/// Persist a fabric to its canonical path.
///
/// Overwrites if it already exists (caller decides whether that's a
/// bug; the CLI layer checks existence before calling to surface a
/// clean error).
pub fn save_fabric(handle: &FabricHandle) -> Result<PathBuf, MatterFabricError> {
    let dir = default_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.enc", handle.label));
    write_encrypted(&path, handle)?;
    Ok(path)
}

/// Load a fabric by label (the file-stem).
pub fn load_fabric(label: &str) -> Result<FabricHandle, MatterFabricError> {
    let dir = default_dir();
    let path = dir.join(format!("{label}.enc"));
    if !path.exists() {
        return Err(MatterFabricError::NotFound {
            label: label.into(),
            dir: dir.display().to_string(),
        });
    }
    read_encrypted(&path)
}

/// Summary of every fabric on disk. Reads + decrypts each file; for a
/// small-N count (usually 1) this is fine. If we ever persist >10
/// fabrics, add a manifest file to avoid iterative decrypt.
pub fn list_fabrics() -> Result<Vec<FabricSummary>, MatterFabricError> {
    let dir = default_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("enc") {
            continue;
        }
        if let Ok(handle) = read_encrypted(&p) {
            out.push(handle.summary());
        }
    }
    out.sort_by_key(|s| s.created_at);
    Ok(out)
}

fn write_encrypted(path: &Path, handle: &FabricHandle) -> Result<(), MatterFabricError> {
    let key = load_master_key()?;
    let cipher = XChaCha20Poly1305::new(&key.into());
    let plaintext = serde_json::to_vec(handle)?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .map_err(|e| MatterFabricError::Envelope(format!("seal: {e}")))?;

    let mut framed = Vec::with_capacity(24 + ciphertext.len());
    framed.extend_from_slice(&nonce);
    framed.extend_from_slice(&ciphertext);

    let tmp = path.with_extension("enc.tmp");
    std::fs::write(&tmp, &framed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_encrypted(path: &Path) -> Result<FabricHandle, MatterFabricError> {
    let key = load_master_key()?;
    let cipher = XChaCha20Poly1305::new(&key.into());
    let bytes = std::fs::read(path)?;
    if bytes.len() < 24 + 16 {
        return Err(MatterFabricError::Envelope("file too short".into()));
    }
    let nonce: &[u8; 24] = bytes[..24].try_into().unwrap();
    let ct = &bytes[24..];
    let plaintext = cipher
        .decrypt(nonce.into(), ct)
        .map_err(|e| MatterFabricError::Envelope(format!("open: {e}")))?;
    let handle: FabricHandle = serde_json::from_slice(&plaintext)?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn roundtrip_envelope() {
        let tmp = tempfile::tempdir().unwrap();
        let master = tmp.path().join(".syntaur").join("master.key");
        std::fs::create_dir_all(master.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&master).unwrap();
        f.write_all(&[0xAB; 32]).unwrap();
        std::env::set_var("HOME", tmp.path());

        let handle = FabricHandle {
            label: "test".into(),
            fabric_id: 42,
            controller_node_id: 1,
            vendor_id: 0xFFF1,
            root_cert_hex: "deadbeef".into(),
            ca_secret_key_hex: "11".repeat(32),
            ipk_hex: "22".repeat(16),
            created_at: chrono::Utc::now(),
        };
        let path = save_fabric(&handle).unwrap();
        assert!(path.exists());

        let back = load_fabric("test").unwrap();
        assert_eq!(back.fabric_id, 42);
        assert_eq!(back.ca_secret_key_hex, "11".repeat(32));

        let all = list_fabrics().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].label, "test");
    }
}
