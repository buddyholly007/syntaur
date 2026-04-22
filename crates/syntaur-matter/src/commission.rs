//! Commissioning state machine — Path C Phase 3.
//!
//! Drives the 8 Matter IM invokes required to bring a factory-fresh
//! device onto our Syntaur fabric after a PASE session has been
//! established:
//!
//! ```text
//! ArmFailSafe(60s)
//!   ↓
//! SetRegulatoryConfig(Outdoor, "US")
//!   ↓
//! CSRRequest(nonce)  → receive device's CSR
//!   ↓
//! sign_device_noc(csr, node_id)  → our NOC (signed with fabric CA)
//!   ↓
//! AddTrustedRootCertificate(fabric_rcac)
//!   ↓
//! AddNOC(noc, None, ipk, case_admin_subject, vendor_id)
//!   ↓
//! (WiFi-only devices)
//!   AddOrUpdateWiFiNetwork(ssid, psk)
//!   ConnectNetwork(ssid)
//!   ↓
//! CommissioningComplete
//! ```
//!
//! The driver is generic over the "exchange-after-PASE" — it takes a
//! `Commissioner` caller-supplied object implementing
//! [`CommissionExchange`] so it can plug into either Phase 4's BLE
//! transport OR a temporary IP-PASE harness for testing.

use rand::RngCore;
use rs_matter::transport::exchange::Exchange;

use crate::fabric::FabricHandle;
use crate::sign::sign_device_noc;
use crate::tlv_build::{
    add_noc, add_or_update_wifi_network, add_trusted_root_certificate, arm_fail_safe,
    commissioning_complete, connect_network, csr_request, set_regulatory_config,
};
use crate::MatterFabricError;

// ── Matter cluster + command IDs used here ────────────────────────────

pub const CLUSTER_GENERAL_COMMISSIONING: u32 = 0x0030;
pub const CLUSTER_NETWORK_COMMISSIONING: u32 = 0x0031;
pub const CLUSTER_OPERATIONAL_CREDENTIALS: u32 = 0x003E;

pub const CMD_ARM_FAIL_SAFE: u32 = 0x00;
pub const CMD_SET_REGULATORY_CONFIG: u32 = 0x02;
pub const CMD_COMMISSIONING_COMPLETE: u32 = 0x04;
pub const CMD_CSR_REQUEST: u32 = 0x04;
pub const CMD_ADD_NOC: u32 = 0x06;
pub const CMD_ADD_TRUSTED_ROOT_CERT: u32 = 0x0B;
pub const CMD_ADD_OR_UPDATE_WIFI_NETWORK: u32 = 0x02;
pub const CMD_CONNECT_NETWORK: u32 = 0x06;

/// Endpoint 0 on every commissionable device — the root node, where
/// all commissioning clusters live.
pub const ENDPOINT_ROOT: u16 = 0;

/// Regulatory configuration values per Matter spec §11.9.6.3.
#[repr(u8)]
#[derive(Debug, Copy, Clone)]
pub enum RegulatoryConfig {
    Indoor = 0,
    Outdoor = 1,
    IndoorOutdoor = 2,
}

/// WiFi credentials pushed to the device during commissioning.
#[derive(Debug, Clone)]
pub struct WifiCredentials {
    pub ssid: Vec<u8>,
    /// WPA2/WPA3 pre-shared key. For open networks pass an empty slice.
    pub psk: Vec<u8>,
}

/// Supplied by the caller — abstracts over the underlying transport
/// (BLE BTP or IP UDP). The key idea: after PASE is established the
/// commissioner doesn't care what's underneath, it just needs to be
/// able to run `invoke_single_cmd` + `read_single_attr` against an
/// authenticated exchange.
///
/// Implementations live in:
/// - Phase 4: `syntaur-matter-ble::BleCommissionExchange`
/// - Ad-hoc IP (tests or already-commissioned devices with OCW open):
///   `syntaur-gateway::matter_direct::IpCommissionExchange`
pub trait CommissionExchange: Send {
    /// Invoke a cluster command on endpoint 0, return the raw response
    /// TLV bytes (the caller parses per-command-specific response
    /// shape).
    fn invoke<'a>(
        &'a mut self,
        cluster: u32,
        command: u32,
        payload: Vec<u8>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, MatterFabricError>> + Send + 'a>,
    >;
}

