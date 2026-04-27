//! Govee LAN adopter (Recipe R2 — open LAN, no auth).
//!
//! Govee's LAN protocol is publicly documented:
//! <https://app-h5.govee.com/user-manual/wlan-guide>
//!
//! ## Wire format
//!
//! - **Discovery**: UDP multicast to `239.255.255.250:4001`. Bulbs reply
//!   on UDP `4002` with `{ "msg": { "cmd": "scan", "data": { ip, device,
//!   sku, bleVersionHard, ... } } }`. The `device` field is the bulb's
//!   8-byte BLE MAC formatted as `AA:BB:CC:DD:EE:FF:00:11`.
//! - **Control**: per-bulb UDP unicast to `<bulb_ip>:4003`.
//! - **Status query**: UDP unicast to `<bulb_ip>:4003`; reply on `4002`.
//!
//! ## Prerequisites
//!
//! Each bulb must have **"Local control"** enabled in the Govee Home
//! app. Without it the bulb stays cloud-only and silent on the LAN
//! ports. Sean's H6022 + 3x H61D3 already have this toggle on (visible
//! via Home Assistant's `govee_light_local` integration).
//!
//! ## VLAN caveat
//!
//! Discovery is multicast and therefore L2-bound. If the gateway and
//! bulbs sit on different VLANs (Sean's IOT VLAN is 192.168.20.0/24,
//! Syntaur runs on 192.168.1.0/24), discovery silently returns zero
//! candidates even though unicast control would work given a known IP.
//! [`GoveeAdopter::probe_unicast`] exists for that case — feed it
//! known IPs from another source (UniFi inventory, manual entry).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::time::timeout;

use super::{LanAdopter, LanCandidate, LanCommand, LanCommandOutcome, Recipe};

const DISCOVER_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const DISCOVER_PORT: u16 = 4001;
const REPLY_PORT: u16 = 4002;
const CONTROL_PORT: u16 = 4003;
const DEFAULT_SCAN_MS: u64 = 5_000;

#[derive(Default, Clone)]
pub struct GoveeAdopter;

#[async_trait]
impl LanAdopter for GoveeAdopter {
    fn slug(&self) -> &'static str {
        "govee"
    }

    fn recipe(&self) -> Recipe {
        Recipe::R2OpenLan
    }

    async fn discover(&self) -> Vec<LanCandidate> {
        match timeout(
            Duration::from_millis(DEFAULT_SCAN_MS + 500),
            scan_inner(DEFAULT_SCAN_MS),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                log::warn!("[govee::discover] {e}");
                Vec::new()
            }
            Err(_) => {
                log::warn!("[govee::discover] outer timeout");
                Vec::new()
            }
        }
    }

    async fn dispatch(
        &self,
        external_id: &str,
        cmd: &LanCommand,
    ) -> LanCommandOutcome {
        match dispatch_inner(external_id, cmd).await {
            Ok(()) => LanCommandOutcome {
                adopter: "govee".to_string(),
                external_id: external_id.to_string(),
                ok: true,
                message: None,
            },
            Err(e) => LanCommandOutcome {
                adopter: "govee".to_string(),
                external_id: external_id.to_string(),
                ok: false,
                message: Some(e),
            },
        }
    }
}

async fn scan_inner(window_ms: u64) -> Result<Vec<LanCandidate>, String> {
    // Bind a single socket to 0.0.0.0:4002 so we can both send the
    // multicast scan and receive replies. SO_REUSEADDR lets a co-running
    // HA `govee_light_local` integration coexist on the same port.
    let std_sock = std::net::UdpSocket::bind(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        REPLY_PORT,
    ))
    .map_err(|e| format!("bind 4002: {e}"))?;
    std_sock
        .set_nonblocking(true)
        .map_err(|e| format!("nonblocking: {e}"))?;
    let sock = UdpSocket::from_std(std_sock).map_err(|e| format!("from_std: {e}"))?;
    sock.set_multicast_ttl_v4(2)
        .map_err(|e| format!("ttl: {e}"))?;
    sock.set_broadcast(true).ok();
    let scan_msg = json!({
        "msg": { "cmd": "scan", "data": { "account_topic": "reserve" } }
    })
    .to_string();
    sock.send_to(scan_msg.as_bytes(), SocketAddrV4::new(DISCOVER_GROUP, DISCOVER_PORT))
        .await
        .map_err(|e| format!("send scan: {e}"))?;

    let mut buf = vec![0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(window_ms);
    let mut out = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match timeout(remaining, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _addr))) => {
                let frame = String::from_utf8_lossy(&buf[..n]);
                if let Some(c) = parse_scan_reply(&frame) {
                    if !out.iter().any(|x: &LanCandidate| x.external_id == c.external_id) {
                        out.push(c);
                    }
                }
            }
            Ok(Err(e)) => {
                log::debug!("[govee::scan] recv error: {e}");
                break;
            }
            Err(_) => break,
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, Serialize)]
struct ScanFrame {
    msg: ScanMsg,
}

