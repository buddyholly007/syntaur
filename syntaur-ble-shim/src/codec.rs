//! ESPHome native-API plaintext framing + minimal protobuf encoder/decoder.
//!
//! Wire format (plaintext mode):
//!   [0x00] [varint: payload_length] [varint: message_type_id] [payload...]
//!
//! Vendored from syntaur-gateway/src/voice/esphome_api.rs — kept private to
//! this crate to avoid dragging gateway internals into the shim's build graph.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Hard cap on a single ESPHome message payload. Real proxy traffic is well
/// under 4KB per frame; 256KB is generous headroom and bounds memory pressure
/// from a hostile peer.
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

// ── Varints ─────────────────────────────────────────────────────────────────

pub fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
    buf
}

pub fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate().take(10) {
        let part = (byte & 0x7F) as u64;
        if shift >= 64 || (shift == 63 && part > 1) {
            return None;
        }
        value |= part << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None
}

pub async fn read_varint<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<u64> {
    let mut value: u64 = 0;
    let mut shift = 0;
    // 10 bytes is the max varint length for u64.
    for _ in 0..10 {
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf).await.ok()?;
        let part = (buf[0] & 0x7F) as u64;
        // Guard: if shift==63 and part > 1, the result would overflow u64.
        if shift >= 64 || (shift == 63 && part > 1) {
            return None;
        }
        value |= part << shift;
        if buf[0] & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

// ── Plaintext frames ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RawMessage {
    pub msg_type: u32,
    pub payload: Vec<u8>,
}

/// Read one plaintext-framed message from a TCP stream.
pub async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<RawMessage> {
    let mut preamble = [0u8; 1];
    reader.read_exact(&mut preamble).await.ok()?;
    if preamble[0] != 0x00 {
        log::warn!(
            "[codec] unexpected preamble: 0x{:02x} (this shim only supports plaintext)",
            preamble[0]
        );
        return None;
    }
    let payload_len = read_varint(reader).await? as usize;
    if payload_len > MAX_PAYLOAD_BYTES {
        log::warn!(
            "[codec] peer announced oversize payload: {} bytes (cap {})",
            payload_len,
            MAX_PAYLOAD_BYTES
        );
        return None;
    }
    let msg_type = read_varint(reader).await? as u32;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await.ok()?;
    }
    log::trace!("[codec] recv: type={} len={}", msg_type, payload_len);
    Some(RawMessage { msg_type, payload })
}

/// Write one plaintext-framed message to a TCP stream.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg_type: u32,
    payload: &[u8],
) -> Result<(), std::io::Error> {
    let mut frame = Vec::with_capacity(8 + payload.len());
    frame.push(0x00);
    frame.extend(encode_varint(payload.len() as u64));
    frame.extend(encode_varint(msg_type as u64));
    frame.extend_from_slice(payload);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    log::trace!("[codec] sent: type={} len={}", msg_type, payload.len());
    Ok(())
}

// ── Minimal protobuf encoder / decoder ──────────────────────────────────────

pub struct ProtoEncoder {
    buf: Vec<u8>,
}

