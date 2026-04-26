//! Fabric generation + the serializable handle persisted to disk.
//!
//! A `FabricHandle` is the minimum a future commissioner needs to:
//! 1. Sign NOCs for devices we're commissioning (via [`sign_device_noc`])
//! 2. Establish CASE sessions with already-commissioned devices using
//!    a stable controller identity (the persisted controller NOC +
//!    secret key, added 2026-04-26 for plug control after commission).

use chrono::{DateTime, Utc};
use rs_matter::commissioner::FabricCredentials;
use rs_matter::crypto::{test_only_crypto, Crypto, SecretKey, SigningSecretKey};
use serde::{Deserialize, Serialize};

use crate::MatterFabricError;

/// Serializable representation of a Syntaur-owned Matter fabric. Kept
/// plaintext-small so the whole thing fits in one AEAD envelope at
/// rest.
///
/// **Field history**:
/// - v1 (2026-04-22): label/fabric_id/controller_node_id/vendor_id +
///   root_cert_hex/ca_secret_key_hex/ipk_hex
/// - v2 (2026-04-26): controller_noc_hex + controller_secret_key_hex
///   added so post-commissioning CASE handshakes use a stable controller
///   identity. Older fabrics that lack these will need to be re-minted +
///   devices re-commissioned (the per-device fabric record on each device
///   was written under the old ephemeral controller NOC, which we can't
///   reconstitute).
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
    /// Hex-encoded Matter-TLV NOC for the *controller* (= our admin
    /// node identity on this fabric, controller_node_id=1). Persisted
    /// so post-commissioning CASE handshakes present the same NOC the
    /// device first saw at CommissioningComplete time. Without this
    /// the device silently drops Sigma1 from a fresh-controller-NOC
    /// initiator. Optional for backward-compat with v1 fabric files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_noc_hex: Option<String>,
    /// Hex-encoded raw 32-byte P-256 scalar for the controller's signing
    /// key (the keypair embedded in `controller_noc_hex`). Used to sign
    /// CASE Sigma3. Pairs with `controller_noc_hex`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_secret_key_hex: Option<String>,
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
    /// Whether the fabric file has a persisted controller NOC. Old
    /// (v1) fabrics show `false`; their devices can't be controlled
    /// post-commission and need re-commissioning under a v2 fabric.
    pub has_controller_noc: bool,
}

impl FabricHandle {
    /// Generate a fresh fabric. Uses rs-matter's `FabricCredentials`
    /// plus a direct read of the CA secret key + IPK so we can
    /// serialize both back out for persistence.
    ///
    /// Also signs and persists the controller's own NOC at mint time
    /// (controller_node_id=1, our admin identity on this fabric). This
    /// is what lets post-commissioning CASE handshakes work — the
    /// controller's NOC + secret stays stable across binary invocations.
    pub fn new(label: impl Into<String>) -> Result<Self, MatterFabricError> {
        use rs_matter::cert::builder::{RcacBuilder, SubjectDN, Validity};
        use rs_matter::commissioner::NocGenerator;
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

        // rcac_id is hardcoded to 1 to match NocGenerator::from_root_ca
        // which always stamps NOC.Issuer.MatterRcacId = 1. Eve verifies
        // NOC.Issuer == RCAC.Subject byte-for-byte, so they MUST match.
        let rcac_id: u64 = 1;
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
                    Validity::new(0x10000, 0xFFFFFFFF),
                    &ca_public,
                    &ca_secret,
                    &serial,
                )
                .map_err(|e| MatterFabricError::Matter(format!("RcacBuilder::build: {e:?}")))?
        };
        let rcac = cert_buf[..cert_len].to_vec();

        // ── Sign the controller's own NOC at mint time ──
        // controller_node_id = 1. The key here will be persisted and
        // reused for every CASE handshake (commissioning + control).
        // Eve/Meross record the controller NOC's identity at first CASE
        // handshake; subsequent handshakes must present the same NOC
        // or they're silently dropped.
        let controller_secret = crypto
            .generate_secret_key()
            .map_err(|e| MatterFabricError::Matter(format!("controller secret: {e:?}")))?;
        let mut controller_csr_buf = [0u8; 256];
        let controller_csr = controller_secret
            .csr(&mut controller_csr_buf)
            .map_err(|e| MatterFabricError::Matter(format!("controller csr: {e:?}")))?;
        let mut controller_secret_canon = CanonPkcSecretKey::new();
        controller_secret
            .write_canon(&mut controller_secret_canon)
            .map_err(|e| MatterFabricError::Matter(format!("write_canon controller: {e:?}")))?;
        let mut controller_secret_scalar = [0u8; 32];
        controller_secret_scalar.copy_from_slice(controller_secret_canon.access());

        // Use the same CA scalar to sign the controller NOC.
        let ca_secret_for_noc = CanonPkcSecretKey::from(&ca_scalar);
        let mut noc_gen = NocGenerator::from_root_ca(&crypto, ca_secret_for_noc, &rcac, fabric_id, 1)
            .map_err(|e| MatterFabricError::Matter(format!("NocGenerator::from_root_ca: {e:?}")))?;
        let controller_creds = noc_gen
            .generate_noc(&crypto, controller_csr, /* node_id */ 1, /* cat_ids */ &[])
            .map_err(|e| MatterFabricError::Matter(format!("generate controller NOC: {e:?}")))?;

        Ok(FabricHandle {
            label,
            fabric_id,
            controller_node_id: 1,
            vendor_id: 0xFFF1,
            root_cert_hex: hex::encode(&rcac),
            ca_secret_key_hex: hex::encode(ca_scalar),
            ipk_hex: hex::encode(ipk_bytes),
            controller_noc_hex: Some(hex::encode(&controller_creds.noc)),
            controller_secret_key_hex: Some(hex::encode(controller_secret_scalar)),
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
            has_controller_noc: self.controller_noc_hex.is_some()
                && self.controller_secret_key_hex.is_some(),
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