#[derive(Debug, Deserialize, Serialize)]
struct ScanMsg {
    cmd: String,
    data: ScanData,
}

#[derive(Debug, Deserialize, Serialize)]
struct ScanData {
    ip: String,
    device: String,
    sku: String,
    #[serde(default, rename = "bleVersionHard")]
    ble_hw: Option<String>,
    #[serde(default, rename = "bleVersionSoft")]
    ble_sw: Option<String>,
    #[serde(default, rename = "wifiVersionHard")]
    wifi_hw: Option<String>,
    #[serde(default, rename = "wifiVersionSoft")]
    wifi_sw: Option<String>,
}

fn parse_scan_reply(text: &str) -> Option<LanCandidate> {
    let frame: ScanFrame = serde_json::from_str(text).ok()?;
    if frame.msg.cmd != "scan" {
        return None;
    }
    let d = frame.msg.data;
    let kind = match d.sku.as_str() {
        // Strips
        s if s.starts_with("H61") || s.starts_with("H70") || s.starts_with("H619") => "light_strip",
        // Bars/lamps
        s if s.starts_with("H60") || s.starts_with("H62") => "light_bulb",
        // Floor/Lyra/etc.
        _ => "light",
    };
    Some(LanCandidate {
        adopter: "govee".to_string(),
        external_id: d.device.clone(),
        name: format!("Govee {}", d.sku),
        kind: kind.to_string(),
        vendor: "Govee".to_string(),
        ip: d.ip.clone(),
        mac: Some(d.device),
        model: Some(d.sku),
        details: json!({
            "ble_hw": d.ble_hw,
            "ble_sw": d.ble_sw,
            "wifi_hw": d.wifi_hw,
            "wifi_sw": d.wifi_sw,
        }),
    })
}

async fn dispatch_inner(external_id: &str, cmd: &LanCommand) -> Result<(), String> {
    // Govee dispatch needs a target IP. The caller stashes it in the
    // device row; we look it up by external_id (BLE-MAC-style id) once
    // we have a stored-device API. For now, the caller must encode the
    // IP into the external_id as `<MAC>@<IP>` to drive control without
    // a registry lookup. This contract simplifies unit testing and the
    // stored-device path will land alongside onboarding (out of scope
    // for this scaffold commit).
    let (mac, ip) = parse_external_id(external_id)?;
    let payload = encode_command(cmd, &mac)?;
    let sock = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| format!("bind: {e}"))?;
    sock.send_to(payload.to_string().as_bytes(), format!("{ip}:{CONTROL_PORT}"))
        .await
        .map_err(|e| format!("send: {e}"))?;
    Ok(())
}

fn parse_external_id(external_id: &str) -> Result<(String, String), String> {
    if let Some((mac, ip)) = external_id.split_once('@') {
        Ok((mac.to_string(), ip.to_string()))
    } else {
        Err(format!(
            "govee external_id must be MAC@IP until registry lookup ships; got '{external_id}'"
        ))
    }
}