impl ProtoEncoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn encode_uint32(&mut self, field: u32, value: u32) {
        if value == 0 {
            return;
        }
        self.buf.extend(encode_varint((field as u64) << 3));
        self.buf.extend(encode_varint(value as u64));
    }

    pub fn encode_uint64(&mut self, field: u32, value: u64) {
        if value == 0 {
            return;
        }
        self.buf.extend(encode_varint((field as u64) << 3));
        self.buf.extend(encode_varint(value));
    }

    pub fn encode_int32(&mut self, field: u32, value: i32) {
        if value == 0 {
            return;
        }
        self.buf.extend(encode_varint((field as u64) << 3));
        // protobuf int32 is encoded as varint of u64 (sign-extended via cast)
        self.buf.extend(encode_varint(value as u64));
    }

    /// Encode a `sint32` (zig-zag varint). Negative values stay compact.
    pub fn encode_sint32(&mut self, field: u32, value: i32) {
        if value == 0 {
            return;
        }
        self.buf.extend(encode_varint((field as u64) << 3));
        let zz = ((value << 1) ^ (value >> 31)) as u32 as u64;
        self.buf.extend(encode_varint(zz));
    }

    pub fn encode_bool(&mut self, field: u32, value: bool) {
        if !value {
            return;
        }
        self.buf.extend(encode_varint((field as u64) << 3));
        self.buf.push(1);
    }

    pub fn encode_string(&mut self, field: u32, value: &str) {
        if value.is_empty() {
            return;
        }
        self.buf.extend(encode_varint(((field as u64) << 3) | 2));
        self.buf.extend(encode_varint(value.len() as u64));
        self.buf.extend_from_slice(value.as_bytes());
    }

    pub fn encode_bytes(&mut self, field: u32, value: &[u8]) {
        if value.is_empty() {
            return;
        }
        self.buf.extend(encode_varint(((field as u64) << 3) | 2));
        self.buf.extend(encode_varint(value.len() as u64));
        self.buf.extend_from_slice(value);
    }

    /// Encode a sub-message: field of type length-delimited containing arbitrary bytes.
    pub fn encode_message(&mut self, field: u32, value: &[u8]) {
        self.buf.extend(encode_varint(((field as u64) << 3) | 2));
        self.buf.extend(encode_varint(value.len() as u64));
        self.buf.extend_from_slice(value);
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct ProtoDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

#[derive(Debug)]
pub enum ProtoField<'a> {
    Varint(u32, u64),
    Bytes(u32, &'a [u8]),
    Fixed32(u32, &'a [u8]),
    Fixed64(u32, &'a [u8]),
}

impl<'a> ProtoDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn next_field(&mut self) -> Option<ProtoField<'a>> {
        if self.pos >= self.data.len() {
            return None;
        }
        let (tag, consumed) = decode_varint(&self.data[self.pos..])?;
        self.pos += consumed;
        let field_number = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;
        match wire_type {
            0 => {
                let (value, consumed) = decode_varint(&self.data[self.pos..])?;
                self.pos += consumed;
                Some(ProtoField::Varint(field_number, value))
            }
            2 => {
                let (len, consumed) = decode_varint(&self.data[self.pos..])?;
                self.pos += consumed;
                let len = len as usize;
                if self.pos + len > self.data.len() {
                    return None;
                }
                let slice = &self.data[self.pos..self.pos + len];
                self.pos += len;
                Some(ProtoField::Bytes(field_number, slice))
            }
            5 => {
                if self.pos + 4 > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + 4];
                self.pos += 4;
                Some(ProtoField::Fixed32(field_number, bytes))
            }
            1 => {
                if self.pos + 8 > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + 8];
                self.pos += 8;
                Some(ProtoField::Fixed64(field_number, bytes))
            }
            _ => {
                // Forward-compatible: skip unknown wire types instead of aborting.
                log::warn!(
                    "[codec] unknown wire type {} for field {} — skipping rest of message",
                    wire_type,
                    field_number
                );
                self.pos = self.data.len();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 16383, 16384, 1_000_000, u64::MAX] {
            let buf = encode_varint(v);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v);
        }
    }

    #[test]
    fn proto_string_roundtrip() {
        let mut enc = ProtoEncoder::new();
        enc.encode_string(1, "hello");
        enc.encode_uint32(2, 42);
        enc.encode_bool(3, true);
        let buf = enc.finish();

        let mut dec = ProtoDecoder::new(&buf);
        let mut got_string = None;
        let mut got_uint = None;
        let mut got_bool = None;
        while let Some(f) = dec.next_field() {
            match f {
                ProtoField::Bytes(1, b) => got_string = Some(std::str::from_utf8(b).unwrap().to_string()),
                ProtoField::Varint(2, v) => got_uint = Some(v as u32),
                ProtoField::Varint(3, v) => got_bool = Some(v != 0),
                _ => {}
            }
        }
        assert_eq!(got_string.as_deref(), Some("hello"));
        assert_eq!(got_uint, Some(42));
        assert_eq!(got_bool, Some(true));
    }
}
