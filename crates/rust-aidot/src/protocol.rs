//! TCP+AES device client protocol. Matches the wire format confirmed via
//! tcpdump on 2026-04-21 talking to an aidot bulb at 192.168.1.14:10000.

use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::crypto::{decrypt_body, encrypt_body};
use crate::{AidotError, InventoryDevice};

/// Big-endian `0x1EED` prefix on every frame.
pub const MAGIC: u16 = 0x1EED;
/// TCP listener port exposed by every aidot bulb.
pub const PORT: u16 = 10000;
/// How long to wait for any read before giving up.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The 2-byte `msgtype` field on the aidot wire frame. Confirmed by
/// reading python-aidot's code: BOTH login AND send_action use `msgtype=1`.
/// The original crate enum had Login=1/Action=2; that's wrong — observed
/// as the root cause of silent-drops on the first getDevAttrReq sent
/// after login.
pub const MSGTYPE_CLIENT_REQUEST: u16 = 1;

/// Wrapper kept for call-site documentation only; both variants encode as `1`.
#[derive(Debug, Copy, Clone)]
pub enum FrameType {
    Login,
    Action,
}

impl FrameType {
    pub fn wire_value(self) -> u16 {
        MSGTYPE_CLIENT_REQUEST
    }
}

pub struct DeviceClient {
    device: InventoryDevice,
    aes_key: [u8; 16],
    user_id: String,
    stream: TcpStream,
    /// `ascNumber` — rolling counter echoed back by the device on every
    /// response. We start with whatever the login response reports and
    /// increment before each subsequent request.
    asc_number: i64,
    seq_counter: u64,
}

impl DeviceClient {
    /// Connect + log in. Blocks until the device returns `ack.code = 200`
    /// for the loginReq.
    pub async fn connect(
        device: InventoryDevice,
        user_id: String,
        ip: &str,
    ) -> Result<Self, AidotError> {
        let addr = format!("{ip}:{PORT}");
        let stream = timeout(IO_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| AidotError::Timeout { seconds: 5, what: "tcp connect" })??;
        stream.set_nodelay(true)?;
        let aes_key = device.aes_key_bytes()?;

        let mut client = Self {
            device,
            aes_key,
            user_id,
            stream,
            asc_number: 1,
            seq_counter: 0,
        };
        client.login().await?;
        Ok(client)
    }

    pub async fn turn_on(&mut self) -> Result<(), AidotError> {
        self.set_dev_attr(json!({ "OnOff": 1 })).await
    }

    pub async fn turn_off(&mut self) -> Result<(), AidotError> {
        self.set_dev_attr(json!({ "OnOff": 0 })).await
    }

    /// Dimming percentage 0..=100 (not 0..=255 — the device's native scale).
    pub async fn set_dimming(&mut self, pct: u8) -> Result<(), AidotError> {
        let v = pct.min(100);
        self.set_dev_attr(json!({ "Dimming": v })).await
    }

    /// Correlated color temperature, Kelvin — valid range depends on the
    /// bulb; the captured device reports bounds via its service modules.
    pub async fn set_cct(&mut self, cct: u32) -> Result<(), AidotError> {
        self.set_dev_attr(json!({ "CCT": cct })).await
    }

    /// RGBW packed as i32 (`r << 24 | g << 16 | b << 8 | w`) — the device
    /// expects a signed 32-bit int (matches `ctypes.c_int32(...).value`
    /// in python-aidot).
    pub async fn set_rgbw(&mut self, r: u8, g: u8, b: u8, w: u8) -> Result<(), AidotError> {
        let packed: u32 = ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (w as u32);
        let signed = packed as i32;
        self.set_dev_attr(json!({ "RGBW": signed })).await
    }

    // ── internals ──────────────────────────────────────────────────────────

    async fn login(&mut self) -> Result<(), AidotError> {
        let seq = seq_str();
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let msg = json!({
            "service": "device",
            "method": "loginReq",
            "seq": seq,
            "srcAddr": self.user_id,
            "deviceId": self.device.id,
            "payload": {
                "userId": self.user_id,
                "password": self.device.password,
                "timestamp": timestamp,
                "ascNumber": 1,
            },
        });
        self.send_frame(FrameType::Login, &msg).await?;
        let resp = self.recv_frame().await?;
        let code = resp["ack"]["code"].as_i64().unwrap_or(-1);
        if code != 200 {
            return Err(AidotError::NonSuccess {
                code,
                payload: resp.to_string(),
            });
        }
        if let Some(asc) = resp["payload"]["ascNumber"].as_i64() {
            self.asc_number = asc + 1;
        }
        // Match python-aidot: right after loginResp it fires a
        // `getDevAttrReq` with empty attr. The device then replies with a
        // `getDevAttrResp` containing the current attribute bag. Without
        // this handshake, the device silently drops subsequent
        // setDevAttrReq frames — TCP-ACKs them at the transport layer but
        // never produces an application-layer response.
        self.send_action("getDevAttrReq", json!({})).await?;
        // Consume the getDevAttrResp before returning so the recv queue is
        // clean for subsequent set_dev_attr calls.
        for _ in 0..4 {
            let r = self.recv_frame().await?;
            if r["method"].as_str() == Some("getDevAttrResp") {
                if let Some(asc) = r["payload"]["ascNumber"].as_i64() {
                    self.asc_number = asc + 1;
                }
                break;
            }
        }
        Ok(())
    }

