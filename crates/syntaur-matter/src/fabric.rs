//! Fabric generation + the serializable handle persisted to disk.
//!
//! A `FabricHandle` is the minimum a future commissioner needs to:
//! 1. Sign NOCs for devices we're commissioning (via [`sign_device_noc`])
//! 2. Establish CASE sessions with already-commissioned devices (via
//!    the existing `syntaur-gateway::matter_direct` runtime — the
//!    serialized `SyntaurFabricFile` shape is a strict superset of
//!    what that code already reads)

use chrono::{DateTime, Utc};
use rs_matter::commissioner::FabricCredentials;
use rs_matter::crypto::{test_only_crypto, Crypto, SecretKey, SigningSecretKey};
use serde::{Deserialize, Serialize};

use crate::MatterFabricError;

/// Serializable representation of a Syntaur-owned Matter fabric. Kept
/// plaintext-small so the whole thing fits in one AEAD envelope at
/// rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FabricHandle {
    /// User-facing label (also the filename stem). `[a-zA-Z0-9_-]+`.
    pub label: String,
    /// Matter fabric ID — the 64-bit value embedded in NOC subjects.
    /// We pick a random non-zero u64 per fabric so a single Syntaur
    /// install could theoretically run multiple isolated fabrics
    /// without collision.
    pub fabric_id: u64,
    /// The Syntaur controller's own node ID on this fabric. Fixed at
    /// 1 for a given fabric (we don't share admin across multiple
    /// Syntaur instances yet).
    pub controller_node_id: u64,
    /// Matter vendor ID we claim as commissioner. `0xFFF1` is the
    /// "test vendor" value; anything in `0xFFF1..=0xFFF4` is safe for
    /// uncertified use.
    pub vendor_id: u16,
    /// Hex-encoded Matter-TLV RCAC (self-signed root cert).
    pub root_cert_hex: String,
    /// Hex-encoded raw 32-byte P-256 scalar for the CA signing key.
    /// This is `ExampleOpCredsCAKey0`-shaped — used to sign device
    /// NOCs during commissioning.
    pub ca_secret_key_hex: String,
    /// Hex-encoded 16-byte IPK epoch key 0. Used as the key-derivation
    /// input when computing CASE destination_ids + operational keys.
    pub ipk_hex: String,
    /// When the fabric was created.
    pub created_at: DateTime<Utc>,
}

/// Public summary for UI + CLI — no secrets, safe to print.
#[derive(Debug, Clone, Serialize)]
pub struct FabricSummary {
    pub label: String,
    pub fabric_id: u64,
    pub controller_node_id: u64,
    pub vendor_id: u16,
    pub created_at: DateTime<Utc>,
    /// First 16 hex chars of sha256(RCAC) — useful for visual
    /// "are these the same fabric?" comparisons.
    pub rcac_fingerprint: String,
}

