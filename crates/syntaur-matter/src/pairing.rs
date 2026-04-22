//! QR code and manual pairing code decoder — Matter Core Spec §5.1.
//!
//! Both forms encode the same logical tuple:
//!   { version, vendor_id?, product_id?, commissioning_flow,
//!     rendezvous_info, discriminator, setup_passcode }
//!
//! Except: the 11-digit manual code CAN'T represent the full 12-bit
//! discriminator (it only carries the top 4 bits). Devices in
//! commissioning mode advertise the full 12-bit discriminator via mDNS
//! / BLE service data; matching by the 4-bit prefix is the standard
//! approach.
//!
//! Decoder is the inverse of rs-matter's encoder (which lives in
//! `rs_matter::pairing::{qr, code}`). Tests cross-check against the
//! vectors in rs-matter's own `compute_pairing_code` test.

use serde::{Deserialize, Serialize};

use crate::MatterFabricError;

/// What both `parse_qr` and `parse_manual_code` return — the
/// commissioner's "everything it needs to know to start PASE".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingPayload {
    /// Always 0 for Matter 1.x — reserved for future protocol revisions.
    pub version: u8,
    /// 16-bit Matter vendor ID. `None` for 11-digit manual codes
    /// (they omit vendor info to save digits).
    pub vendor_id: Option<u16>,
    /// 16-bit Matter product ID. Same caveat as `vendor_id`.
    pub product_id: Option<u16>,
    /// See [`CommissioningFlow`]. Default `Standard` for 11-digit manual codes.
    pub commissioning_flow: CommissioningFlow,
    /// 8-bit rendezvous bitmap: 0x1 = SoftAP, 0x2 = BLE, 0x4 = OnNetwork,
    /// 0x8 = WiFiPAF, 0x10 = NFC. Device may support multiple in parallel.
    pub rendezvous: u8,
    /// Full 12-bit discriminator (0..=4095). For 11-digit manual codes
    /// only bits 11..=8 are encoded; the low 8 bits are set to 0 here.
    /// Callers matching BLE advertisements should compare only the
    /// high 4 bits when the source was an 11-digit manual code.
    pub discriminator: u16,
    /// Whether the discriminator was delivered in full 12-bit form
    /// (QR, 21-digit manual code) or the 4-bit short form (11-digit).
    pub discriminator_short: bool,
    /// 27-bit setup passcode — the value fed to `PaseInitiator::initiate`.
    pub passcode: u32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CommissioningFlow {
    /// Device is ready to be commissioned at any time — just power it on.
    Standard = 0,
    /// Device requires a user action (physical button press, screen
    /// gesture, serial number entry, etc.) before accepting commissioning.
    UserActionRequired = 1,
    /// Vendor-defined flow — requires consulting vendor documentation.
    Custom = 2,
    /// Reserved / unknown.
    Reserved = 3,
}

impl From<u8> for CommissioningFlow {
    fn from(v: u8) -> Self {
        match v & 0x3 {
            0 => Self::Standard,
            1 => Self::UserActionRequired,
            2 => Self::Custom,
            _ => Self::Reserved,
        }
    }
}

