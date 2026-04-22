//! Device NOC signing for commissioning-time (Phase 3 will wire this
//! into the full state machine). Phase 1 exposes the primitive so it
//! can be unit-tested standalone.

use rs_matter::commissioner::NocGenerator;
use rs_matter::crypto::{test_only_crypto, CanonPkcSecretKey};

use crate::fabric::FabricHandle;
use crate::MatterFabricError;

/// Sign a NOC for a device being commissioned.
///
/// Given:
///   - our fabric's CA (keypair + RCAC)
///   - the device-provided CSR (DER-encoded, from CSRRequest response)
///   - the node ID we want to assign on our fabric
///   - optional CATs (CASE Authenticated Tags)
///
/// Returns:
///   - NOC TLV bytes to send to the device via `AddNOC`
pub fn sign_device_noc(
    handle: &FabricHandle,
    csr_der: &[u8],
    node_id: u64,
    cat_ids: &[u32],
) -> Result<Vec<u8>, MatterFabricError> {
    let crypto = test_only_crypto();

    let mut ca_scalar = [0u8; 32];
    let decoded = hex::decode(&handle.ca_secret_key_hex)
        .map_err(|e| MatterFabricError::Matter(format!("ca_secret_key_hex decode: {e}")))?;
    if decoded.len() != 32 {
        return Err(MatterFabricError::Matter(format!(
            "ca_secret_key_hex wrong length: {}B",
            decoded.len()
        )));
    }
    ca_scalar.copy_from_slice(&decoded);
    let ca_secret = CanonPkcSecretKey::from(&ca_scalar);

    let rcac = hex::decode(&handle.root_cert_hex)
        .map_err(|e| MatterFabricError::Matter(format!("root_cert_hex decode: {e}")))?;

    let mut gen = NocGenerator::from_root_ca(
        &crypto,
        ca_secret,
        &rcac,
        handle.fabric_id,
        /* rcac_id = */ 1,
    )
    .map_err(|e| MatterFabricError::Matter(format!("NocGenerator::from_root_ca: {e:?}")))?;

    let noc_creds = gen
        .generate_noc(&crypto, csr_der, node_id, cat_ids)
        .map_err(|e| MatterFabricError::Matter(format!("generate_noc: {e:?}")))?;

    Ok(noc_creds.noc.to_vec())
}
