//! ESPHome native API protocol — framing and message codec.
//!
//! Wire format (plaintext mode):
//!   [0x00] [varint: payload_length] [varint: message_type_id] [payload...]
//!
//! We hand-code the protobuf messages we need rather than using prost codegen.
//! Only ~15 message types are needed for the voice assistant flow.

use log::{info, warn};

#[allow(unused_macros)]
macro_rules! debug {
    ($($arg:tt)*) => { log::debug!($($arg)*) };
}
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ── Noise encrypted transport ───────────────────────────────────────────────

/// ESPHome Noise protocol: Noise_NNpsk0_25519_ChaChaPoly_SHA256
/// Handshake:
///   1. Client sends: 0x01 0x00 0x00 (hello)
///   2. Server responds: 0x01 + 2-byte BE length + frame (chosen proto byte + padding)
///   3. Client sends handshake message (-> e, psk)
///   4. Server responds with handshake message (<- e, ee)
///   5. Transport is now encrypted
///
/// Each encrypted frame: 0x01 + 2-byte BE length + encrypted(msg_type_hi, msg_type_lo, data_len_hi, data_len_lo, data...)

pub struct NoiseTransport<S> {
    stream: S,
    transport: snow::TransportState,
    read_buf: Vec<u8>,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin> NoiseTransport<S> {
    /// Perform the Noise handshake and return an encrypted transport.
    pub async fn handshake(mut stream: S, psk: &[u8; 32]) -> Result<Self, String> {
        let params: snow::params::NoiseParams = "Noise_NNpsk0_25519_ChaChaPoly_SHA256"
            .parse()
            .map_err(|e| format!("noise pattern: {}", e))?;

        let prologue = b"NoiseAPIInit\x00\x00";

        let mut handshake = snow::Builder::new(params)
            .psk(0, psk)
            .map_err(|e| format!("noise psk: {}", e))?
            .prologue(prologue)
            .map_err(|e| format!("noise prologue: {}", e))?
            .build_initiator()
            .map_err(|e| format!("noise build: {}", e))?;

        // Step 1: Send NOISE_HELLO (bare 3-byte frame)
        stream.write_all(&[0x01, 0x00, 0x00]).await
            .map_err(|e| format!("noise hello: {}", e))?;

        // Step 2: Send client hello with Noise handshake message
        // Frame: 0x01 + 2-byte BE len + (0x00 indicator + handshake_msg)
        let mut hs_msg = vec![0u8; 256];
        let hs_len = handshake.write_message(&[], &mut hs_msg)
            .map_err(|e| format!("noise write_message: {}", e))?;
        let payload_len = 1 + hs_len; // 0x00 indicator byte + handshake
        let mut frame = Vec::with_capacity(3 + payload_len);
        frame.push(0x01);
        frame.extend_from_slice(&(payload_len as u16).to_be_bytes());
        frame.push(0x00); // protocol indicator
        frame.extend_from_slice(&hs_msg[..hs_len]);
        stream.write_all(&frame).await
            .map_err(|e| format!("noise client hello: {}", e))?;
        stream.flush().await.map_err(|e| format!("noise flush: {}", e))?;

        // Step 3: Read server hello (NOT a Noise message — just informational)
        let server_hello = read_noise_frame(&mut stream).await
            .ok_or("no server hello")?;
        if server_hello.is_empty() || server_hello[0] != 0x01 {
            return Err(format!(
                "server chose unsupported protocol: 0x{:02x}",
                server_hello.first().copied().unwrap_or(0)
            ));
        }
        // Extract device name (after the 0x01 byte, null-terminated)
        let name_bytes = &server_hello[1..];
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
        let device_name = String::from_utf8_lossy(&name_bytes[..name_end]);
        info!("[noise] server hello: device={}", device_name);

        // Step 4: Read server handshake (Noise message with status byte prefix)
        let server_hs = read_noise_frame(&mut stream).await
            .ok_or("no server handshake")?;
        if server_hs.is_empty() {
            return Err("empty server handshake".to_string());
        }
        if server_hs[0] != 0x00 {
            return Err(format!("server handshake error: status=0x{:02x}", server_hs[0]));
        }
        // Strip status byte, feed rest to Noise
        let mut payload = vec![0u8; 256];
        let _payload_len = handshake.read_message(&server_hs[1..], &mut payload)
            .map_err(|e| format!("noise read_message: {}", e))?;

        let transport = handshake.into_transport_mode()
            .map_err(|e| format!("noise transport: {}", e))?;

        info!("[noise] handshake complete, transport encrypted");

        Ok(Self {
            stream,
            transport,
            read_buf: Vec::new(),
        })
    }

}

/// Read one raw noise frame (0x01 + 2-byte BE len + payload).
async fn read_noise_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<Vec<u8>> {
    let mut preamble = [0u8; 1];
    reader.read_exact(&mut preamble).await.ok()?;
    if preamble[0] != 0x01 {
        warn!("[noise] bad preamble: 0x{:02x}", preamble[0]);
        return None;
    }
    let mut len_buf = [0u8; 2];
    reader.read_exact(&mut len_buf).await.ok()?;
    let frame_len = u16::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; frame_len];
    if frame_len > 0 {
        reader.read_exact(&mut payload).await.ok()?;
    }
    Some(payload)
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin> NoiseTransport<S> {
    /// Read one decrypted ESPHome message.
    pub async fn read_message(&mut self) -> Option<RawMessage> {
        // Read noise frame: 0x01 + 2-byte BE length + encrypted data
        let mut preamble = [0u8; 1];
        self.stream.read_exact(&mut preamble).await.ok()?;
        if preamble[0] != 0x01 {
            warn!("[noise] bad preamble: 0x{:02x}", preamble[0]);
            return None;
        }

        let mut len_buf = [0u8; 2];
        self.stream.read_exact(&mut len_buf).await.ok()?;
        let frame_len = u16::from_be_bytes(len_buf) as usize;

        let mut encrypted = vec![0u8; frame_len];
        if frame_len > 0 {
            self.stream.read_exact(&mut encrypted).await.ok()?;
        }

        // Decrypt
        let mut decrypted = vec![0u8; frame_len + 16]; // extra space for tag
        let len = self
            .transport
            .read_message(&encrypted, &mut decrypted)
            .map_err(|e| {
                warn!("[noise] decrypt failed: {}", e);
                e
            })
            .ok()?;

        if len < 4 {
            warn!("[noise] decrypted frame too short: {} bytes", len);
            return None;
        }

        // Decrypted format: msg_type (2 bytes BE) + data_len (2 bytes BE) + data
        let msg_type = u16::from_be_bytes([decrypted[0], decrypted[1]]) as u32;
        let data_len = u16::from_be_bytes([decrypted[2], decrypted[3]]) as usize;

        let payload = if data_len > 0 && 4 + data_len <= len {
            decrypted[4..4 + data_len].to_vec()
        } else {
            Vec::new()
        };

        debug!("[noise] recv: type={} len={}", msg_type, payload.len());
        Some(RawMessage { msg_type, payload })
    }

    /// Write one encrypted ESPHome message.
    pub async fn write_message(&mut self, msg_type: u32, payload: &[u8]) -> Result<(), String> {
        // Build plaintext: msg_type (2 bytes BE) + data_len (2 bytes BE) + data
        let mut plaintext = Vec::with_capacity(4 + payload.len());
        plaintext.extend_from_slice(&(msg_type as u16).to_be_bytes());
        plaintext.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        plaintext.extend_from_slice(payload);

        // Encrypt
        let mut encrypted = vec![0u8; plaintext.len() + 64]; // space for overhead
        let len = self
            .transport
            .write_message(&plaintext, &mut encrypted)
            .map_err(|e| format!("noise encrypt: {}", e))?;

        // Send as noise frame
        let mut frame = Vec::with_capacity(3 + len);
        frame.push(0x01);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
        frame.extend_from_slice(&encrypted[..len]);

        self.stream
            .write_all(&frame)
            .await
            .map_err(|e| format!("noise write: {}", e))?;
        self.stream
            .flush()
            .await
            .map_err(|e| format!("noise flush: {}", e))?;

        debug!("[noise] sent: type={} len={}", msg_type, payload.len());
        Ok(())
    }
}

// ── Varint encoding/decoding ────────────────────────────────────────────────

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
    for (i, &byte) in data.iter().enumerate() {
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

pub async fn read_varint<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<u64> {
    let mut value: u64 = 0;
    let mut shift = 0;
    loop {
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf).await.ok()?;
        value |= ((buf[0] & 0x7F) as u64) << shift;
        if buf[0] & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

// ── Message type IDs ────────────────────────────────────────────────────────

pub const MSG_HELLO_REQUEST: u32 = 1;
pub const MSG_HELLO_RESPONSE: u32 = 2;
pub const MSG_DISCONNECT_REQUEST: u32 = 5;
pub const MSG_DISCONNECT_RESPONSE: u32 = 6;
pub const MSG_PING_REQUEST: u32 = 7;
pub const MSG_PING_RESPONSE: u32 = 8;
pub const MSG_DEVICE_INFO_REQUEST: u32 = 9;
pub const MSG_DEVICE_INFO_RESPONSE: u32 = 10;
pub const MSG_LIST_ENTITIES_REQUEST: u32 = 11;
pub const MSG_LIST_ENTITIES_DONE: u32 = 19;
pub const MSG_SUBSCRIBE_VOICE_ASSISTANT: u32 = 89;
pub const MSG_VOICE_ASSISTANT_REQUEST: u32 = 90;
pub const MSG_VOICE_ASSISTANT_RESPONSE: u32 = 91;
pub const MSG_VOICE_ASSISTANT_EVENT: u32 = 92;
pub const MSG_VOICE_ASSISTANT_AUDIO: u32 = 106;
pub const MSG_VOICE_ASSISTANT_TIMER_EVENT: u32 = 115;
pub const MSG_VOICE_ASSISTANT_ANNOUNCE: u32 = 119;
pub const MSG_VOICE_ASSISTANT_ANNOUNCE_FINISHED: u32 = 120;
pub const MSG_VOICE_ASSISTANT_CONFIG_REQUEST: u32 = 121;
pub const MSG_VOICE_ASSISTANT_CONFIG_RESPONSE: u32 = 122;
pub const MSG_VOICE_ASSISTANT_SET_CONFIG: u32 = 123;

// ── Voice assistant event types ─────────────────────────────────────────────

pub const EVENT_ERROR: u32 = 0;
pub const EVENT_RUN_START: u32 = 1;
pub const EVENT_RUN_END: u32 = 2;
pub const EVENT_STT_START: u32 = 3;
pub const EVENT_STT_END: u32 = 4;
pub const EVENT_INTENT_START: u32 = 5;
pub const EVENT_INTENT_END: u32 = 6;
pub const EVENT_TTS_START: u32 = 7;
pub const EVENT_TTS_END: u32 = 8;
pub const EVENT_TTS_STREAM_START: u32 = 98;
pub const EVENT_TTS_STREAM_END: u32 = 99;

// ── Raw message ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RawMessage {
    pub msg_type: u32,
    pub payload: Vec<u8>,
}

/// Read one plaintext-framed message from a TCP stream.
pub async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<RawMessage> {
    // Read preamble byte (0x00 for plaintext)
    let mut preamble = [0u8; 1];
    reader.read_exact(&mut preamble).await.ok()?;
    if preamble[0] != 0x00 {
        warn!("[esphome] unexpected preamble: 0x{:02x} (noise encryption not supported)", preamble[0]);
        return None;
    }

    let payload_len = read_varint(reader).await? as usize;
    let msg_type = read_varint(reader).await? as u32;

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await.ok()?;
    }

    debug!("[esphome] recv: type={} len={}", msg_type, payload_len);
    Some(RawMessage { msg_type, payload })
}

/// Write one plaintext-framed message to a TCP stream.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg_type: u32,
    payload: &[u8],
) -> Result<(), String> {
    let mut frame = Vec::new();
    frame.push(0x00); // plaintext preamble
    frame.extend(encode_varint(payload.len() as u64));
    frame.extend(encode_varint(msg_type as u64));
    frame.extend_from_slice(payload);

    writer
        .write_all(&frame)
        .await
        .map_err(|e| format!("write: {}", e))?;
    writer.flush().await.map_err(|e| format!("flush: {}", e))?;

    debug!("[esphome] sent: type={} len={}", msg_type, payload.len());
    Ok(())
}

