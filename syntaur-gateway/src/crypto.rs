//! AES-256-GCM encryption for secrets at rest (OAuth tokens, etc.).
//!
//! On first use, generates a 256-bit master key and writes it to
//! `~/.syntaur/master.key` with 0600 permissions.  Subsequent calls
//! load the existing key.
//!
//! Ciphertext format: `nonce (12 bytes) || ciphertext+tag`.
//! Stored in the database as hex-encoded strings prefixed with `enc:`.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore, Key, Nonce,
};
use log::{info, warn};
use std::path::{Path, PathBuf};

const KEY_FILE: &str = "master.key";
const ENC_PREFIX: &str = "enc:";

/// Load or generate the master key from `data_dir/master.key`.
pub fn load_or_create_key(data_dir: &Path) -> Result<Key<Aes256Gcm>, String> {
    let path = data_dir.join(KEY_FILE);
    if path.exists() {
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("read master key: {}", e))?;
        if bytes.len() != 32 {
            return Err(format!(
                "master key file is {} bytes, expected 32",
                bytes.len()
            ));
        }
        let key = Key::<Aes256Gcm>::from_slice(&bytes).clone();
        info!("[crypto] loaded master key from {}", path.display());
        Ok(key)
    } else {
        let key = Aes256Gcm::generate_key(OsRng);
        std::fs::write(&path, key.as_slice())
            .map_err(|e| format!("write master key: {}", e))?;
        // Set file permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms)
                .map_err(|e| format!("chmod master key: {}", e))?;
        }
        info!("[crypto] generated new master key at {}", path.display());
        Ok(key)
    }
}

/// Encrypt a plaintext string → hex-encoded `enc:<nonce><ciphertext>`.
pub fn encrypt(key: &Key<Aes256Gcm>, plaintext: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("encrypt: {}", e))?;

    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{}{}", ENC_PREFIX, hex::encode(&blob)))
}

/// Decrypt a hex-encoded `enc:<nonce><ciphertext>` → plaintext string.
/// If the value does NOT start with `enc:`, it's treated as legacy
/// plaintext and returned as-is (for transparent migration).
pub fn decrypt(key: &Key<Aes256Gcm>, stored: &str) -> Result<String, String> {
    if !stored.starts_with(ENC_PREFIX) {
        // Legacy plaintext — return as-is for migration
        return Ok(stored.to_string());
    }
    let hex_str = &stored[ENC_PREFIX.len()..];
    let blob = hex::decode(hex_str)
        .map_err(|e| format!("decrypt hex: {}", e))?;
    if blob.len() < 12 {
        return Err("ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decrypt: {}", e))?;
    String::from_utf8(plaintext).map_err(|e| format!("decrypt utf8: {}", e))
}

/// Returns true if the value is already encrypted.
pub fn is_encrypted(stored: &str) -> bool {
    stored.starts_with(ENC_PREFIX)
}

// ── Binary envelope (for library file BLOBs) ──────────────────────────
//
// Format: `SYNX1` (5-byte magic) + `\0` + `nonce (12B)` + `ciphertext+tag`.
// The leading magic lets the read path differentiate encrypted blobs
// from legacy plaintext, so we can flip the "encrypt at rest" toggle
// without rewriting every existing file. Detection is constant-time:
// we just look at the first six bytes.

/// Wire-format magic bytes (6) prefix on encrypted file blobs.
pub const FILE_MAGIC: &[u8] = b"SYNX1\0";

/// Encrypt a binary blob with the master key. Output starts with
/// `FILE_MAGIC`; nonce is randomized per call. Used for library file
/// BLOBs where the kind warrants at-rest protection (see
/// `library::encryption::should_encrypt_kind`).
pub fn encrypt_blob(key: &Key<Aes256Gcm>, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("encrypt_blob: {}", e))?;
    let mut out = Vec::with_capacity(FILE_MAGIC.len() + 12 + ciphertext.len());
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a binary blob. Returns the original plaintext if the input
/// starts with `FILE_MAGIC`, otherwise returns the input unchanged
/// (legacy plaintext bypass).
pub fn decrypt_blob(key: &Key<Aes256Gcm>, stored: &[u8]) -> Result<Vec<u8>, String> {
    if stored.len() < FILE_MAGIC.len() || !stored.starts_with(FILE_MAGIC) {
        return Ok(stored.to_vec());
    }
    let body = &stored[FILE_MAGIC.len()..];
    if body.len() < 12 { return Err("encrypted blob too short".into()); }
    let (nonce_bytes, ciphertext) = body.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decrypt_blob: {}", e))
}

/// True iff `bytes` starts with `FILE_MAGIC`.
pub fn is_encrypted_blob(bytes: &[u8]) -> bool {
    bytes.len() >= FILE_MAGIC.len() && bytes.starts_with(FILE_MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let key = load_or_create_key(dir.path()).unwrap();
        let secret = "my-super-secret-oauth-token-12345";
        let encrypted = encrypt(&key, secret).unwrap();
        assert!(encrypted.starts_with("enc:"));
        assert_ne!(encrypted, secret);
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, secret);
    }

    #[test]
    fn legacy_plaintext_passthrough() {
        let dir = tempdir().unwrap();
        let key = load_or_create_key(dir.path()).unwrap();
        let plain = "old-unencrypted-token";
        let result = decrypt(&key, plain).unwrap();
        assert_eq!(result, plain);
    }

    #[test]
    fn key_persistence() {
        let dir = tempdir().unwrap();
        let k1 = load_or_create_key(dir.path()).unwrap();
        let k2 = load_or_create_key(dir.path()).unwrap();
        assert_eq!(k1.as_slice(), k2.as_slice());
    }
}