/// Parse a QR-code text payload like `MT:Y.K9042C00KA0648G00`.
///
/// Strips the `MT:` prefix and any trailing TLV supplemental data
/// (separated by `.`), base38-decodes the 11-byte payload, then
/// bit-unpacks per spec §5.1.2.2.
pub fn parse_qr(raw: &str) -> Result<PairingPayload, MatterFabricError> {
    let stripped = raw
        .strip_prefix("MT:")
        .ok_or_else(|| MatterFabricError::Matter("QR payload missing \"MT:\" prefix".into()))?;
    // NB: `.` is a legitimate base38 character (it encodes value 37),
    // so don't split on it. Any supplemental TLV data lives INSIDE the
    // decoded bytes past offset 11, not as a text-level suffix.
    let bytes = base38_decode(stripped)?;
    if bytes.len() < 11 {
        return Err(MatterFabricError::Matter(format!(
            "QR payload decoded to {}B, expected ≥ 11",
            bytes.len()
        )));
    }

    // Bit-unpack per spec §5.1.2.2. The "Total Payload Data Size" is 88
    // bits = 11 bytes. Individual fields are little-endian bit-packed
    // across the byte stream.
    let bits = BitReader::new(&bytes);
    let version = bits.read(0, 3) as u8;
    let vendor_id = bits.read(3, 16) as u16;
    let product_id = bits.read(19, 16) as u16;
    let flow = CommissioningFlow::from(bits.read(35, 2) as u8);
    let rendezvous = bits.read(37, 8) as u8;
    let discriminator = bits.read(45, 12) as u16;
    let passcode = bits.read(57, 27);
    // bits 84-87 are padding.

    Ok(PairingPayload {
        version,
        vendor_id: Some(vendor_id),
        product_id: Some(product_id),
        commissioning_flow: flow,
        rendezvous,
        discriminator,
        discriminator_short: false,
        passcode,
    })
}

/// Parse an 11- or 21-digit manual pairing code string.
///
/// Accepts dashes (e.g. `"0876-800-071"`) — they're stripped before
/// parsing. Digits only; Verhoeff check digit is validated.
pub fn parse_manual_code(raw: &str) -> Result<PairingPayload, MatterFabricError> {
    let digits: Vec<u8> = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| {
            c.to_digit(10)
                .ok_or_else(|| {
                    MatterFabricError::Matter(format!("non-digit {c:?} in manual code"))
                })
                .map(|d| d as u8)
        })
        .collect::<Result<_, _>>()?;

    match digits.len() {
        11 => parse_manual_11(&digits),
        21 => parse_manual_21(&digits),
        n => Err(MatterFabricError::Matter(format!(
            "manual code has {n} digits, expected 11 or 21"
        ))),
    }
}

fn parse_manual_11(d: &[u8]) -> Result<PairingPayload, MatterFabricError> {
    verify_verhoeff(d)?;

    let d1 = d[0] as u32;
    let p1 = digits_to_u32(&d[1..=5]); // 5-digit block
    let p2 = digits_to_u32(&d[6..=9]); // 4-digit block

    let vid_pid_present = (d1 >> 2) & 0x1 == 1;
    if vid_pid_present {
        return Err(MatterFabricError::Matter(
            "11-digit manual code has VID/PID flag set, but only 11 digits — malformed".into(),
        ));
    }

    let upper_disc_bits_11_10 = d1 & 0x3; // 2 bits
    let upper_disc_bits_9_8 = (p1 >> 14) & 0x3; // 2 bits
    let upper4 = (upper_disc_bits_11_10 << 2) | upper_disc_bits_9_8; // 4 bits, range 0..=15
    // Shift into the 12-bit slot's high nibble. Low 8 bits unknown.
    let discriminator = ((upper4 as u16) & 0xF) << 8;

    let passcode = (p2 << 14) | (p1 & 0x3FFF);

    Ok(PairingPayload {
        version: 0,
        vendor_id: None,
        product_id: None,
        commissioning_flow: CommissioningFlow::Standard,
        rendezvous: 0,
        discriminator,
        discriminator_short: true,
        passcode,
    })
}

fn parse_manual_21(d: &[u8]) -> Result<PairingPayload, MatterFabricError> {
    verify_verhoeff(d)?;

    let d1 = d[0] as u32;
    let p1 = digits_to_u32(&d[1..=5]);
    let p2 = digits_to_u32(&d[6..=9]);
    let vid = digits_to_u32(&d[10..=14]);
    let pid = digits_to_u32(&d[15..=19]);
    // d[20] is Verhoeff, already checked.

    let vid_pid_present = (d1 >> 2) & 0x1 == 1;
    if !vid_pid_present {
        return Err(MatterFabricError::Matter(
            "21-digit manual code has VID/PID flag unset".into(),
        ));
    }

    let upper_disc_bits_11_10 = d1 & 0x3;
    let upper_disc_bits_9_8 = (p1 >> 14) & 0x3;
    let upper4 = (upper_disc_bits_11_10 << 2) | upper_disc_bits_9_8;
    let discriminator = ((upper4 as u16) & 0xF) << 8;
    let passcode = (p2 << 14) | (p1 & 0x3FFF);

    Ok(PairingPayload {
        version: 0,
        vendor_id: Some(vid as u16),
        product_id: Some(pid as u16),
        commissioning_flow: CommissioningFlow::Standard,
        rendezvous: 0,
        discriminator,
        discriminator_short: true,
        passcode,
    })
}

