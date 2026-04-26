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
    add_noc, add_or_update_thread_network, add_or_update_wifi_network,
    add_trusted_root_certificate, arm_fail_safe, commissioning_complete, connect_network,
    csr_request, extract_thread_extpanid, set_regulatory_config,
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
pub const CMD_ADD_OR_UPDATE_THREAD_NETWORK: u32 = 0x03;
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

/// Thread credentials pushed to the device during commissioning. The
/// `operational_dataset` is a raw Thread TLV blob (per Thread spec
/// §8.10) carrying network key + channel + extended PAN ID + mesh-local
/// prefix. Same dataset every device on the same mesh receives; no
/// per-device customization.
#[derive(Debug, Clone)]
pub struct ThreadCredentials {
    pub operational_dataset: Vec<u8>,
}

/// Network credentials to push during commissioning. The right variant
/// depends on the device's physical radio — WiFi devices want WiFi,
/// Thread devices want Thread. Mis-matching will surface as a
/// `NETWORK_CONFIG` status from the device at the AddOrUpdate step.
#[derive(Debug, Clone)]
pub enum NetworkCredentials {
    Wifi(WifiCredentials),
    Thread(ThreadCredentials),
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

    /// Drive CASE handshake on the operational identity (controller NOC
    /// signed by the fabric's RCAC), then invoke CommissioningComplete
    /// on the resulting CASE session. Required because CommissioningComplete
    /// per Matter Core §11.10.6.6 must run on a CASE session, not PASE.
    fn case_and_commissioning_complete<'a>(
        &'a mut self,
        fabric: &'a crate::FabricHandle,
        peer_node_id: u64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), MatterFabricError>> + Send + 'a>,
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
            fail_safe_seconds: 900,
        }
    }

    /// Run the full commission flow against an already-PASE-authenticated
    /// exchange. `assigned_node_id` is what we'll give the device on
    /// our fabric. `network` is required for BLE-rendezvous devices
    /// (must be handed WiFi OR Thread credentials before the device
    /// can reach the IP network and CASE can be established). Pass
    /// `None` for already-on-network devices (Track A / OCW path).
    pub async fn commission<E: CommissionExchange>(
        &self,
        ex: &mut E,
        assigned_node_id: u64,
        network: Option<NetworkCredentials>,
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
        log::info!(
            "[commission] device NOC ({} bytes): {}",
            noc_tlv.len(),
            noc_tlv.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
        );

        // Step 5 — AddTrustedRootCertificate (spec §11.18.6.14). Must
        // precede AddNOC in the same failsafe.
        let fabric_rcac = hex::decode(&self.fabric.root_cert_hex)
            .map_err(|e| MatterFabricError::Matter(format!("root_cert_hex decode: {e}")))?;
        eprintln!("RCAC FULL ({} bytes): {}",
                  fabric_rcac.len(),
                  fabric_rcac.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(""));
        let payload = add_trusted_root_certificate(&fabric_rcac);
        eprintln!("AddTrustedRootCert payload: {} bytes, first 16: {:02x?}",
                  payload.len(),
                  &payload[..payload.len().min(16)]);
        let _ = ex
            .invoke(
                CLUSTER_OPERATIONAL_CREDENTIALS,
                CMD_ADD_TRUSTED_ROOT_CERT,
                payload,
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
        log::info!(
            "[commission] AddNOC returned {} bytes: first 32 = {:02x?}",
            add_noc_response.len(),
            &add_noc_response[..add_noc_response.len().min(32)]
        );
        if let Some(status) = parse_noc_response_status(&add_noc_response) {
            log::info!("[commission] AddNOC NOCResponse.StatusCode = {status:#x} ({})", noc_status_name(status));
            if status != 0 {
                return Err(MatterFabricError::Matter(format!(
                    "AddNOC rejected: NOCResponse.StatusCode={status:#x} ({})",
                    noc_status_name(status)
                )));
            }
        }

        // Step 7 — network handoff (if applicable). The NetworkID passed
        // to ConnectNetwork is the SSID for WiFi and the 8-byte
        // Extended PAN ID for Thread (Matter spec §11.8.6.6.1).
        match network {
            Some(NetworkCredentials::Wifi(wifi)) => {
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
            Some(NetworkCredentials::Thread(thread)) => {
                let extpanid = extract_thread_extpanid(&thread.operational_dataset)
                    .map_err(MatterFabricError::Matter)?;
                let aou_resp = ex
                    .invoke(
                        CLUSTER_NETWORK_COMMISSIONING,
                        CMD_ADD_OR_UPDATE_THREAD_NETWORK,
                        add_or_update_thread_network(&thread.operational_dataset, 3),
                    )
                    .await?;
                log::info!(
                    "[commission] AddOrUpdateThreadNetwork returned {} bytes: first 32 = {:02x?}",
                    aou_resp.len(),
                    &aou_resp[..aou_resp.len().min(32)]
                );
                if let Some(status) = parse_network_config_status(&aou_resp) {
                    log::info!("[commission] AddOrUpdateThreadNetwork NetworkingStatus = {status:#x} ({})", networking_status_name(status));
                    if status != 0 {
                        return Err(MatterFabricError::Matter(format!(
                            "AddOrUpdateThreadNetwork rejected: status={status:#x} ({})",
                            networking_status_name(status)
                        )));
                    }
                }
                let cn_resp = ex
                    .invoke(
                        CLUSTER_NETWORK_COMMISSIONING,
                        CMD_CONNECT_NETWORK,
                        connect_network(&extpanid, 4),
                    )
                    .await?;
                log::info!(
                    "[commission] ConnectNetwork returned {} bytes: first 32 = {:02x?}",
                    cn_resp.len(),
                    &cn_resp[..cn_resp.len().min(32)]
                );
                if let Some(status) = parse_network_config_status(&cn_resp) {
                    log::info!("[commission] ConnectNetwork NetworkingStatus = {status:#x} ({})", networking_status_name(status));
                    if status != 0 {
                        return Err(MatterFabricError::Matter(format!(
                            "ConnectNetwork rejected: status={status:#x} ({})",
                            networking_status_name(status)
                        )));
                    }
                }
            }
            None => {}
        }

        // Step 8 — CASE handshake then CommissioningComplete on CASE.
        // Per Matter Core §11.10.6.6, CommissioningComplete must run on
        // a CASE-secured session using the operational NOC, not the PASE
        // setup session. The transport impl is responsible for:
        //   1. Generating controller keypair + signing controller NOC
        //   2. Registering fabric in rs-matter's FabricMgr
        //   3. Running CaseInitiator::initiate(exchange, crypto, fab_idx, peer_node_id)
        //   4. Invoking CommissioningComplete on the CASE-secured exchange
        ex.case_and_commissioning_complete(self.fabric, assigned_node_id)
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
        .ok_or_else(|| {
            eprintln!("CSRResponse raw bytes ({} bytes): {:02x?}", tlv.len(), tlv);
            MatterFabricError::Matter(format!("CSRResponse: missing tag 0 in {} bytes", tlv.len()))
        })?;
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
    // Skip optional leading struct-begin marker. Matter responses can use
    // either 0x15 (anonymous struct, 1 byte) or 0x35 <tag> (context-tagged
    // struct, 2 bytes). Eve's CSRResponse comes wrapped in 0x35 0x01.
    if blob.first() == Some(&0x15) {
        i = 1;
    } else if blob.first() == Some(&0x35) {
        i = 2;  // skip control byte + 1-byte context tag
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


// ── Response parsers (best-effort) ─────────────────────────────────────────
//
// The Matter IM `InvokeResponse` we surface above is a raw TLV blob — the
// device's struct-shaped response after `CmdResp::Cmd` unwrap. We pull the
// first context-tagged u8 field which is the StatusCode for both
// NOCResponse (cluster 0x3E cmd 0x08 reply) and NetworkConfigResponse
// (cluster 0x31 cmd 0x05 reply). If the TLV doesn't parse (unexpected
// shape), we return None and let the caller continue rather than blocking
// commissioning on a parser limitation.

fn parse_noc_response_status(buf: &[u8]) -> Option<u8> {
    parse_first_context_u8(buf)
}

fn parse_network_config_status(buf: &[u8]) -> Option<u8> {
    parse_first_context_u8(buf)
}

/// Find the first context-tagged u8 element inside a TLV struct.
/// Matter struct begins with 0x15. Context-tagged u8 element control
/// byte = 0x24 (type=04 unsigned int 1B, tag form=01 context-1B).
fn parse_first_context_u8(buf: &[u8]) -> Option<u8> {
    let mut i = 0;
    if buf.first()? != &0x15 { return None; } // expected struct begin
    i += 1;
    while i < buf.len() {
        let cb = buf[i];
        if cb == 0x18 { return None; } // end of struct, no u8 found
        if cb == 0x24 && i + 2 < buf.len() {
            // 0x24 = (type=04 u8, tag=01 ctx-1B). Skip tag byte at i+1, value at i+2.
            return Some(buf[i + 2]);
        }
        // Skip element. We only handle a few common forms enough to walk past
        // them; if we hit anything more complex we bail.
        i += 1;
    }
    None
}

fn noc_status_name(s: u8) -> &'static str {
    match s {
        0x00 => "OK",
        0x01 => "InvalidPublicKey",
        0x02 => "InvalidNodeOpId",
        0x03 => "InvalidNOC",
        0x04 => "MissingCsr",
        0x05 => "TableFull",
        0x06 => "InvalidAdminSubject",
        0x09 => "FabricConflict",
        0x0a => "LabelConflict",
        0x0b => "InvalidFabricIndex",
        _ => "?",
    }
}

fn networking_status_name(s: u8) -> &'static str {
    match s {
        0x00 => "Success",
        0x01 => "OutOfRange",
        0x02 => "BoundsExceeded",
        0x03 => "NetworkIDNotFound",
        0x04 => "DuplicateNetworkID",
        0x05 => "NetworkNotFound",
        0x06 => "RegulatoryError",
        0x07 => "AuthFailure",
        0x08 => "UnsupportedSecurity",
        0x09 => "OtherConnectionFailure",
        0x0a => "IPV6Failed",
        0x0b => "IPBindFailed",
        0x0c => "UnknownError",
        _ => "?",
    }
}