// ── Minimal protobuf encoding/decoding ──────────────────────────────────────
//
// We only need a few field types:
//   - varint (field types: uint32, bool, enum)
//   - length-delimited (string, bytes)
//
// Protobuf wire format:
//   field_tag = (field_number << 3) | wire_type
//   wire_type 0 = varint, wire_type 2 = length-delimited

pub struct ProtoEncoder {
    buf: Vec<u8>,
}

impl ProtoEncoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn encode_uint32(&mut self, field: u32, value: u32) {
        if value == 0 {
            return; // protobuf default, omit
        }
        self.buf.extend(encode_varint(((field as u64) << 3) | 0));
        self.buf.extend(encode_varint(value as u64));
    }

    pub fn encode_bool(&mut self, field: u32, value: bool) {
        if !value {
            return;
        }
        self.buf.extend(encode_varint(((field as u64) << 3) | 0));
        self.buf.push(1);
    }

    pub fn encode_string(&mut self, field: u32, value: &str) {
        if value.is_empty() {
            return;
        }
        self.buf
            .extend(encode_varint(((field as u64) << 3) | 2));
        self.buf.extend(encode_varint(value.len() as u64));
        self.buf.extend_from_slice(value.as_bytes());
    }

    pub fn encode_bytes(&mut self, field: u32, value: &[u8]) {
        if value.is_empty() {
            return;
        }
        self.buf
            .extend(encode_varint(((field as u64) << 3) | 2));
        self.buf.extend(encode_varint(value.len() as u64));
        self.buf.extend_from_slice(value);
    }

    pub fn encode_float(&mut self, field: u32, value: f32) {
        if value == 0.0 {
            return;
        }
        // wire type 5 = 32-bit
        self.buf
            .extend(encode_varint(((field as u64) << 3) | 5));
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct ProtoDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ProtoDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Read next field: returns (field_number, wire_type, value).
    /// For varint: value is the u64 directly.
    /// For length-delimited: returns the byte slice.
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
                // varint
                let (value, consumed) = decode_varint(&self.data[self.pos..])?;
                self.pos += consumed;
                Some(ProtoField::Varint(field_number, value))
            }
            2 => {
                // length-delimited
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
                // 32-bit
                if self.pos + 4 > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + 4];
                self.pos += 4;
                Some(ProtoField::Fixed32(field_number, bytes))
            }
            1 => {
                // 64-bit
                if self.pos + 8 > self.data.len() {
                    return None;
                }
                self.pos += 8;
                Some(ProtoField::Varint(field_number, 0)) // skip
            }
            _ => {
                warn!("[proto] unknown wire type {} for field {}", wire_type, field_number);
                None
            }
        }
    }
}

