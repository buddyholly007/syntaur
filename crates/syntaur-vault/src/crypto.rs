//! KDF (argon2id) + AEAD (ChaCha20-Poly1305) primitives.
//!
//! Split out so the agent + the init/change-passphrase paths can all
//! share the same parameter set. Bumping the argon2 parameters or the
//! cipher choice is a format-version bump (see `FORMAT_VERSION`).

use anyhow::{anyhow, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use zeroize::Zeroize;

/// Size of the argon2 output, in bytes. 32 matches the cipher's key
/// length (ChaCha20 = 256-bit).
pub const KEY_LEN: usize = 32;
/// Size of the argon2 salt, in bytes. 32 bytes (256 bits) is well past
/// any collision risk for the number of vaults Sean will ever create.
pub const SALT_LEN: usize = 32;
/// Size of the ChaCha20-Poly1305 nonce, in bytes.
pub const NONCE_LEN: usize = 12;

/// Argon2id parameters. Tuned so unlock takes ~500ms on a modern x86
/// laptop; strong enough that brute-forcing the vault passphrase is
/// impractical without the actual passphrase.
fn argon2_params() -> Params {
    // m_cost = 64 MiB (65536 KiB), t_cost = 3 iterations, parallelism = 4.
    // These are OWASP 2024 recommended minimums for argon2id.
    Params::new(65536, 3, 4, Some(KEY_LEN))
        .expect("argon2 params invariant")
}

/// Generate a fresh random salt. Called once at `init`; persisted in
/// the vault header.
pub fn new_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// Generate a fresh random nonce. Called on every `write` — never
/// reuse a nonce under the same key (AEAD security breaks otherwise).
pub fn new_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Derive a 32-byte master key from a passphrase + salt. The output
/// buffer is writable so the caller can place it wherever it wants
/// (typically the agent's long-lived `Zeroizing` wrapper).
pub fn derive_key(passphrase: &[u8], salt: &[u8], out: &mut [u8; KEY_LEN]) -> Result<()> {
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params());
    argon
        .hash_password_into(passphrase, salt, out)
        .map_err(|e| anyhow!("argon2 kdf failed: {e}"))?;
    Ok(())
}

/// Encrypt a plaintext with the given key + nonce. Returns
/// ciphertext-plus-tag (16-byte Poly1305 tag appended internally by
/// the crate).
pub fn encrypt(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .map_err(|e| anyhow!("aead encrypt: {e}"))
}

/// Decrypt a ciphertext-plus-tag with the given key + nonce. Returns
/// an error if the tag doesn't verify (tampered file or wrong key).
pub fn decrypt(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow!("aead decrypt failed — wrong passphrase or tampered vault file"))
}

/// RAII wrapper around a derived key so it's zeroed on drop. The
/// agent holds one of these for its full lifetime.
pub struct MasterKey(pub [u8; KEY_LEN]);

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MasterKey {
    pub fn derive_from_passphrase(passphrase: &[u8], salt: &[u8]) -> Result<Self> {
        let mut k = [0u8; KEY_LEN];
        derive_key(passphrase, salt, &mut k)?;
        Ok(Self(k))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let salt = new_salt();
        let mut key = [0u8; KEY_LEN];
        derive_key(b"correct horse battery staple", &salt, &mut key).unwrap();
        let nonce = new_nonce();
        let plaintext = b"some secret payload";
        let ct = encrypt(&key, &nonce, plaintext).unwrap();
        let pt = decrypt(&key, &nonce, &ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let salt = new_salt();
        let mut key_a = [0u8; KEY_LEN];
        let mut key_b = [0u8; KEY_LEN];
        derive_key(b"right", &salt, &mut key_a).unwrap();
        derive_key(b"wrong", &salt, &mut key_b).unwrap();
        let nonce = new_nonce();
        let ct = encrypt(&key_a, &nonce, b"payload").unwrap();
        assert!(decrypt(&key_b, &nonce, &ct).is_err());
    }
}