    async fn send_action(&mut self, method: &str, attr: Value) -> Result<(), AidotError> {
        self.seq_counter += 1;
        let seq = format!("ha93{:05}", self.seq_counter);
        let tst = Utc::now().timestamp_millis();
        let msg = json!({
            "method": method,
            "service": "device",
            "clientId": format!("ha-{}", self.user_id),
            "srcAddr": format!("0.{}", self.user_id),
            "seq": seq,
            "payload": {
                "devId": self.device.id,
                "parentId": self.device.id,
                "userId": self.user_id,
                "password": self.device.password,
                "attr": attr,
                "channel": "tcp",
                "ascNumber": self.asc_number,
            },
            "tst": tst,
            "deviceId": self.device.id,
        });
        self.send_frame(FrameType::Action, &msg).await
    }

    async fn set_dev_attr(&mut self, attr: Value) -> Result<(), AidotError> {
        self.send_action("setDevAttrReq", attr).await?;

        // Consume frames until we see a setDevAttrResp or getDevAttrResp ack.
        // The device can reply with a status blob first (getDevAttrResp) and
        // THEN the ack we're waiting for — the login flow already burned
        // those from the socket, but during steady-state it's still
        // possible, so tolerate it.
        for _ in 0..4 {
            let resp = self.recv_frame().await?;
            let method = resp["method"].as_str().unwrap_or("");
            if method == "setDevAttrResp" {
                let code = resp["ack"]["code"].as_i64().unwrap_or(-1);
                if code == 200 {
                    if let Some(asc) = resp["payload"]["ascNumber"].as_i64() {
                        self.asc_number = asc + 1;
                    }
                    return Ok(());
                } else {
                    return Err(AidotError::NonSuccess { code, payload: resp.to_string() });
                }
            }
            // incidental frames (state push, getDevAttrResp, pingresp): skip
        }
        Err(AidotError::Timeout { seconds: 5, what: "setDevAttrResp" })
    }

    async fn send_frame(&mut self, msgtype: FrameType, body: &Value) -> Result<(), AidotError> {
        let plaintext = serde_json::to_vec(body)?;
        let ciphertext = encrypt_body(&plaintext, &self.aes_key);
        let mut frame = Vec::with_capacity(8 + ciphertext.len());
        frame.extend_from_slice(&MAGIC.to_be_bytes());
        frame.extend_from_slice(&msgtype.wire_value().to_be_bytes());
        frame.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        frame.extend_from_slice(&ciphertext);
        timeout(IO_TIMEOUT, self.stream.write_all(&frame))
            .await
            .map_err(|_| AidotError::Timeout { seconds: 5, what: "tcp write" })??;
        Ok(())
    }

    async fn recv_frame(&mut self) -> Result<Value, AidotError> {
        let mut header = [0u8; 8];
        timeout(IO_TIMEOUT, self.stream.read_exact(&mut header))
            .await
            .map_err(|_| AidotError::Timeout { seconds: 5, what: "frame header" })??;
        let magic = u16::from_be_bytes([header[0], header[1]]);
        if magic != MAGIC {
            return Err(AidotError::BadMagic { got: magic });
        }
        // msgtype (header[2..4]) ignored on the read path — python-aidot
        // doesn't use it; response category is conveyed by `method` in the
        // JSON body.
        let bodysize = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; bodysize];
        timeout(IO_TIMEOUT, self.stream.read_exact(&mut body))
            .await
            .map_err(|_| AidotError::Timeout { seconds: 5, what: "frame body" })??;
        let plaintext = decrypt_body(&body, &self.aes_key)?;
        let v: Value = serde_json::from_slice(&plaintext)?;
        Ok(v)
    }
}

/// Last 9 digits of the current millisecond timestamp — matches
/// python-aidot's `str(int(time.time() * 1000) + n)[-9:]`.
fn seq_str() -> String {
    let ms = Utc::now().timestamp_millis();
    let s = ms.to_string();
    if s.len() > 9 {
        s[s.len() - 9..].to_string()
    } else {
        s
    }
}