#[derive(Debug)]
pub enum ProtoField<'a> {
    Varint(u32, u64),
    Bytes(u32, &'a [u8]),
    Fixed32(u32, &'a [u8]),
}

// ── Typed message builders ──────────────────────────────────────────────────

/// Build HelloRequest: client_info (1), api_version_major (2), api_version_minor (3)
pub fn build_hello_request(client_info: &str) -> Vec<u8> {
    let mut enc = ProtoEncoder::new();
    enc.encode_string(1, client_info);
    enc.encode_uint32(2, 1);  // api_version_major
    enc.encode_uint32(3, 10); // api_version_minor
    enc.finish()
}

/// Build SubscribeVoiceAssistantRequest: subscribe (1), flags (2)
pub fn build_subscribe_voice_assistant(subscribe: bool, api_audio: bool) -> Vec<u8> {
    let mut enc = ProtoEncoder::new();
    enc.encode_bool(1, subscribe);
    if api_audio {
        enc.encode_uint32(2, 1); // VOICE_ASSISTANT_SUBSCRIBE_API_AUDIO
    }
    enc.finish()
}

/// Build VoiceAssistantResponse: port (1), error (2)
pub fn build_voice_assistant_response(port: u32, error: bool) -> Vec<u8> {
    let mut enc = ProtoEncoder::new();
    enc.encode_uint32(1, port);
    enc.encode_bool(2, error);
    enc.finish()
}

/// Build VoiceAssistantEventResponse: event_type (1), data (2, repeated)
pub fn build_voice_assistant_event(event_type: u32, data: &[(&str, &str)]) -> Vec<u8> {
    let mut enc = ProtoEncoder::new();
    enc.encode_uint32(1, event_type);
    // Each data entry is a sub-message with name (1) and value (2)
    for (name, value) in data {
        let mut sub = ProtoEncoder::new();
        sub.encode_string(1, name);
        sub.encode_string(2, value);
        let sub_bytes = sub.finish();
        enc.encode_bytes(2, &sub_bytes);
    }
    enc.finish()
}

/// Build VoiceAssistantAudio: data (1), end (2)
pub fn build_voice_assistant_audio(audio_data: &[u8], end: bool) -> Vec<u8> {
    let mut enc = ProtoEncoder::new();
    enc.encode_bytes(1, audio_data);
    enc.encode_bool(2, end);
    enc.finish()
}

// ── Message parsers ─────────────────────────────────────────────────────────

/// Parse HelloResponse: api_version_major (1), api_version_minor (2), server_info (3), name (4)
pub struct HelloResponseData {
    pub api_version_major: u32,
    pub api_version_minor: u32,
    pub server_info: String,
    pub name: String,
}

pub fn parse_hello_response(payload: &[u8]) -> HelloResponseData {
    let mut result = HelloResponseData {
        api_version_major: 0,
        api_version_minor: 0,
        server_info: String::new(),
        name: String::new(),
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(field) = dec.next_field() {
        match field {
            ProtoField::Varint(1, v) => result.api_version_major = v as u32,
            ProtoField::Varint(2, v) => result.api_version_minor = v as u32,
            ProtoField::Bytes(3, b) => result.server_info = String::from_utf8_lossy(b).to_string(),
            ProtoField::Bytes(4, b) => result.name = String::from_utf8_lossy(b).to_string(),
            _ => {}
        }
    }
    result
}

/// Parse DeviceInfoResponse — we only care about voice_assistant_feature_flags (17)
pub struct DeviceInfoData {
    pub name: String,
    pub voice_assistant_feature_flags: u32,
}

pub fn parse_device_info_response(payload: &[u8]) -> DeviceInfoData {
    let mut result = DeviceInfoData {
        name: String::new(),
        voice_assistant_feature_flags: 0,
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(field) = dec.next_field() {
        match field {
            ProtoField::Bytes(2, b) => result.name = String::from_utf8_lossy(b).to_string(),
            ProtoField::Varint(17, v) => result.voice_assistant_feature_flags = v as u32,
            _ => {}
        }
    }
    result
}

/// Parse VoiceAssistantRequest: start (1), conversation_id (2), flags (3), wake_word_phrase (5)
pub struct VoiceAssistantRequestData {
    pub start: bool,
    pub conversation_id: String,
    pub flags: u32,
    pub wake_word_phrase: String,
}

pub fn parse_voice_assistant_request(payload: &[u8]) -> VoiceAssistantRequestData {
    let mut result = VoiceAssistantRequestData {
        start: false,
        conversation_id: String::new(),
        flags: 0,
        wake_word_phrase: String::new(),
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(field) = dec.next_field() {
        match field {
            ProtoField::Varint(1, v) => result.start = v != 0,
            ProtoField::Bytes(2, b) => result.conversation_id = String::from_utf8_lossy(b).to_string(),
            ProtoField::Varint(3, v) => result.flags = v as u32,
            ProtoField::Bytes(5, b) => result.wake_word_phrase = String::from_utf8_lossy(b).to_string(),
            _ => {}
        }
    }
    result
}

/// Parse VoiceAssistantAudio: data (1), end (2)
pub struct VoiceAssistantAudioData {
    pub data: Vec<u8>,
    pub end: bool,
}

pub fn parse_voice_assistant_audio(payload: &[u8]) -> VoiceAssistantAudioData {
    let mut result = VoiceAssistantAudioData {
        data: Vec::new(),
        end: false,
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(field) = dec.next_field() {
        match field {
            ProtoField::Bytes(1, b) => result.data = b.to_vec(),
            ProtoField::Varint(2, v) => result.end = v != 0,
            _ => {}
        }
    }
    result
}

/// Parse SubscribeVoiceAssistantRequest: subscribe (1), flags (2)
pub struct SubscribeVoiceAssistantData {
    pub subscribe: bool,
    pub flags: u32,
}

pub fn parse_subscribe_voice_assistant(payload: &[u8]) -> SubscribeVoiceAssistantData {
    let mut result = SubscribeVoiceAssistantData {
        subscribe: false,
        flags: 0,
    };
    let mut dec = ProtoDecoder::new(payload);
    while let Some(field) = dec.next_field() {
        match field {
            ProtoField::Varint(1, v) => result.subscribe = v != 0,
            ProtoField::Varint(2, v) => result.flags = v as u32,
            _ => {}
        }
    }
    result
}
