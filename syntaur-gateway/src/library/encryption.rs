//! Phase 6 — file-level envelope encryption for the library.
//!
//! Encrypts non-photo blobs at rest using AES-256-GCM keyed off the
//! gateway's master key (`~/.syntaur/master.key`). Photos stay
//! plaintext for two reasons:
//!
//!   1. Photos are the bulk of the library by byte count; encrypting
//!      them would push the per-read latency above acceptable for the
//!      photo grid + face-detect pipeline.
//!   2. The smart-speaker streaming roadmap (DLNA/AirPlay) needs
//!      direct file-system access on the host to publish photos as
//!      a media library — encryption would require a decryption proxy
//!      sitting in front of that, which isn't worth the engineering.
//!
//! Documents (receipts, statements, tax forms, manuals, personal docs)
//! are exactly the surface where at-rest encryption matters: they're
//! sensitive, low-volume, and never streamed.
//!
//! Migration is graceful: `decrypt_if_needed` looks at the magic
//! header on read. New ingests get encrypted; existing plaintext
//! files keep working until they're touched (next read can rewrite
//! to encrypted form lazily). The settings UI exposes a one-shot
//! "encrypt existing documents" sweep.
//!
//! ## SQLCipher / full-DB encryption (deferred)
//!
//! The original Phase-6 directive asked for SQLCipher swap on the
//! main `index.db`. That's a one-way DB rewrite affecting every
//! `Connection::open()` call across the codebase plus a migration
//! tool to re-encrypt the existing plaintext DB. It deserves its
//! own session with proper test coverage — flipping the cargo
//! feature today would brick prod if the migration failed mid-flight
//! or if any caller forgot to apply `PRAGMA key`. Tracked in vault
//! `projects/syntaur_doc_intake_storage.md` as Phase 6.5. File-level
//! envelope encryption shipped here is the immediately-useful slice.

use anyhow::{anyhow, Result};

/// Returns true when files of this kind should be encrypted at rest.
///
/// Photos return false; everything else returns true. Called from the
/// ingest path before write and from the serve path before read.
pub fn should_encrypt_kind(kind: &str) -> bool {
    !matches!(kind, "photo" | "video" | "audio")
}

/// Encrypt `bytes` if `kind` warrants it. Pass-through for photos.
pub fn maybe_encrypt(
    key: &aes_gcm::Key<aes_gcm::Aes256Gcm>,
    kind: &str,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    if !should_encrypt_kind(kind) {
        return Ok(bytes.to_vec());
    }
    crate::crypto::encrypt_blob(key, bytes).map_err(|e| anyhow!("encrypt: {e}"))
}

/// Decrypt `bytes` if it carries the magic header. Pass-through
/// for plaintext (legacy migration).
pub fn decrypt_if_needed(
    key: &aes_gcm::Key<aes_gcm::Aes256Gcm>,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    if !crate::crypto::is_encrypted_blob(bytes) {
        return Ok(bytes.to_vec());
    }
    crate::crypto::decrypt_blob(key, bytes).map_err(|e| anyhow!("decrypt: {e}"))
}
