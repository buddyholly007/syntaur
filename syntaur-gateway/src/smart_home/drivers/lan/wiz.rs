//! WiZ Connected (Signify / Philips) LAN adopter (Recipe R2 — open LAN).
//!
//! WiZ bulbs speak a JSON-over-UDP protocol on port `38899`. Discovery
//! uses an L2 broadcast; control is unicast.
//!
//! Sean's network has 7 WiZ bulbs registered (UniFi `D8:A0:11:*`
//! historical client list 2026-04-26) including the living-room
//! chandelier `light.living_room_chandelier_light_bulb_1` (model
//! `SHTW`). All are dormant most of the time and wake on Wi-Fi when
//! the wall switch flips.
//!
//! ## Wire format
//!
//! - **Discovery**: UDP broadcast to `255.255.255.255:38899` with
//!   `{"method":"getPilot","params":{}}`. Bulbs reply unicast with their
//!   `mac`, `state`, current pilot params, and `moduleName`.
//! - **Control**: UDP unicast to `<bulb_ip>:38899` with `setPilot`:
//!   - power: `{"method":"setPilot","params":{"state":true}}`
//!   - brightness 10..=100: `{"method":"setPilot","params":{"dimming":80}}`
//!   - color temp K (2200..=6500): `{"method":"setPilot","params":{"temp":3500}}`
//!   - rgb: `{"method":"setPilot","params":{"r":255,"g":100,"b":50}}`
//! - **Status query**: `{"method":"getPilot","params":{}}`.
//!
//! No auth, no shared secret. Packets are TCP-style atomic — one JSON
//! object per UDP datagram.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::time::timeout;

use super::{LanAdopter, LanCandidate, LanCommand, LanCommandOutcome, Recipe};

const WIZ_PORT: u16 = 38899;
const DEFAULT_SCAN_MS: u64 = 5_000;

#[derive(Default, Clone)]
pub struct WizAdopter;

#[async_trait]
impl LanAdopter for WizAdopter {
    fn slug(&self) -> &'static str {
        "wiz"
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
                log::warn!("[wiz::discover] {e}");
                Vec::new()
            }
            Err(_) => {
                log::warn!("[wiz::discover] outer timeout");
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
                adopter: "wiz".to_string(),
                external_id: external_id.to_string(),
                ok: true,
                message: None,
            },
            Err(e) => LanCommandOutcome {
                adopter: "wiz".to_string(),
                external_id: external_id.to_string(),
                ok: false,
                message: Some(e),
            },
        }
    }
}

async fn scan_inner(window_ms: u64) -> Result<Vec<LanCandidate>, String> {
    let std_sock = std::net::UdpSocket::bind(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        0,
    ))
    .map_err(|e| format!("bind: {e}"))?;
    std_sock.set_broadcast(true).map_err(|e| format!("broadcast: {e}"))?;
    std_sock.set_nonblocking(true).map_err(|e| format!("nonblocking: {e}"))?;
    let sock = UdpSocket::from_std(std_sock).map_err(|e| format!("from_std: {e}"))?;

    let probe = json!({"method": "getPilot", "params": {}}).to_string();
    let bcast: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::BROADCAST, WIZ_PORT);
    sock.send_to(probe.as_bytes(), bcast)
        .await
        .map_err(|e| format!("send: {e}"))?;

    let mut buf = vec![0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(window_ms);
    let mut out: Vec<LanCandidate> = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match timeout(remaining, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, addr))) => {
                let frame = String::from_utf8_lossy(&buf[..n]);
                let ip = addr.ip().to_string();
                if let Some(c) = parse_pilot_reply(&frame, &ip) {
                    if !out.iter().any(|x| x.external_id == c.external_id) {
                        out.push(c);
                    }
                }
            }
            Ok(Err(e)) => {
                log::debug!("[wiz::scan] recv error: {e}");
                break;
            }
            Err(_) => break,
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, Serialize)]
struct PilotFrame {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    result: Option<PilotResult>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PilotResult {
    #[serde(default)]
    mac: Option<String>,
    #[serde(default, rename = "moduleName")]
    module_name: Option<String>,
    #[serde(default)]
    state: Option<bool>,
    #[serde(default)]
    dimming: Option<u32>,
    #[serde(default)]
    temp: Option<u32>,
    #[serde(default)]
    r: Option<u32>,
    #[serde(default)]
    g: Option<u32>,
    #[serde(default)]
    b: Option<u32>,
    #[serde(default, rename = "fwVersion")]
    fw_version: Option<String>,
    #[serde(default, rename = "homeId")]
    home_id: Option<u32>,
}

fn parse_pilot_reply(text: &str, ip: &str) -> Option<LanCandidate> {
    let frame: PilotFrame = serde_json::from_str(text).ok()?;
    if frame.method.as_deref() != Some("getPilot") {
        return None;
    }
    let r = frame.result?;
    let mac = r.mac.clone()?;
    let model = r.module_name.clone();
    let kind = match model.as_deref() {
        Some(m) if m.contains("STRIPE") || m.contains("STRIP") => "light_strip",
        _ => "light_bulb",
    };
    Some(LanCandidate {
        adopter: "wiz".to_string(),
        external_id: mac.clone(),
        name: model
            .clone()
            .map(|m| format!("WiZ {m}"))
            .unwrap_or_else(|| format!("WiZ {mac}")),
        kind: kind.to_string(),
        vendor: "WiZ".to_string(),
        ip: ip.to_string(),
        mac: Some(mac),
        model,
        details: json!({
            "state": r.state,
            "dimming": r.dimming,
            "temp": r.temp,
            "r": r.r,
            "g": r.g,
            "b": r.b,
            "fw_version": r.fw_version,
            "home_id": r.home_id,
        }),
    })
}

async fn dispatch_inner(external_id: &str, cmd: &LanCommand) -> Result<(), String> {
    // Like Govee, dispatch needs the bulb's IP. Until the registry
    // lookup ships, encode as `<MAC>@<IP>`.
    let (_mac, ip) = parse_external_id(external_id)?;
    let payload = encode_command(cmd)?;
    let sock = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| format!("bind: {e}"))?;
    sock.send_to(payload.to_string().as_bytes(), format!("{ip}:{WIZ_PORT}"))
        .await
        .map_err(|e| format!("send: {e}"))?;
    Ok(())
}

