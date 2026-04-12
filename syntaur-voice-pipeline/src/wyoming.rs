//! Wyoming protocol implementation.
//!
//! Wire format per the rhasspy/wyoming spec:
//!
//! ```text
//! {"type":"audio-chunk","version":"1.0","data_length":0,"payload_length":3200}\n
//! <3200 bytes of raw PCM audio>
//! ```
//!
//! 1. JSON header line terminated by `\n` — contains type, version,
//!    data_length (JSON metadata bytes), payload_length (binary bytes)
//! 2. `data_length` bytes of UTF-8 JSON (the structured data object)
//! 3. `payload_length` bytes of raw binary (e.g., audio PCM)

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct WyomingMessage {
    pub msg_type: String,
    pub data: serde_json::Value,
    /// Binary payload (audio data).
    pub payload: Vec<u8>,
}

impl WyomingMessage {
    pub fn new(msg_type: &str) -> Self {
        Self {
            msg_type: msg_type.to_string(),
            data: serde_json::Value::Null,
            payload: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    pub fn with_payload(mut self, payload: Vec<u8>) -> Self {
        self.payload = payload;
        self
    }

    pub fn transcript(text: &str) -> Self {
        Self::new("transcript").with_data(serde_json::json!({"text": text}))
    }

    pub fn synthesize(text: &str) -> Self {
        Self::new("synthesize").with_data(serde_json::json!({"text": text}))
    }

    pub fn audio_start(rate: u32, width: u16, channels: u16) -> Self {
        Self::new("audio-start").with_data(serde_json::json!({
            "rate": rate, "width": width, "channels": channels
        }))
    }

    pub fn audio_chunk(pcm: Vec<u8>) -> Self {
        Self::new("audio-chunk").with_payload(pcm)
    }

    pub fn audio_stop() -> Self {
        Self::new("audio-stop")
    }

    pub fn error(text: &str) -> Self {
        Self::new("error").with_data(serde_json::json!({"text": text}))
    }
}

/// Header sent on the wire (JSON line).
#[derive(Serialize, Deserialize)]
struct WireHeader {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    data_length: usize,
    #[serde(default)]
    payload_length: usize,
}

/// Read one Wyoming message from a TCP stream.
pub async fn read_message(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<WyomingMessage> {
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) => return None,
        Ok(_) => {}
        Err(e) => {
            warn!("[wyoming] read error: {}", e);
            return None;
        }
    }

    let header: WireHeader = match serde_json::from_str(line.trim()) {
        Ok(h) => h,
        Err(e) => {
            warn!("[wyoming] parse error: {} line='{}'", e, line.trim().chars().take(100).collect::<String>());
            return None;
        }
    };

    // Read data bytes (JSON metadata)
    let data = if header.data_length > 0 {
        let mut buf = vec![0u8; header.data_length];
        if let Err(e) = reader.read_exact(&mut buf).await {
            warn!("[wyoming] data read error: {}", e);
            return None;
        }
        match serde_json::from_slice(&buf) {
            Ok(v) => v,
            Err(e) => {
                warn!("[wyoming] data parse error: {}", e);
                serde_json::Value::Null
            }
        }
    } else {
        serde_json::Value::Null
    };

    // Read binary payload (audio)
    let payload = if header.payload_length > 0 {
        let mut buf = vec![0u8; header.payload_length];
        if let Err(e) = reader.read_exact(&mut buf).await {
            warn!("[wyoming] payload read error: {}", e);
            return None;
        }
        buf
    } else {
        Vec::new()
    };

    debug!("[wyoming] recv: type={} data_len={} payload_len={}", header.msg_type, header.data_length, header.payload_length);

    Some(WyomingMessage {
        msg_type: header.msg_type,
        data,
        payload,
    })
}

/// Write one Wyoming message to a TCP stream.
pub async fn write_message(writer: &mut tokio::net::tcp::OwnedWriteHalf, msg: &WyomingMessage) -> Result<(), String> {
    // Serialize data to bytes
    let data_bytes = if msg.data.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&msg.data).unwrap_or_default()
    };

    let header = serde_json::json!({
        "type": msg.msg_type,
        "version": "1.0.0",
        "data_length": data_bytes.len(),
        "payload_length": msg.payload.len(),
    });

    let header_str = serde_json::to_string(&header)
        .map_err(|e| format!("serialize header: {}", e))?;

    // Write: header\n + data bytes + payload bytes
    writer
        .write_all(format!("{}\n", header_str).as_bytes())
        .await
        .map_err(|e| format!("write header: {}", e))?;

    if !data_bytes.is_empty() {
        writer
            .write_all(&data_bytes)
            .await
            .map_err(|e| format!("write data: {}", e))?;
    }

    if !msg.payload.is_empty() {
        writer
            .write_all(&msg.payload)
            .await
            .map_err(|e| format!("write payload: {}", e))?;
    }

    writer.flush().await.map_err(|e| format!("flush: {}", e))?;

    debug!("[wyoming] sent: type={} data={} payload={}", msg.msg_type, data_bytes.len(), msg.payload.len());
    Ok(())
}