impl FabricHandle {
    /// Generate a fresh fabric. Uses rs-matter's `FabricCredentials`
    /// plus a direct read of the CA secret key + IPK so we can
    /// serialize both back out for persistence.
    pub fn new(label: impl Into<String>) -> Result<Self, MatterFabricError> {
        use rs_matter::cert::builder::{RcacBuilder, SubjectDN, Validity};
        use rs_matter::crypto::{
            test_only_crypto, CanonPkcPublicKey, CanonPkcSecretKey, RngCore,
        };

        let label = validate_label(&label.into())?;
        let crypto = test_only_crypto();

        // Random fabric_id (non-zero, 60-ish bits of entropy with high
        // nibble masked for legibility).
        let mut fab_bytes = [0u8; 8];
        let mut rand = crypto
            .rand()
            .map_err(|e| MatterFabricError::Matter(format!("rand(): {e:?}")))?;
        loop {
            rand.fill_bytes(&mut fab_bytes);
            fab_bytes[0] &= 0x0F;
            let n = u64::from_be_bytes(fab_bytes);
            if n != 0 {
                break;
            }
        }
        let fabric_id = u64::from_be_bytes(fab_bytes);

        // Generate ONE CA keypair that will both sign the RCAC AND sign
        // future NOCs. Earlier versions of this fabric used FabricCredentials::new
        // for the RCAC (random CA #1) and a separately-generated key for NOC
        // signing (CA #2). The chain didn't validate, which broke CASE
        // handshakes. Verified 2026-04-25 against Eve Energy.
        let ca_secret = crypto
            .generate_secret_key()
            .map_err(|e| MatterFabricError::Matter(format!("generate_secret_key: {e:?}")))?;
        let ca_public = ca_secret
            .pub_key()
            .map_err(|e| MatterFabricError::Matter(format!("pub_key: {e:?}")))?;
        let mut ca_secret_canon = CanonPkcSecretKey::new();
        ca_secret
            .write_canon(&mut ca_secret_canon)
            .map_err(|e| MatterFabricError::Matter(format!("write_canon ca: {e:?}")))?;
        let mut ca_scalar = [0u8; 32];
        ca_scalar.copy_from_slice(ca_secret_canon.access());

        // Random rcac_id and serial.
        let mut rcac_id_bytes = [0u8; 8];
        rand.fill_bytes(&mut rcac_id_bytes);
        let rcac_id = u64::from_be_bytes(rcac_id_bytes) | 1; // ensure non-zero
        let mut serial = [0u8; 8];
        rand.fill_bytes(&mut serial);

        // Random IPK.
        let mut ipk_bytes = [0u8; 16];
        rand.fill_bytes(&mut ipk_bytes);

        // Build the RCAC with our CA keypair self-signing.
        let mut cert_buf = [0u8; 1024];
        let cert_len = {
            let mut builder = RcacBuilder::new(&mut cert_buf);
            builder
                .build(
                    &crypto,
                    SubjectDN::rcac(fabric_id, rcac_id),
                    Validity::new(762624000, 0xFFFFFFFE),
                    &ca_public,
                    &ca_secret,
                    &serial,
                )
                .map_err(|e| MatterFabricError::Matter(format!("RcacBuilder::build: {e:?}")))?
        };
        let rcac = cert_buf[..cert_len].to_vec();

        Ok(FabricHandle {
            label,
            fabric_id,
            controller_node_id: 1,
            vendor_id: 0xFFF1,
            root_cert_hex: hex::encode(&rcac),
            ca_secret_key_hex: hex::encode(ca_scalar),
            ipk_hex: hex::encode(ipk_bytes),
            created_at: Utc::now(),
        })
    }

    pub fn summary(&self) -> FabricSummary {
        use sha2_compat::sha256;
        let rcac = hex::decode(&self.root_cert_hex).unwrap_or_default();
        let d = sha256(&rcac);
        FabricSummary {
            label: self.label.clone(),
            fabric_id: self.fabric_id,
            controller_node_id: self.controller_node_id,
            vendor_id: self.vendor_id,
            created_at: self.created_at,
            rcac_fingerprint: hex::encode(&d[..8]),
        }
    }
}

fn validate_label(label: &str) -> Result<String, MatterFabricError> {
    if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(MatterFabricError::BadLabel { label: label.into() });
    }
    Ok(label.into())
}

// Tiny SHA-256 wrapper so we don't pull the full `sha2` dep; rs-matter
// already has it transitively so this just fronts it.
mod sha2_compat {
    pub fn sha256(data: &[u8]) -> [u8; 32] {
        use rs_matter::crypto::{test_only_crypto, Crypto, Digest};
        let crypto = test_only_crypto();
        let mut h = crypto.hash().unwrap();
        let _ = h.update(data);
        let mut out = rs_matter::crypto::CryptoSensitive::<32>::new();
        let _ = h.finish(&mut out);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(out.access());
        arr
    }
}