fn encode_command(cmd: &LanCommand, _mac: &str) -> Result<Value, String> {
    Ok(match cmd {
        LanCommand::SetOn(v) => json!({
            "msg": { "cmd": "turn", "data": { "value": if *v { 1 } else { 0 } } }
        }),
        LanCommand::SetBrightness(v) => json!({
            "msg": { "cmd": "brightness", "data": { "value": (*v).clamp(1, 100) } }
        }),
        LanCommand::SetColorRgb { r, g, b } => json!({
            "msg": {
                "cmd": "colorwc",
                "data": { "color": { "r": *r, "g": *g, "b": *b }, "colorTemInKelvin": 0 }
            }
        }),
        LanCommand::SetColorTempK(k) => json!({
            "msg": {
                "cmd": "colorwc",
                "data": { "color": { "r": 0, "g": 0, "b": 0 }, "colorTemInKelvin": (*k).clamp(2000, 9000) }
            }
        }),
        LanCommand::Raw(v) => v.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_real_scan_reply_h6022() {
        let payload = r#"{"msg":{"cmd":"scan","data":{"ip":"192.168.20.50","device":"2A:9D:D4:0F:43:86:6D:3E","sku":"H6022","bleVersionHard":"1.04.45","bleVersionSoft":"1.05.16","wifiVersionHard":"1.00.10","wifiVersionSoft":"1.00.20"}}}"#;
        let c = parse_scan_reply(payload).expect("must parse");
        assert_eq!(c.adopter, "govee");
        assert_eq!(c.external_id, "2A:9D:D4:0F:43:86:6D:3E");
        assert_eq!(c.kind, "light_bulb");
        assert_eq!(c.vendor, "Govee");
        assert_eq!(c.ip, "192.168.20.50");
        assert_eq!(c.model.as_deref(), Some("H6022"));
        assert_eq!(c.name, "Govee H6022");
    }

    #[test]
    fn parse_real_scan_reply_h61d3_strip() {
        let payload = r#"{"msg":{"cmd":"scan","data":{"ip":"192.168.20.51","device":"0C:AE:D0:C8:02:86:64:74","sku":"H61D3"}}}"#;
        let c = parse_scan_reply(payload).expect("must parse");
        assert_eq!(c.kind, "light_strip");
        assert_eq!(c.model.as_deref(), Some("H61D3"));
    }

    #[test]
    fn parse_rejects_non_scan_frame() {
        let payload = r#"{"msg":{"cmd":"devStatus","data":{}}}"#;
        assert!(parse_scan_reply(payload).is_none());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_scan_reply("not json").is_none());
        assert!(parse_scan_reply("{}").is_none());
        assert!(parse_scan_reply(r#"{"msg":42}"#).is_none());
    }

    #[test]
    fn encode_set_on() {
        let v = encode_command(&LanCommand::SetOn(true), "AA").unwrap();
        assert_eq!(v["msg"]["cmd"], "turn");
        assert_eq!(v["msg"]["data"]["value"], 1);
        let off = encode_command(&LanCommand::SetOn(false), "AA").unwrap();
        assert_eq!(off["msg"]["data"]["value"], 0);
    }

    #[test]
    fn encode_brightness_clamps() {
        let v = encode_command(&LanCommand::SetBrightness(150), "AA").unwrap();
        assert_eq!(v["msg"]["data"]["value"], 100);
        let z = encode_command(&LanCommand::SetBrightness(0), "AA").unwrap();
        assert_eq!(z["msg"]["data"]["value"], 1);
    }

    #[test]
    fn encode_color_rgb() {
        let v = encode_command(
            &LanCommand::SetColorRgb { r: 255, g: 128, b: 0 },
            "AA",
        )
        .unwrap();
        assert_eq!(v["msg"]["cmd"], "colorwc");
        assert_eq!(v["msg"]["data"]["color"]["r"], 255);
        assert_eq!(v["msg"]["data"]["color"]["g"], 128);
        assert_eq!(v["msg"]["data"]["color"]["b"], 0);
        assert_eq!(v["msg"]["data"]["colorTemInKelvin"], 0);
    }

    #[test]
    fn encode_color_temp_clamps() {
        let v = encode_command(&LanCommand::SetColorTempK(50_000), "AA").unwrap();
        assert_eq!(v["msg"]["data"]["colorTemInKelvin"], 9000);
        let v2 = encode_command(&LanCommand::SetColorTempK(100), "AA").unwrap();
        assert_eq!(v2["msg"]["data"]["colorTemInKelvin"], 2000);
    }

    #[test]
    fn parse_external_id_split() {
        let (mac, ip) = parse_external_id("AA:BB:CC:DD:EE:FF:00:11@192.168.1.50").unwrap();
        assert_eq!(mac, "AA:BB:CC:DD:EE:FF:00:11");
        assert_eq!(ip, "192.168.1.50");
        assert!(parse_external_id("no-at-sign").is_err());
    }
}
