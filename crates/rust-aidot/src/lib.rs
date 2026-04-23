//! Pure-Rust LAN driver for aidot (formerly Linkind) smart bulbs.
//!
//! Reverse-engineered from the vendor's Python library `python-aidot`
//! (Apache-2.0, <https://github.com/AiDot-Development-Team/python-aidot>)
//! by reading its wire format + observing live LAN traffic on
//! 2026-04-21. After a one-time harvest of per-device credentials from
//! aidot's cloud (`prod-us-api.arnoo.com`), this crate talks to the
//! bulbs directly over LAN — no cloud calls at runtime.
//!
//! ## Wire protocol (what we confirmed on the wire)
//!
//! - TCP to `device_ip:10000`, keepalive via periodic `pingreq`
//! - Every frame: `magic(2 B BE) || msgtype(2 B BE) || bodysize(4 B BE) || body`
//! - `magic = 0x1EED`, `msgtype = 1` for loginReq; other request types use
//!   other values but the responses all come back with the same framing
//! - `body` is AES-128-ECB encrypted JSON with PKCS7 padding; the key is
//!   the per-device 16-byte `aesKey` (ASCII padded if shorter) harvested
//!   from the cloud at provisioning time
//! - Login request body:
//!   ```json
//!   { "service": "device", "method": "loginReq",
//!     "seq": "<last 9 digits of ms_timestamp>",
//!     "srcAddr": "<user_id>", "deviceId": "<device_id>",
//!     "payload": { "userId": "<user_id>", "password": "<device_password>",
//!                  "timestamp": "<YYYY-MM-DD HH:MM:SS.ffffff>",
//!                  "ascNumber": 1 } }
//!   ```
//! - Login response has `ack.code = 200` on success + `payload.ascNumber`
//!   which the client increments and echoes back on each subsequent
//!   `setDevAttrReq` / `getDevAttrReq`
//! - Commands reuse the frame shape with `method = "setDevAttrReq"` and
//!   `payload.attr` = e.g. `{ "OnOff": 1 }` / `{ "Dimming": 100 }` /
//!   `{ "CCT": 2702 }` / `{ "RGBW": <i32> }`.
//!
//! ## What the cloud provisioning gives you (harvest into `Inventory`)
//!
//! - `id` / `directId` — 32-char hex device identifier
//! - `password` — 12-char login password bound to the device
//! - `aesKey[0]` — 16-byte ASCII session key
//! - `mac` / `name` / `modelId` — human-readable metadata

use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod crypto;
mod protocol;

pub use crypto::{decrypt_body, encrypt_body, pad_key_to_16};
pub use protocol::{DeviceClient, FrameType, MAGIC, PORT};
// One-time cloud harvest lives in the separate `rust-aidot-harvest`
// crate (workspace-excluded) to keep the `rsa` crate off the main
// lockfile — see its Cargo.toml for the rationale.

/// On-disk inventory (one-time cloud harvest, then never touched again at runtime).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub user_id: String,
    pub country_code: String,
    pub devices: Vec<InventoryDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryDevice {
    #[serde(rename = "id")]
    pub id: String,
    pub name: String,
    pub mac: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
    pub password: String,
    #[serde(rename = "aesKey")]
    pub aes_key: Vec<String>,
    #[serde(rename = "firmwareVersion")]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub online: bool,
    /// Raw properties bag — useful for reading `ipAddress`, `matterUniqueId`, etc.
    #[serde(default)]
    pub properties: serde_json::Value,
}

impl InventoryDevice {
    /// The 16-byte AES key, right-padded with zeros if the ASCII source was
    /// shorter (matches python-aidot's `bytearray(16)` behavior).
    pub fn aes_key_bytes(&self) -> Result<[u8; 16], AidotError> {
        let s = self.aes_key.first().ok_or(AidotError::MissingAesKey)?;
        Ok(pad_key_to_16(s.as_bytes()))
    }

    /// Best-effort guess of the device's current LAN IP from the cloud-cached
    /// properties. The device announces its `ipAddress` via the cloud blob
    /// at the last connection; if unset, the caller should use LAN broadcast
    /// discovery instead.
    pub fn last_known_ip(&self) -> Option<String> {
        self.properties
            .get("ipAddress")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }
}

#[derive(Debug, Error)]
pub enum AidotError {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("AES decrypt failed (wrong key or corrupt frame)")]
    Decrypt,
    #[error("bad magic: expected {:#06x}, got {got:#06x}", MAGIC)]
    BadMagic { got: u16 },
    #[error("device returned ack.code={code}, payload={payload}")]
    NonSuccess { code: i64, payload: String },
    #[error("device inventory missing aesKey[0]")]
    MissingAesKey,
    #[error("timed out after {seconds}s waiting for {what}")]
    Timeout { seconds: u64, what: &'static str },
}
