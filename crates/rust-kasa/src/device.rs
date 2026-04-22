//! High-level `Device` client — connect, handshake, send smart-protocol
//! queries, return parsed responses.

use base64::Engine as _;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::http_raw;
use crate::klap::{auth_hash_v2, h1_server_hash, h2_send_hash, KlapSession};
use crate::KasaError;

pub struct Device {
    host: String,
    session_cookie: String,
    session: KlapSession,
    terminal_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    #[serde(default)]
    pub device_on: Option<bool>,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub hw_ver: Option<String>,
    #[serde(default)]
    pub fw_ver: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    /// Everything else, unchanged.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn extract_tp_sessionid(set_cookie: &str) -> Option<String> {
    let prefix = "TP_SESSIONID=";
    let start = set_cookie.find(prefix)? + prefix.len();
    let tail = &set_cookie[start..];
    let end = tail
        .find(|c: char| c == ';' || c == ',' || c.is_whitespace())
        .unwrap_or(tail.len());
    Some(tail[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::extract_tp_sessionid;

    #[test]
    fn parses_tp_link_no_space() {
        assert_eq!(
            extract_tp_sessionid("TP_SESSIONID=ABCD1234;TIMEOUT=86400").as_deref(),
            Some("ABCD1234"),
        );
    }

    #[test]
    fn parses_standard_space_attr() {
        assert_eq!(
            extract_tp_sessionid("TP_SESSIONID=XYZ; Path=/").as_deref(),
            Some("XYZ"),
        );
    }
}

impl Device {
    /// Do handshake1 + handshake2 + derive session keys. Leaves the
    /// device ready for `query` calls.
    pub async fn connect(host: &str, username: &str, password: &str) -> Result<Self, KasaError> {
        let auth = auth_hash_v2(username, password);

        // handshake1
        let mut local_seed = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut local_seed);
        let resp = http_raw::post(host, "/app/handshake1", &local_seed, None).await?;
        if resp.status != 200 {
            return Err(KasaError::Handshake1Status {
                status: resp.status,
                host: host.into(),
            });
        }
        let sid = resp
            .headers_all("Set-Cookie")
            .find_map(extract_tp_sessionid)
            .ok_or(KasaError::NoSessionCookie)?;
        if resp.body.len() != 48 {
            return Err(KasaError::Handshake1Len {
                got: resp.body.len(),
            });
        }
        let mut remote_seed = [0u8; 16];
        remote_seed.copy_from_slice(&resp.body[..16]);
        let mut server_hash = [0u8; 32];
        server_hash.copy_from_slice(&resp.body[16..]);
        let expected = h1_server_hash(&local_seed, &remote_seed, &auth);
        if expected != server_hash {
            return Err(KasaError::AuthFailed);
        }

        // handshake2
        let h2_body = h2_send_hash(&remote_seed, &local_seed, &auth);
        let cookie = format!("TP_SESSIONID={sid}");
        let resp = http_raw::post(host, "/app/handshake2", &h2_body, Some(&cookie)).await?;
        if resp.status != 200 {
            return Err(KasaError::Handshake2Status {
                status: resp.status,
            });
        }

        // Derive session + terminal_uuid.
        let session = KlapSession::derive(&local_seed, &remote_seed, &auth);
        // `terminal_uuid` is an opaque 16-byte identifier base64'd — python-kasa
        // uses `md5(uuid4())` which is just a way to generate a random 16-byte
        // blob. The device never validates the MD5 construction, so any random
        // 16 bytes works and saves a dep.
        let mut uuid_bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut uuid_bytes);
        let terminal_uuid = base64::engine::general_purpose::STANDARD.encode(uuid_bytes);

        Ok(Self {
            host: host.into(),
            session_cookie: sid,
            session,
            terminal_uuid,
        })
    }

    /// Send a smart-protocol request, return the parsed response as JSON.
    pub async fn query(&mut self, method: &str, params: Option<Value>) -> Result<Value, KasaError> {
        let mut req = json!({
            "method": method,
            "request_time_milis": Utc::now().timestamp_millis(),
            "terminal_uuid": self.terminal_uuid,
        });
        if let Some(p) = params {
            req["params"] = p;
        }
        let plaintext = serde_json::to_vec(&req)?;
        let (body, seq) = self.session.encrypt(&plaintext);

        let path = format!("/app/request?seq={seq}");
        let cookie = format!("TP_SESSIONID={}", self.session_cookie);
        let resp = http_raw::post(&self.host, &path, &body, Some(&cookie)).await?;
        if resp.status != 200 {
            return Err(KasaError::RequestStatus {
                status: resp.status,
            });
        }
        let plain = self.session.decrypt(&resp.body)?;
        let v: Value = serde_json::from_slice(&plain)?;
        let code = v["error_code"].as_i64().unwrap_or(0);
        if code != 0 {
            return Err(KasaError::DeviceError {
                code,
                msg: v["msg"].as_str().unwrap_or("").into(),
            });
        }
        Ok(v["result"].clone())
    }

    pub async fn get_device_info(&mut self) -> Result<DeviceInfo, KasaError> {
        let v = self.query("get_device_info", None).await?;
        let info: DeviceInfo = serde_json::from_value(v)?;
        Ok(info)
    }

    pub async fn turn_on(&mut self) -> Result<(), KasaError> {
        self.query("set_device_info", Some(json!({ "device_on": true })))
            .await?;
        Ok(())
    }

    pub async fn turn_off(&mut self) -> Result<(), KasaError> {
        self.query("set_device_info", Some(json!({ "device_on": false })))
            .await?;
        Ok(())
    }

    /// Dimmer-only — device will return an error if the model doesn't
    /// support it (S515D does, S505 doesn't).
    pub async fn set_brightness(&mut self, pct: u8) -> Result<(), KasaError> {
        let v = pct.min(100);
        self.query("set_device_info", Some(json!({ "brightness": v })))
            .await?;
        Ok(())
    }
}