/// Result of a successful commissioning run.
#[derive(Debug, Clone)]
pub struct CommissionedDevice {
    pub node_id: u64,
    pub fabric_label: String,
    /// Whatever TLV bytes the device returned for AddNOC's NOCResponse.
    /// Callers can persist this alongside the QR code for audit.
    pub add_noc_response: Vec<u8>,
}

/// Full commissioning driver.
pub struct Commissioner<'a> {
    pub fabric: &'a FabricHandle,
    /// Admin CAT — shared ACL entry so future key rotations don't
    /// require touching every device's ACL. Fixed to `0x0001_0001`
    /// (version 1, identifier 1) for now. See Matter spec §6.6.2.1.
    pub admin_cat: u32,
    /// Regulatory location (indoor/outdoor/both). Stored on the device.
    pub regulatory_config: RegulatoryConfig,
    /// ISO-3166 two-letter country code (e.g. `"US"`).
    pub country_code: String,
    /// Failsafe window for the whole commissioning sequence (seconds).
    /// 60s is generous; the spec allows up to 900s.
    pub fail_safe_seconds: u16,
}

impl<'a> Commissioner<'a> {
    /// Reasonable defaults: Outdoor regulatory, US, 60s failsafe, CAT 0x00010001.
    pub fn new(fabric: &'a FabricHandle) -> Self {
        Self {
            fabric,
            admin_cat: 0x0001_0001,
            regulatory_config: RegulatoryConfig::Outdoor,
            country_code: "US".into(),
            fail_safe_seconds: 60,
        }
    }

    /// Run the full commission flow against an already-PASE-authenticated
    /// exchange. `assigned_node_id` is what we'll give the device on
    /// our fabric. `wifi` is required for devices whose only rendezvous
    /// channel is BLE (must be handed WiFi credentials before CASE can
    /// reach them); pass `None` for already-WiFi devices.
    pub async fn commission<E: CommissionExchange>(
        &self,
        ex: &mut E,
        assigned_node_id: u64,
        wifi: Option<WifiCredentials>,
    ) -> Result<CommissionedDevice, MatterFabricError> {
        // Step 1 — ArmFailSafe: gates the rest; if the sequence fails
        // the device rolls back any writes after this window expires.
        let _ = ex
            .invoke(
                CLUSTER_GENERAL_COMMISSIONING,
                CMD_ARM_FAIL_SAFE,
                arm_fail_safe(self.fail_safe_seconds, 1),
            )
            .await?;

        // Step 2 — SetRegulatoryConfig.
        let _ = ex
            .invoke(
                CLUSTER_GENERAL_COMMISSIONING,
                CMD_SET_REGULATORY_CONFIG,
                set_regulatory_config(
                    self.regulatory_config as u8,
                    &self.country_code,
                    2,
                ),
            )
            .await?;

        // Step 3 — CSRRequest. The device returns NOCSRElements which
        // contains the device's CSR (DER-encoded PKCS#10).
        let mut nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce);
        let csr_resp_tlv = ex
            .invoke(
                CLUSTER_OPERATIONAL_CREDENTIALS,
                CMD_CSR_REQUEST,
                csr_request(&nonce, false),
            )
            .await?;
        let csr_der = extract_csr_from_response(&csr_resp_tlv)?;

        // Step 4 — sign a NOC for this device using our fabric CA.
        let noc_tlv = sign_device_noc(
            self.fabric,
            &csr_der,
            assigned_node_id,
            &[self.admin_cat],
        )?;

        // Step 5 — AddTrustedRootCertificate (spec §11.18.6.14). Must
        // precede AddNOC in the same failsafe.
        let fabric_rcac = hex::decode(&self.fabric.root_cert_hex)
            .map_err(|e| MatterFabricError::Matter(format!("root_cert_hex decode: {e}")))?;
        let _ = ex
            .invoke(
                CLUSTER_OPERATIONAL_CREDENTIALS,
                CMD_ADD_TRUSTED_ROOT_CERT,
                add_trusted_root_certificate(&fabric_rcac),
            )
            .await?;