fn digits_to_u32(digits: &[u8]) -> u32 {
    digits.iter().fold(0u32, |acc, d| acc * 10 + *d as u32)
}

fn verify_verhoeff(digits: &[u8]) -> Result<(), MatterFabricError> {
    // ISO 7090 Verhoeff algorithm. Multiplication + permutation +
    // inverse tables from Wikipedia. The trailing check digit closes
    // the sum when the algorithm is applied in reverse over the whole
    // string.
    const D: [[u8; 10]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
        [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
        [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
        [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
        [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
        [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
        [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
        [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
        [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
    ];
    const P: [[u8; 10]; 8] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
        [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
        [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
        [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
        [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
        [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
        [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
    ];
    let mut c = 0u8;
    for (i, digit) in digits.iter().rev().enumerate() {
        c = D[c as usize][P[i % 8][*digit as usize] as usize];
    }
    if c != 0 {
        return Err(MatterFabricError::Matter(
            "Verhoeff check digit mismatch (manual code corrupt)".into(),
        ));
    }
    Ok(())
}

// ── base38 decode ──────────────────────────────────────────────────────

fn base38_decode(s: &str) -> Result<Vec<u8>, MatterFabricError> {
    // Inverse of rs-matter's `utils::codec::base38::encode`. Each 3 base38
    // chars = 2 bytes (16 bits), except the final chunk which may be 1
    // or 2 chars for 1 byte. Per Matter spec §5.1.2.1.
    let chars: Vec<u8> = s.bytes().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let remaining = chars.len() - i;
        let chunk = remaining.min(3);
        let mut value: u32 = 0;
        for j in 0..chunk {
            let c = chars[i + chunk - 1 - j];
            let d = b38_val(c)?;
            value = value * 38 + d as u32;
        }
        let out_bytes = match chunk {
            3 => 2,
            2 => 1,
            1 => 1,
            _ => return Err(MatterFabricError::Matter("base38 chunk empty".into())),
        };
        for b in 0..out_bytes {
            out.push((value >> (b * 8)) as u8);
        }
        i += chunk;
    }
    Ok(out)
}

fn b38_val(c: u8) -> Result<u8, MatterFabricError> {
    const UNUSED: u8 = 255;
    const TABLE: [u8; 46] = [
        36,     // '-'
        37,     // '.'
        UNUSED, // '/'
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9,    // '0'..'9'
        UNUSED, UNUSED, UNUSED, UNUSED, UNUSED, UNUSED, UNUSED, // ':' through '@'
        10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
        26, 27, 28, 29, 30, 31, 32, 33, 34, 35, // 'A'..'Z'
    ];
    if !(45..=90).contains(&c) {
        return Err(MatterFabricError::Matter(format!(
            "invalid base38 char {:?}",
            c as char
        )));
    }
    let v = TABLE[(c - 45) as usize];
    if v == UNUSED {
        return Err(MatterFabricError::Matter(format!(
            "invalid base38 char {:?}",
            c as char
        )));
    }
    Ok(v)
}

// ── bit reader ─────────────────────────────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Read `count` bits starting at absolute bit offset `offset`.
    /// Bits are packed little-endian within each byte (bit 0 of the
    /// stream is bit 0 of byte 0).
    fn read(&self, offset: usize, count: usize) -> u32 {
        let mut out: u32 = 0;
        for i in 0..count {
            let bit_pos = offset + i;
            let byte = self.data[bit_pos / 8];
            let bit = (byte >> (bit_pos % 8)) & 1;
            out |= (bit as u32) << i;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors from rs-matter's own encoder self-test
    // (pairing/code.rs::tests::can_compute_pairing_code).
    #[test]
    fn manual_11_roundtrip_vector_1() {
        // password=123456, disc=250 → "00876800071"
        let p = parse_manual_code("00876800071").unwrap();
        assert_eq!(p.passcode, 123456);
        assert!(p.discriminator_short);
        // disc=250 has upper 4 bits (11:8) = 0, so decoded discriminator = 0 << 8 = 0
        assert_eq!(p.discriminator, 0);
        assert_eq!(p.vendor_id, None);
    }

    #[test]
    fn manual_11_roundtrip_vector_2() {
        // password=34567890, disc=2976 → "26318621095"
        // disc=2976=0xBA0 has upper 4 bits = 0xB = 11
        let p = parse_manual_code("26318621095").unwrap();
        assert_eq!(p.passcode, 34567890);
        assert!(p.discriminator_short);
        assert_eq!(p.discriminator, 0xB00);
    }

    #[test]
    fn manual_accepts_dashes_and_pretty() {
        let with_dashes = parse_manual_code("0087-680-0071").unwrap();
        let plain = parse_manual_code("00876800071").unwrap();
        assert_eq!(with_dashes, plain);
    }

    #[test]
    fn manual_rejects_verhoeff_mismatch() {
        // Flip last digit — Verhoeff must catch
        assert!(parse_manual_code("00876800072").is_err());
    }

    #[test]
    fn qr_decodes_real_device_payload() {
        // `MT:Y.K9042C00KA0648G00` — decoded once with our pipeline
        // and captured here as a regression lock. If the decode ever
        // drifts, this will catch it. (Values captured 2026-04-21.)
        let p = parse_qr("MT:Y.K9042C00KA0648G00").unwrap();
        assert_eq!(p.version, 0);
        assert_eq!(p.vendor_id, Some(0x2ECE));
        assert_eq!(p.product_id, Some(0x42D3));
        assert_eq!(p.commissioning_flow, CommissioningFlow::UserActionRequired);
        assert_eq!(p.rendezvous, 0x0E); // BLE | OnNetwork | WiFi-PAF
        assert_eq!(p.discriminator, 0xB00);
        assert_eq!(p.passcode, 67877405);
        assert!(!p.discriminator_short);
    }

    #[test]
    fn qr_roundtrip_our_own_encode_decode() {
        // Build 11 bytes with known field values, decode, verify.
        // vid=0xFFF1, pid=0x8001, flow=0, rendezvous=2, disc=0xF00,
        // passcode=20202021
        let mut bytes = [0u8; 11];
        let mut bw = BitWriter::new(&mut bytes);
        bw.write(0, 3, 0);               // version
        bw.write(3, 16, 0xFFF1);         // vendor_id
        bw.write(19, 16, 0x8001);        // product_id
        bw.write(35, 2, 0);              // flow
        bw.write(37, 8, 0x2);            // rendezvous BLE
        bw.write(45, 12, 0xF00);         // discriminator
        bw.write(57, 27, 20202021);      // passcode

        let br = BitReader::new(&bytes);
        assert_eq!(br.read(3, 16), 0xFFF1);
        assert_eq!(br.read(19, 16), 0x8001);
        assert_eq!(br.read(45, 12), 0xF00);
        assert_eq!(br.read(57, 27), 20202021);
    }

    struct BitWriter<'a> { data: &'a mut [u8] }
    impl<'a> BitWriter<'a> {
        fn new(d: &'a mut [u8]) -> Self { Self { data: d } }
        fn write(&mut self, offset: usize, count: usize, mut value: u32) {
            for i in 0..count {
                let bp = offset + i;
                let bit = (value & 1) as u8;
                self.data[bp / 8] |= bit << (bp % 8);
                value >>= 1;
            }
        }
    }

    #[test]
    fn qr_rejects_missing_prefix() {
        assert!(parse_qr("Y.K9042C00KA0648G00").is_err());
    }
}
