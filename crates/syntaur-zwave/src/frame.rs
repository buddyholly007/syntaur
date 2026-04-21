//! Z-Wave Serial API Data Frame codec.
//!
//! Wire format of a data frame, per the Z-Wave Public Spec (Sigma Designs,
//! later Silicon Labs; sections "Host Layer — Frame Layer" + "Host Layer
//! — Transfer Layer"):
//!
//! ```text
//!   ┌──────┬────────┬──────┬──────────┬─────────┬──────────┐
//!   │ SOF  │ Length │ Type │ Function │ Payload │ Checksum │
//!   │ 0x01 │  u8    │ u8   │  u8      │  N * u8 │   u8     │
//!   └──────┴────────┴──────┴──────────┴─────────┴──────────┘
//! ```
//!
//! - `Length` is the number of bytes that follow the length field itself
//!   up to and **including** the checksum byte minus one, i.e.
//!   `2 + payload.len() + 1 - 1 = payload.len() + 2`.  Every real
//!   library quotes the same rule: *length counts everything after
//!   itself excluding the checksum*. We use `length = payload.len() + 3`
//!   (Type + Function + payload), matching zwave-js.
//! - `Type` is 0x00 = Request, 0x01 = Response. `FrameKind` here.
//! - `Function` is the Serial API function id (e.g. 0x02 = GetInitData,
//!   0x13 = SendData). Enumerated by upper layers as command classes
//!   + controller operations land (weeks 3+).
//! - `Payload` is opaque at this layer.
//! - `Checksum` = XOR of all bytes from Length through the last payload
//!   byte, XOR'd with 0xFF. i.e. `start = 0xFF; for b in &data[1..end] { start ^= b; }`.
//!
//! The three control bytes (ACK / NAK / CAN) are not Data Frames — they
//! live as single bytes on the transport and are handled by the link
//! layer in `serial.rs`. This module covers *only* the framed data path.

use bytes::{BufMut, BytesMut};
use thiserror::Error;

pub const SOF: u8 = 0x01;
pub const ACK: u8 = 0x06;
pub const NAK: u8 = 0x15;
pub const CAN: u8 = 0x18;

/// Serial API frame "Type" byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// Host→controller request, or controller→host unsolicited event.
    Request = 0x00,
    /// Reply frame matching the previous Request.
    Response = 0x01,
}

impl FrameKind {
    pub fn from_byte(b: u8) -> Option<FrameKind> {
        match b {
            0x00 => Some(FrameKind::Request),
            0x01 => Some(FrameKind::Response),
            _ => None,
        }
    }
}

