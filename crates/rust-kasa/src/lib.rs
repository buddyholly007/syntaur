//! Pure-Rust LAN driver for TP-Link Tapo (and newer Kasa) smart devices
//! that speak the **KLAP v2** protocol over HTTP.
//!
//! Reverse-engineered from the `python-kasa` project (MIT,
//! <https://github.com/python-kasa/python-kasa>) by reading its
//! `kasa/transports/klaptransport.py` + `kasa/protocols/smartprotocol.py`
//! and exercising the protocol against Sean's Matter-era S505 / S515D
//! switches on 2026-04-21. After a one-time harvest of (or just static
//! config of) the user's TP-Link cloud email + password, this crate
//! talks to the devices directly over LAN — no cloud calls at runtime.
//!
//! # Wire summary — what we confirmed against an S505
//!
//! 1. `POST http://<device>:80/app/handshake1` with a 16-byte random
//!    `local_seed`.
//!    - Response body: `remote_seed(16 B) || server_hash(32 B)` where
//!      `server_hash = sha256(local_seed || remote_seed || auth_hash)`.
//!    - Response header: `Set-Cookie: TP_SESSIONID=<hex>; TIMEOUT=<n>`.
//!    - `auth_hash = sha256(sha1(username) || sha1(password))` (KLAP v2).
//! 2. `POST http://<device>:80/app/handshake2` with body
//!    `sha256(remote_seed || local_seed || auth_hash)`, sending the same
//!    `TP_SESSIONID` cookie. 200 OK confirms the session.
//! 3. Session keys (all SHA-256 truncated):
//!      - `key = sha256("lsk" || local_seed || remote_seed || auth_hash)[..16]` (AES-128)
//!      - `iv_full = sha256("iv" || local_seed || remote_seed || auth_hash)`
//!      - `iv_base = iv_full[..12]`
//!      - `seq = i32::from_be_bytes(iv_full[28..32])` — signed, starts at some large number
//!      - `sig = sha256("ldk" || local_seed || remote_seed || auth_hash)[..28]`
//! 4. Per-request (AES-128-CBC):
//!      - increment `seq`
//!      - `iv = iv_base(12) || seq.to_be_bytes()(4)`
//!      - `ciphertext = AES-CBC-encrypt(key, iv, PKCS7(json_body))`
//!      - `signature = sha256(sig || seq.to_be_bytes() || ciphertext)`
//!      - POST body = `signature(32) || ciphertext`
//!      - POST URL: `http://<device>:80/app/request?seq=<seq>`
//!      - Cookie: `TP_SESSIONID=<hex>`
//! 5. Decrypt response: same key+iv at current `seq`, strip first 32 bytes
//!    (signature), AES-CBC-decrypt, PKCS7-unpad.
//!
//! # Smart request envelope
//!
//! Inside the encrypted body, JSON:
//! ```json
//! { "method": "set_device_info",
//!   "params": { "device_on": true },
//!   "request_time_milis": 1729543210000,
//!   "terminal_uuid": "<md5(random uuid), base64>" }
//! ```

mod klap;
mod device;
mod harvest;
mod http_raw;
mod inventory;

pub use device::{Device, DeviceInfo};
pub use harvest::harvest_from_ips;
pub use inventory::{Inventory, InventoryDevice};
pub use klap::{auth_hash_v2, KlapSession};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum KasaError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("handshake1 HTTP {status} from {host}")]
    Handshake1Status { status: u16, host: String },
    #[error("handshake1 body was {got}B, expected 48 (remote_seed 16 + server_hash 32)")]
    Handshake1Len { got: usize },
    #[error("handshake1 server_hash did not match — credentials wrong or device bound to different account")]
    AuthFailed,
    #[error("no TP_SESSIONID in handshake1 response")]
    NoSessionCookie,
    #[error("handshake2 HTTP {status}")]
    Handshake2Status { status: u16 },
    #[error("request HTTP {status}")]
    RequestStatus { status: u16 },
    #[error("request body too short: {got}B (needs ≥ 32 B signature + AES-block ciphertext)")]
    ResponseTooShort { got: usize },
    #[error("PKCS7 unpad failed — likely session desync (wrong seq number)")]
    BadPadding,
    #[error("device returned error_code={code}: {msg}")]
    DeviceError { code: i64, msg: String },
    #[error("inventory: {0}")]
    Inventory(String),
}