        // Step 6 — AddNOC. After this the device is a member of our
        // fabric (though not yet on WiFi if it was BLE-only).
        let mut ipk = [0u8; 16];
        let ipk_bytes = hex::decode(&self.fabric.ipk_hex)
            .map_err(|e| MatterFabricError::Matter(format!("ipk_hex decode: {e}")))?;
        if ipk_bytes.len() != 16 {
            return Err(MatterFabricError::Matter(format!(
                "ipk_hex wrong length: {}B (want 16)",
                ipk_bytes.len()
            )));
        }
        ipk.copy_from_slice(&ipk_bytes);

        let add_noc_response = ex
            .invoke(
                CLUSTER_OPERATIONAL_CREDENTIALS,
                CMD_ADD_NOC,
                add_noc(
                    &noc_tlv,
                    None, // no ICAC for a directly-RCAC-signed NOC
                    &ipk,
                    self.fabric.controller_node_id, // our controller as the admin
                    self.fabric.vendor_id,
                ),
            )
            .await?;

        // Step 7 — WiFi handoff (if applicable).
        if let Some(wifi) = wifi {
            let _ = ex
                .invoke(
                    CLUSTER_NETWORK_COMMISSIONING,
                    CMD_ADD_OR_UPDATE_WIFI_NETWORK,
                    add_or_update_wifi_network(&wifi.ssid, &wifi.psk, 3),
                )
                .await?;
            let _ = ex
                .invoke(
                    CLUSTER_NETWORK_COMMISSIONING,
                    CMD_CONNECT_NETWORK,
                    connect_network(&wifi.ssid, 4),
                )
                .await?;
        }

        // Step 8 — CommissioningComplete. Device now switches to CASE
        // operational sessions only — any further ops use our NOC.
        let _ = ex
            .invoke(
                CLUSTER_GENERAL_COMMISSIONING,
                CMD_COMMISSIONING_COMPLETE,
                commissioning_complete(),
            )
            .await?;

        Ok(CommissionedDevice {
            node_id: assigned_node_id,
            fabric_label: self.fabric.label.clone(),
            add_noc_response,
        })
    }
}

/// Pull the device's CSR bytes (DER) out of the TLV-encoded CSRResponse.
///
/// `CSRResponse` shape (Matter spec §11.18.5.4):
/// ```text
/// struct {
///   0 : octets NOCSRElements,   // TLV-encoded inner struct
///   1 : octets AttestationSignature,  // 64 bytes
/// }
/// ```
/// `NOCSRElements` inner shape:
/// ```text
/// struct {
///   1 : octets CSR,          // DER-encoded PKCS#10 CertificateRequest
///   2 : octets CSRNonce,
///   3, 4, 5 : optional vendor_reserved
/// }
/// ```
/// We find tag 0 in the outer, then tag 1 in the inner. This is a
/// small TLV scan; a full parser is overkill.
fn extract_csr_from_response(tlv: &[u8]) -> Result<Vec<u8>, MatterFabricError> {
    let outer = tag_octets(tlv, 0)
        .ok_or_else(|| MatterFabricError::Matter("CSRResponse: missing tag 0".into()))?;
    let csr = tag_octets(outer, 1)
        .ok_or_else(|| MatterFabricError::Matter("NOCSRElements: missing tag 1 (CSR)".into()))?;
    Ok(csr.to_vec())
}

