//! Minimal Matter-TLV byte builders for commissioning-command payloads.
//!
//! rs-matter has a full typestate-chained `Builder` family per cluster
//! command, but bringing those into this crate needs a
//! `TLVBuilderParent` implementation + careful lifetime plumbing. For
//! the 8 commissioning commands we need, the hand-built form below is
//! ~80 LoC total — trading a little redundancy for build-time
//! independence and unit-testable byte-for-byte output.
//!
//! Matter TLV reference (control byte ‖ optional tag ‖ optional length ‖ value):
//! - `0x15`            anon struct begin
//! - `0x18`            end of container
//! - `0x24 tt vv`       context-tagged u8
//! - `0x25 tt ll hh`    context-tagged u16 (little-endian)
//! - `0x26 tt ....`     context-tagged u32 (LE, 4 bytes)
//! - `0x27 tt ........` context-tagged u64 (LE, 8 bytes)
//! - `0x28 tt`          context-tagged bool false
//! - `0x29 tt`          context-tagged bool true
//! - `0x2C tt ll str`   context-tagged UTF-8 string (1-byte length)
//! - `0x30 tt ll bin`   context-tagged octet-string (1-byte length)
//! - `0x31 tt ll2 bin`  context-tagged octet-string (2-byte length)

/// Buffer that emits Matter TLV.
#[derive(Default)]
pub struct TlvBuf(pub Vec<u8>);

impl TlvBuf {
    pub fn new() -> Self {
        Self(Vec::with_capacity(64))
    }

    pub fn start_struct(&mut self) -> &mut Self {
        self.0.push(0x15);
        self
    }

    pub fn end(&mut self) -> &mut Self {
        self.0.push(0x18);
        self
    }

    pub fn u8(&mut self, tag: u8, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[0x24, tag, v]);
        self
    }

    pub fn u16(&mut self, tag: u8, v: u16) -> &mut Self {
        self.0.push(0x25);
        self.0.push(tag);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    #[allow(dead_code)]
    pub fn u32(&mut self, tag: u8, v: u32) -> &mut Self {
        self.0.push(0x26);
        self.0.push(tag);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, tag: u8, v: u64) -> &mut Self {
        self.0.push(0x27);
        self.0.push(tag);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bool(&mut self, tag: u8, v: bool) -> &mut Self {
        self.0.extend_from_slice(&[if v { 0x29 } else { 0x28 }, tag]);
        self
    }

    /// UTF-8 string, 1-byte length. Panics if `s.len() > 255` — only
    /// reasonable for country codes, fabric labels, etc.
    pub fn string1(&mut self, tag: u8, s: &str) -> &mut Self {
        assert!(s.len() <= 255, "string1 too long");
        self.0.extend_from_slice(&[0x2C, tag, s.len() as u8]);
        self.0.extend_from_slice(s.as_bytes());
        self
    }

    /// Octet-string with 1-byte length prefix (`type = 0x30`). For
    /// payloads ≤ 255 bytes — fine for our nonces. Use
    /// [`Self::octets2`] for longer.
    pub fn octets1(&mut self, tag: u8, bytes: &[u8]) -> &mut Self {
        assert!(bytes.len() <= 255, "octets1 too long; use octets2");
        self.0.extend_from_slice(&[0x30, tag, bytes.len() as u8]);
        self.0.extend_from_slice(bytes);
        self
    }

    /// Octet-string with 2-byte LE length prefix (`type = 0x31`).
    /// Used for TLV-encoded certs (NOC, RCAC, ICAC) which can exceed
    /// 255 bytes.
    pub fn octets2(&mut self, tag: u8, bytes: &[u8]) -> &mut Self {
        assert!(bytes.len() <= 65535, "octets2 too long; use 4-byte length");
        self.0.push(0x31);
        self.0.push(tag);
        self.0.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        self.0.extend_from_slice(bytes);
        self
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

// ── Commissioning command payload builders ──────────────────────────────

/// GeneralCommissioning::ArmFailSafe(cluster 0x0030, cmd 0x00):
/// `{ 0: u16 expiry_length_seconds, 1: u64 breadcrumb }`
pub fn arm_fail_safe(expiry_seconds: u16, breadcrumb: u64) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .u16(0, expiry_seconds)
        .u64(1, breadcrumb)
        .end();
    b.into_bytes()
}

/// GeneralCommissioning::SetRegulatoryConfig(cluster 0x0030, cmd 0x02):
/// `{ 0: u8 config, 1: string country_code, 2: u64 breadcrumb }`.
/// `config` values: 0=Indoor, 1=Outdoor, 2=IndoorOutdoor.
pub fn set_regulatory_config(config: u8, country_code: &str, breadcrumb: u64) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .u8(0, config)
        .string1(1, country_code)
        .u64(2, breadcrumb)
        .end();
    b.into_bytes()
}

/// GeneralCommissioning::CommissioningComplete(cluster 0x0030, cmd 0x04):
/// empty struct `{}`.
pub fn commissioning_complete() -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct().end();
    b.into_bytes()
}