fn parse_external_id(external_id: &str) -> Result<(String, String), String> {
    if let Some((mac, ip)) = external_id.split_once('@') {
        Ok((mac.to_string(), ip.to_string()))
    } else {
        Err(format!(
            "wiz external_id must be MAC@IP until registry lookup ships; got '{external_id}'"
        ))
    }
}

fn encode_command(cmd: &LanCommand) -> Result<Value, String> {
    Ok(match cmd {
        LanCommand::SetOn(v) => json!({"method": "setPilot", "params": {"state": v}}),
        LanCommand::SetBrightness(v) => json!({
            "method": "setPilot",
            "params": { "dimming": (*v).clamp(10, 100) }
        }),
        LanCommand::SetColorTempK(k) => json!({
            "method": "setPilot",
            "params": { "temp": (*k).clamp(2200, 6500) }
        }),
        LanCommand::SetColorRgb { r, g, b } => json!({
            "method": "setPilot",
            "params": { "r": *r, "g": *g, "b": *b }
        }),
        LanCommand::Raw(v) => v.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_real_pilot_reply_shtw() {
        let payload = r#"{"method":"getPilot","env":"pro","result":{"mac":"d8a011b3581f","rssi":-65,"src":"","state":true,"sceneId":0,"temp":3000,"dimming":80,"moduleName":"ESP_0711_STR","fwVersion":"1.31.13","homeId":12345}}"#;
        let c = parse_pilot_reply(payload, "192.168.1.51").expect("parse");
        assert_eq!(c.adopter, "wiz");
        assert_eq!(c.external_id, "d8a011b3581f");
        assert_eq!(c.vendor, "WiZ");
        assert_eq!(c.ip, "192.168.1.51");
        assert_eq!(c.model.as_deref(), Some("ESP_0711_STR"));
    }

    #[test]
    fn parse_pilot_reply_strip_kind_inferred() {
        let payload = r#"{"method":"getPilot","result":{"mac":"d8a011b34c93","moduleName":"ESP01_STRIPE_03","state":false}}"#;
        let c = parse_pilot_reply(payload, "192.168.1.52").expect("parse");
        assert_eq!(c.kind, "light_strip");
    }

    #[test]
    fn parse_pilot_rejects_non_getpilot() {
        let payload = r#"{"method":"setPilot","result":{"success":true}}"#;
        assert!(parse_pilot_reply(payload, "1.2.3.4").is_none());
    }

    #[test]
    fn parse_pilot_rejects_missing_mac() {
        let payload = r#"{"method":"getPilot","result":{"state":true}}"#;
        assert!(parse_pilot_reply(payload, "1.2.3.4").is_none());
    }

    #[test]
    fn encode_set_on_state_field() {
        let v = encode_command(&LanCommand::SetOn(true)).unwrap();
        assert_eq!(v["method"], "setPilot");
        assert_eq!(v["params"]["state"], true);
    }

    #[test]
    fn encode_brightness_clamps_to_min_10() {
        let v = encode_command(&LanCommand::SetBrightness(2)).unwrap();
        assert_eq!(v["params"]["dimming"], 10);
        let v2 = encode_command(&LanCommand::SetBrightness(150)).unwrap();
        assert_eq!(v2["params"]["dimming"], 100);
    }

    #[test]
    fn encode_temp_clamps_2200_6500() {
        let v = encode_command(&LanCommand::SetColorTempK(1500)).unwrap();
        assert_eq!(v["params"]["temp"], 2200);
        let v2 = encode_command(&LanCommand::SetColorTempK(9000)).unwrap();
        assert_eq!(v2["params"]["temp"], 6500);
    }

    #[test]
    fn encode_rgb_no_clamp_needed() {
        let v = encode_command(&LanCommand::SetColorRgb { r: 255, g: 0, b: 100 }).unwrap();
        assert_eq!(v["params"]["r"], 255);
        assert_eq!(v["params"]["g"], 0);
        assert_eq!(v["params"]["b"], 100);
    }

    #[test]
    fn parse_external_id_split() {
        let (mac, ip) = parse_external_id("d8a011b3581f@192.168.1.51").unwrap();
        assert_eq!(mac, "d8a011b3581f");
        assert_eq!(ip, "192.168.1.51");
        assert!(parse_external_id("no-at-sign").is_err());
    }
}