/// Very small TLV walker: given an anon-struct blob, find a
/// context-tagged octet string (`0x30 <tag> <len1>` or
/// `0x31 <tag> <len2 LE>`) with the given `want_tag`, return its
/// payload slice. No recursion; assumes the target is at the top
/// level of the given struct. Returns `None` on any shape that
/// doesn't match — good enough for well-formed Matter TLV from the
/// device.
fn tag_octets(blob: &[u8], want_tag: u8) -> Option<&[u8]> {
    let mut i = 0;
    // skip optional leading 0x15 struct-begin marker
    if blob.first() == Some(&0x15) {
        i = 1;
    }
    while i < blob.len() {
        let ctl = blob[i];
        if ctl == 0x18 {
            return None; // end of container
        }
        match ctl {
            0x24 => {
                // u8 ctx: 3 bytes
                if i + 2 >= blob.len() {
                    return None;
                }
                i += 3;
            }
            0x25 => {
                if i + 3 >= blob.len() {
                    return None;
                }
                i += 4;
            }
            0x26 => {
                if i + 5 >= blob.len() {
                    return None;
                }
                i += 6;
            }
            0x27 => {
                if i + 9 >= blob.len() {
                    return None;
                }
                i += 10;
            }
            0x28 | 0x29 => {
                if i + 1 >= blob.len() {
                    return None;
                }
                i += 2;
            }
            0x2C => {
                // string1 ctx: 0x2C tag len <bytes>
                if i + 2 >= blob.len() {
                    return None;
                }
                let len = blob[i + 2] as usize;
                i += 3 + len;
            }
            0x30 => {
                // octets1 ctx: 0x30 tag len <bytes>
                if i + 2 >= blob.len() {
                    return None;
                }
                let tag = blob[i + 1];
                let len = blob[i + 2] as usize;
                let start = i + 3;
                if start + len > blob.len() {
                    return None;
                }
                if tag == want_tag {
                    return Some(&blob[start..start + len]);
                }
                i = start + len;
            }
            0x31 => {
                // octets2 ctx: 0x31 tag len_lo len_hi <bytes>
                if i + 3 >= blob.len() {
                    return None;
                }
                let tag = blob[i + 1];
                let len = u16::from_le_bytes([blob[i + 2], blob[i + 3]]) as usize;
                let start = i + 4;
                if start + len > blob.len() {
                    return None;
                }
                if tag == want_tag {
                    return Some(&blob[start..start + len]);
                }
                i = start + len;
            }
            0x15 => i += 1,
            _ => return None, // unknown
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_octets_finds_targeted_field() {
        // struct { 0: "hi"[2], 1: "world"[5] }
        let blob = vec![
            0x15, // struct
            0x30, 0x00, 0x02, b'h', b'i', 0x30, 0x01, 0x05, b'w', b'o', b'r', b'l', b'd', 0x18,
        ];
        assert_eq!(tag_octets(&blob, 0), Some(&b"hi"[..]));
        assert_eq!(tag_octets(&blob, 1), Some(&b"world"[..]));
        assert_eq!(tag_octets(&blob, 2), None);
    }

    #[test]
    fn tag_octets_handles_long_form() {
        // struct { 0: <256 bytes> } via 0x31 length prefix
        let big = vec![0xAB; 256];
        let mut blob = vec![0x15, 0x31, 0x00, 0x00, 0x01];
        blob.extend_from_slice(&big);
        blob.push(0x18);
        assert_eq!(tag_octets(&blob, 0).map(|s| s.len()), Some(256));
    }

    #[test]
    fn extract_csr_sees_nested() {
        // outer: { 0: octets<inner>, 1: octets<sig> }
        //   inner: { 1: octets<csr>, 2: octets<nonce> }
        let inner_csr = b"fake-csr-bytes";
        let inner_nonce = [0xEE; 32];
        let mut inner = vec![0x15];
        inner.extend_from_slice(&[0x30, 0x01, inner_csr.len() as u8]);
        inner.extend_from_slice(inner_csr);
        inner.extend_from_slice(&[0x30, 0x02, 32]);
        inner.extend_from_slice(&inner_nonce);
        inner.push(0x18);

        let sig = [0xCC; 64];
        let mut outer = vec![0x15];
        outer.push(0x31);
        outer.push(0x00);
        outer.extend_from_slice(&(inner.len() as u16).to_le_bytes());
        outer.extend_from_slice(&inner);
        outer.push(0x30);
        outer.push(0x01);
        outer.push(64);
        outer.extend_from_slice(&sig);
        outer.push(0x18);

        let csr = extract_csr_from_response(&outer).unwrap();
        assert_eq!(csr, inner_csr);
    }
}

// Exchange trait is declared above; a concrete implementation requires
// an existing `rs_matter::transport::exchange::Exchange` handle after
// `PaseInitiator::initiate` has succeeded. Keep the trait dyn-safe so
// Phase 4's `syntaur-matter-ble` and any future IP wrapper can slot in
// without changing this crate.
#[allow(dead_code)]
fn _exchange_trait_is_object_safe(_e: &dyn CommissionExchange) {}

// Silence the unused-`Exchange`-import warning until Phase 4 plugs in
// the BLE-backed implementation.
#[allow(dead_code)]
fn _phase4_will_use_exchange<'a>(_: &'a mut Exchange<'a>) {}