/// OperationalCredentials::CSRRequest(cluster 0x003E, cmd 0x04):
/// `{ 0: octets[32] csr_nonce, 1: bool is_for_update_noc }`.
pub fn csr_request(csr_nonce: &[u8; 32], is_for_update: bool) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .octets1(0, csr_nonce)
        .bool(1, is_for_update)
        .end();
    b.into_bytes()
}

/// OperationalCredentials::AddTrustedRootCertificate(cluster 0x003E, cmd 0x0B):
/// `{ 0: octets TLV-encoded root cert }`. Single field, no bool.
/// Spec requires this to be invoked BEFORE AddNOC in the same failsafe.
pub fn add_trusted_root_certificate(root_cert_tlv: &[u8]) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct().octets2(0, root_cert_tlv).end();
    b.into_bytes()
}

/// OperationalCredentials::AddNOC(cluster 0x003E, cmd 0x06):
/// `{ 0: octets NOC, 1: octets ICAC (optional; omit field for none),
///    2: octets IPK (16 B), 3: u64 case_admin_subject, 4: u16 admin_vendor_id }`.
pub fn add_noc(
    noc_tlv: &[u8],
    icac_tlv: Option<&[u8]>,
    ipk: &[u8; 16],
    case_admin_subject: u64,
    admin_vendor_id: u16,
) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct().octets2(0, noc_tlv);
    if let Some(ic) = icac_tlv {
        b.octets2(1, ic);
    }
    b.octets1(2, ipk)
        .u64(3, case_admin_subject)
        .u16(4, admin_vendor_id)
        .end();
    b.into_bytes()
}

/// NetworkCommissioning::AddOrUpdateWiFiNetwork(cluster 0x0031, cmd 0x02):
/// `{ 0: octets SSID (≤32 B), 1: octets Credentials (PSK, ≤64 B),
///    2: u64 breadcrumb }`.
pub fn add_or_update_wifi_network(ssid: &[u8], psk: &[u8], breadcrumb: u64) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .octets1(0, ssid)
        .octets1(1, psk)
        .u64(2, breadcrumb)
        .end();
    b.into_bytes()
}

/// NetworkCommissioning::AddOrUpdateThreadNetwork(cluster 0x0031, cmd 0x03):
/// `{ 0: octets OperationalDataset (Thread TLV blob, ≤254 B),
///    1: u64 breadcrumb }`.
///
/// The operational dataset is Thread's own TLV-encoded set of network
/// credentials (network key, channel, extended PAN ID, mesh-local prefix,
/// etc.) — we pass it opaquely; the device parses it per Thread spec.
pub fn add_or_update_thread_network(operational_dataset: &[u8], breadcrumb: u64) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .octets1(0, operational_dataset)
        .u64(1, breadcrumb)
        .end();
    b.into_bytes()
}

/// NetworkCommissioning::ConnectNetwork(cluster 0x0031, cmd 0x06):
/// `{ 0: octets NetworkID (SSID for WiFi, 8-byte Extended PAN ID for Thread),
///    1: u64 breadcrumb }`.
pub fn connect_network(network_id: &[u8], breadcrumb: u64) -> Vec<u8> {
    let mut b = TlvBuf::new();
    b.start_struct()
        .octets1(0, network_id)
        .u64(1, breadcrumb)
        .end();
    b.into_bytes()
}

