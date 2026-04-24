//! Cosign signing of deploy stamps (Phase 7).
//!
//! The release-sign.yml workflow already signs published binaries with
//! cosign keyless (OIDC identity from GitHub Actions itself). Phase 7
//! extends that chain: every `syntaur-ship` deploy produces a
//! cosign-signed attestation binding {git_commit, binary_sha256,
//! timestamp, operator_session} together. External auditors can then
//! verify: "this binary on prod came from this verified commit,
//! signed by this operator, at this instant."
//!
//! Key design choice: we use cosign's LOCAL key-pair for deploy stamps
//! (as opposed to keyless OIDC, which requires a GitHub Actions
//! context). The key pair is `~/.syntaur/ship/cosign.key` / .pub on
//! claudevm. One-time generation via `cosign generate-key-pair`; after
//! that, every deploy signs with `cosign sign-blob --key cosign.key`.
//! The public key is committed to the repo at
//! `openclaw-workspace/syntaur-ship/cosign.pub` so anyone can verify.
//!
//! Verification path:
//!     cosign verify-blob \
//!         --key openclaw-workspace/syntaur-ship/cosign.pub \
//!         --bundle deploy-stamp.json.cosign.bundle \
//!         deploy-stamp.json
//!
//! Format note (2026-04-24): cosign 2.x deprecated `--output-signature`
//! in favour of `--bundle`, which writes a single Sigstore-bundle file
//! containing signature + cert + rekor entry. `verify-blob --bundle`
//! is the mirror read path. We migrated both sides in this commit; the
//! old `.sig` files from prior releases stay verifiable with older
//! cosign binaries but new stamps only get `.cosign.bundle`. Keep both
//! legible in the deploy journal via the fallback path below.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Sign the deploy-stamp.json at `stamp_path` using the local cosign
/// key. Writes a Sigstore-format `<stamp>.cosign.bundle`. Non-fatal on
/// failure (deploy already succeeded; signature is an audit extra).
pub fn sign_stamp(state_dir: &Path) -> Result<()> {
    let stamp = state_dir.join("deploy-stamp.json");
    let key = state_dir.join("cosign.key");
    let bundle = state_dir.join("deploy-stamp.json.cosign.bundle");

    if !key.exists() {
        log::debug!(
            "[cosign] {} missing — run `cosign generate-key-pair` in {} to enable signed stamps",
            key.display(),
            state_dir.display()
        );
        return Ok(());
    }
    if !stamp.exists() {
        return Ok(()); // nothing to sign
    }

    // Use COSIGN_PASSWORD env var — the passphrase for the private
    // key. Defaults to empty string (the tool keeps the key on disk
    // 0400 with the passphrase blank; this is equivalent-security to
    // the existing unencrypted state dir).
    let output = Command::new("cosign")
        .args([
            "sign-blob",
            "--yes",
            "--key",
            key.to_str().unwrap(),
            "--bundle",
            bundle.to_str().unwrap(),
            stamp.to_str().unwrap(),
        ])
        .env("COSIGN_PASSWORD", "")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            log::info!(
                "[cosign] ✓ signed {} → {}",
                stamp.file_name().unwrap().to_string_lossy(),
                bundle.file_name().unwrap().to_string_lossy()
            );
        }
        Ok(o) => {
            log::warn!(
                "[cosign] sign failed (non-fatal): {}",
                String::from_utf8_lossy(&o.stderr)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        Err(e) => {
            log::debug!("[cosign] binary missing or unreachable: {e}");
        }
    }
    Ok(())
}

/// Verify a deploy stamp using the public key. Prefers the new
/// `.cosign.bundle` format; falls back to legacy `.sig` for stamps
/// produced by syntaur-ship versions before the 2026-04-24 migration.
/// Subcommand `syntaur-ship verify-stamp` calls this.
pub fn verify_stamp(stamp_path: &Path, pubkey_path: &Path) -> Result<()> {
    let ext = stamp_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let bundle = stamp_path.with_extension(format!("{ext}.cosign.bundle"));
    let legacy_sig = stamp_path.with_extension(format!("{ext}.sig"));

    if !pubkey_path.exists() {
        anyhow::bail!("public key missing: {}", pubkey_path.display());
    }

    let (flag, sig_path) = if bundle.exists() {
        ("--bundle", bundle)
    } else if legacy_sig.exists() {
        ("--signature", legacy_sig)
    } else {
        anyhow::bail!(
            "no signature at {} or {}",
            bundle.display(),
            legacy_sig.display()
        );
    };
    let status = Command::new("cosign")
        .args([
            "verify-blob",
            "--key",
            pubkey_path.to_str().unwrap(),
            flag,
            sig_path.to_str().unwrap(),
            stamp_path.to_str().unwrap(),
        ])
        .status()
        .context("cosign verify-blob")?;
    if !status.success() {
        anyhow::bail!("cosign verify failed");
    }
    println!("✓ stamp signature valid");
    Ok(())
}