/// A decoded Z-Wave Serial API Data Frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub function: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn request(function: u8, payload: impl Into<Vec<u8>>) -> Frame {
        Frame {
            kind: FrameKind::Request,
            function,
            payload: payload.into(),
        }
    }

    pub fn response(function: u8, payload: impl Into<Vec<u8>>) -> Frame {
        Frame {
            kind: FrameKind::Response,
            function,
            payload: payload.into(),
        }
    }

    /// Serialize into on-wire bytes including SOF + length + checksum.
    pub fn encode(&self) -> BytesMut {
        // length = Type (1) + Function (1) + payload.len() — checksum
        // itself is NOT counted in `length`.
        //
        // Frame too long (>254 payload) would overflow u8; spec mandates
        // <= 252 payload bytes, callers should validate. We saturate and
        // let the receiver reject rather than panic here.
        let payload_len = self.payload.len().min(252);
        let length = (payload_len as u8).saturating_add(3);

        // Pre-size: SOF + Length + Type + Function + payload + checksum.
        let mut buf = BytesMut::with_capacity(5 + payload_len);
        buf.put_u8(SOF);
        buf.put_u8(length);
        buf.put_u8(self.kind as u8);
        buf.put_u8(self.function);
        buf.put_slice(&self.payload[..payload_len]);

        // Checksum covers everything after the SOF byte except the
        // checksum itself, starting from 0xFF.
        let checksum = compute_checksum(&buf[1..]);
        buf.put_u8(checksum);
        buf
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameParseError {
    #[error("buffer empty")]
    Empty,
    #[error("first byte {0:#04x} is not SOF")]
    NotSof(u8),
    #[error("length byte declares {declared} bytes, only {available} available")]
    ShortRead { declared: usize, available: usize },
    #[error("length byte {0} is below minimum 3 (Type + Function + Checksum)")]
    LengthTooSmall(u8),
    #[error("unknown frame kind byte {0:#04x}")]
    UnknownKind(u8),
    #[error("checksum mismatch — got {got:#04x}, expected {expected:#04x}")]
    BadChecksum { got: u8, expected: u8 },
}

/// Attempt to parse a single frame starting at `data[0]`.
///
/// Returns `(frame, bytes_consumed)` on success. `data` MUST start with
/// a SOF byte — the link layer strips ACK/NAK/CAN before calling this.
pub fn parse(data: &[u8]) -> Result<(Frame, usize), FrameParseError> {
    if data.is_empty() {
        return Err(FrameParseError::Empty);
    }
    if data[0] != SOF {
        return Err(FrameParseError::NotSof(data[0]));
    }
    if data.len() < 2 {
        return Err(FrameParseError::ShortRead {
            declared: 2,
            available: data.len(),
        });
    }
    let length = data[1];
    if length < 3 {
        return Err(FrameParseError::LengthTooSmall(length));
    }
    // Total frame is SOF + Length + (length - 1) data + checksum
    //                    1        1     length - 1         1
    // Equivalently: 2 + (length + 1 - 1) + ... simplifies to 2 + length.
    // zwave-js: `totalLength = length + 2`. Use the same.
    let total = 2usize + length as usize;
    if data.len() < total {
        return Err(FrameParseError::ShortRead {
            declared: total,
            available: data.len(),
        });
    }

    let kind_byte = data[2];
    let kind = FrameKind::from_byte(kind_byte).ok_or(FrameParseError::UnknownKind(kind_byte))?;
    let function = data[3];
    // Payload is everything from index 4 up to (but not including) the
    // checksum at index `total - 1`.
    let payload = data[4..total - 1].to_vec();

    // Verify checksum.
    let got = data[total - 1];
    let expected = compute_checksum(&data[1..total - 1]);
    if got != expected {
        return Err(FrameParseError::BadChecksum { got, expected });
    }

    Ok((Frame { kind, function, payload }, total))
}

/// Checksum = 0xFF XOR'd with every byte in `bytes`.
///
/// `bytes` should start at the Length byte and end at the last payload
/// byte (i.e. everything between SOF and the checksum). Matches the
/// spec and zwave-js / open-zwave's `updateChecksum` helper.
pub fn compute_checksum(bytes: &[u8]) -> u8 {
    let mut checksum: u8 = 0xFF;
    for &b in bytes {
        checksum ^= b;
    }
    checksum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured byte stream of a real `GetCapabilities` Request frame:
    ///   SOF 0x03 0x00 0x07 0xFB
    /// length=3, Request, function=0x07 (FUNC_GET_CAPABILITIES), empty payload.
    /// Checksum: 0xFF ^ 0x03 ^ 0x00 ^ 0x07 = 0xFB. ✓
    const GET_CAPABILITIES_REQUEST: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];

    #[test]
    fn checksum_xor_matches_spec() {
        // 0xFF ^ 0x03 ^ 0x00 ^ 0x07 = 0xFB
        assert_eq!(compute_checksum(&[0x03, 0x00, 0x07]), 0xFB);
        // Empty slice leaves the initial 0xFF unchanged.
        assert_eq!(compute_checksum(&[]), 0xFF);
    }

    #[test]
    fn encode_round_trips_known_request() {
        let f = Frame::request(0x07, vec![]);
        let bytes = f.encode();
        assert_eq!(&bytes[..], GET_CAPABILITIES_REQUEST);
    }

    #[test]
    fn parse_round_trips_known_request() {
        let (f, n) = parse(GET_CAPABILITIES_REQUEST).expect("parse");
        assert_eq!(n, GET_CAPABILITIES_REQUEST.len());
        assert_eq!(f.kind, FrameKind::Request);
        assert_eq!(f.function, 0x07);
        assert!(f.payload.is_empty());
    }

    #[test]
    fn parse_rejects_bad_checksum() {
        let mut bad = GET_CAPABILITIES_REQUEST.to_vec();
        *bad.last_mut().unwrap() ^= 0x01;
        let err = parse(&bad).unwrap_err();
        matches!(err, FrameParseError::BadChecksum { .. });
    }

    #[test]
    fn parse_rejects_non_sof() {
        let err = parse(&[0x02, 0x03, 0x00, 0x07, 0xFB]).unwrap_err();
        assert_eq!(err, FrameParseError::NotSof(0x02));
    }

    #[test]
    fn parse_requires_full_frame() {
        // Length byte says 3, but only 3 bytes follow (including checksum
        // — total frame is 5, we give it 4).
        let err = parse(&[0x01, 0x03, 0x00, 0x07]).unwrap_err();
        matches!(err, FrameParseError::ShortRead { .. });
    }

    #[test]
    fn encode_parse_round_trip_with_payload() {
        // Fake SendData(node=5, command_class=0x25 SwitchBinary, cmd=Set, value=0xFF).
        let f = Frame::request(0x13, vec![0x05, 0x03, 0x25, 0x01, 0xFF]);
        let bytes = f.encode();
        let (parsed, n) = parse(&bytes).expect("parse encoded");
        assert_eq!(n, bytes.len());
        assert_eq!(parsed, f);
    }

    #[test]
    fn parse_response_kind() {
        // 0x01 length=0x03 type=0x01(response) func=0x07 cksum=0xFA
        // 0xFF ^ 0x03 ^ 0x01 ^ 0x07 = 0xFA
        let (f, _n) = parse(&[0x01, 0x03, 0x01, 0x07, 0xFA]).expect("parse");
        assert_eq!(f.kind, FrameKind::Response);
    }

    #[test]
    fn length_too_small_rejected() {
        // Length of 2 is never legal (min is 3: Type + Function + implicit
        // checksum). 0xFF ^ 0x02 ^ 0x00 = 0xFD.
        let err = parse(&[0x01, 0x02, 0x00, 0xFD]).unwrap_err();
        assert_eq!(err, FrameParseError::LengthTooSmall(2));
    }

    #[test]
    fn unknown_kind_rejected() {
        // Kind byte 0x05 is neither Request (0x00) nor Response (0x01).
        // 0xFF ^ 0x03 ^ 0x05 ^ 0x07 = 0xFE
        let err = parse(&[0x01, 0x03, 0x05, 0x07, 0xFE]).unwrap_err();
        assert_eq!(err, FrameParseError::UnknownKind(0x05));
    }
}