/// Extract the 8-byte Extended PAN ID from a Thread operational dataset TLV
/// blob. Thread TLV type 0x02 = Extended PAN ID, always 8 bytes. Used as
/// the NetworkID for [`connect_network`] on Thread devices.
pub fn extract_thread_extpanid(operational_dataset: &[u8]) -> Result<[u8; 8], String> {
    let mut i = 0;
    while i + 2 <= operational_dataset.len() {
        let t = operational_dataset[i];
        let len = operational_dataset[i + 1] as usize;
        if i + 2 + len > operational_dataset.len() {
            return Err(format!("malformed thread TLV at offset {i}: len={len} exceeds blob"));
        }
        if t == 0x02 {
            if len != 8 {
                return Err(format!("extended pan id TLV len {len} (want 8)"));
            }
            let mut out = [0u8; 8];
            out.copy_from_slice(&operational_dataset[i + 2..i + 10]);
            return Ok(out);
        }
        i += 2 + len;
    }
    Err("extended pan id TLV (type 0x02) not found in operational dataset".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_fail_safe_shape() {
        // { 0: 60, 1: 1 } → 15 | 25 00 3C 00 | 27 01 01 00 00 00 00 00 00 00 | 18
        let b = arm_fail_safe(60, 1);
        assert_eq!(
            b,
            vec![
                0x15, // struct begin
                0x25, 0x00, 0x3C, 0x00, // u16 tag 0 = 60
                0x27, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // u64 tag 1 = 1
                0x18  // end
            ]
        );
    }

    #[test]
    fn set_regulatory_config_shape() {
        // config=Outdoor(1), cc="US", breadcrumb=0
        let b = set_regulatory_config(1, "US", 0);
        assert_eq!(
            b,
            vec![
                0x15, // struct
                0x24, 0x00, 0x01, // u8 tag 0 = 1 (Outdoor)
                0x2C, 0x01, 0x02, b'U', b'S', // string tag 1 = "US"
                0x27, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // u64 tag 2 = 0
                0x18
            ]
        );
    }

    #[test]
    fn commissioning_complete_is_empty_struct() {
        assert_eq!(commissioning_complete(), vec![0x15, 0x18]);
    }

    #[test]
    fn csr_request_has_nonce_and_bool() {
        let nonce = [0x42u8; 32];
        let b = csr_request(&nonce, false);
        assert_eq!(b[0], 0x15); // struct begin
        assert_eq!(&b[1..4], &[0x30, 0x00, 32]); // octet-string tag 0 len 32
        assert_eq!(&b[4..36], &nonce);
        assert_eq!(&b[36..38], &[0x28, 0x01]); // bool false tag 1
        assert_eq!(b[38], 0x18); // end
        assert_eq!(b.len(), 39);
    }

    #[test]
    fn add_noc_no_icac_has_correct_fields() {
        let noc = vec![0xAB; 300]; // large enough to force 2-byte length
        let ipk = [0xCCu8; 16];
        let b = add_noc(&noc, None, &ipk, 0xDEAD_BEEF, 0xFFF1);
        // NOC field first: 0x31 0x00 <len_lo> <len_hi> <bytes>
        assert_eq!(b[0], 0x15);
        assert_eq!(b[1..4], [0x31, 0x00, 0x2C]); // tag 0, len 300 LE = 0x012C
        assert_eq!(b[4], 0x01);
        assert_eq!(&b[5..305], noc.as_slice());
        // Then ipk (tag 2, no tag 1)
        assert_eq!(&b[305..308], &[0x30, 0x02, 0x10]);
        assert_eq!(&b[308..324], &ipk);
        // u64 tag 3 + u16 tag 4 + end
        assert_eq!(&b[324..334], &[0x27, 0x03, 0xEF, 0xBE, 0xAD, 0xDE, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&b[334..338], &[0x25, 0x04, 0xF1, 0xFF]);
        assert_eq!(b[338], 0x18);
    }
}
