//! Shelly Gen1 dialect — the pre-Plus legacy stack.
//!
//! Device announce: `shellies/<model>-<deviceid>/announce` with a JSON
//! payload: `{id, model, mac, ip, fw_ver, mode, …}`.
//!
//! Runtime topics (not parsed in v1 — would land alongside Tasmota's
//! Phase B4 STATE handling if we need them):
//!   - `shellies/<id>/relay/<n>`            — plain "on"/"off"
//!   - `shellies/<id>/relay/<n>/power`      — watts
//!   - `shellies/<id>/roller/0/pos`         — cover position
//!   - `shellies/<id>/light/0/status`       — light JSON
//!
//! Commands (Phase D):
//!   - `shellies/<id>/relay/<n>/command`    — "on" | "off" | "toggle"
//!
//! Shelly Gen2+ uses JSON-RPC over MQTT — see `shelly_gen2.rs` (Phase B2).
//!
//! Spec: https://shelly-api-docs.shelly.cloud/gen1/

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::drivers::mqtt::command::{EncodedCommand, MqttCommand};
use crate::smart_home::scan::ScanCandidate;

pub struct ShellyGen1;

impl Dialect for ShellyGen1 {
    fn id(&self) -> &'static str {
        "shelly_gen1"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &["shellies/+/announce"]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if !(topic.starts_with("shellies/") && topic.ends_with("/announce")) {
            return None;
        }
        let payload_s = std::str::from_utf8(payload).ok()?;
        parse_announce(topic, payload_s).map(DialectMessage::Discovery)
    }

    fn encode_command(
        &self,
        external_id: &str,
        cmd: &MqttCommand,
    ) -> Option<EncodedCommand> {
        // Shelly Gen1 has a flat topic space. v1 wires the single-relay
        // path (`relay/0`); multi-relay devices can use `Raw` until we
        // add a channel parameter to the command vocabulary.
        let id = external_id.strip_prefix("shelly:")?;
        match cmd {
            MqttCommand::SetOn(on) => Some(EncodedCommand::new(
                format!("shellies/{}/relay/0/command", id),
                if *on { b"on".to_vec() } else { b"off".to_vec() },
            )),
            MqttCommand::SetCoverPosition(pct) => Some(EncodedCommand::new(
                format!("shellies/{}/roller/0/command/pos", id),
                pct.to_string().into_bytes(),
            )),
            MqttCommand::Raw(v) => {
                let payload = match v {
                    Value::String(s) => s.clone().into_bytes(),
                    other => other.to_string().into_bytes(),
                };
                Some(EncodedCommand::new(
                    format!("shellies/{}/command", id),
                    payload,
                ))
            }
            _ => None,
        }
    }
}

/// Parse one `shellies/<id>/announce` frame.
pub fn parse_announce(topic: &str, payload: &str) -> Option<ScanCandidate> {
    let parts: Vec<&str> = topic.split('/').collect();
    let id = parts.get(1).copied().unwrap_or("").to_string();
    let j: Value = serde_json::from_str(payload).ok()?;
    let ip = j.get("ip").and_then(|v| v.as_str()).map(str::to_string);
    let mac = j.get("mac").and_then(|v| v.as_str()).map(str::to_string);
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("shelly:{}", id),
        name: id.clone(),
        kind: "plug".into(),
        vendor: Some("Shelly".into()),
        ip,
        mac,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "shelly_gen1",
            "topic": topic,
            "payload": j,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shelly_gen1_announce() {
        let payload = r#"{"id":"shellyplug-s-aabbcc","ip":"192.168.1.60","mac":"AA:BB:CC:DD:EE:FF","fw_ver":"20250101"}"#;
        let c = parse_announce("shellies/shellyplug-s-aabbcc/announce", payload)
            .expect("candidate");
        assert_eq!(c.vendor.as_deref(), Some("Shelly"));
        assert_eq!(c.ip.as_deref(), Some("192.168.1.60"));
        assert_eq!(c.kind, "plug");
    }

    #[test]
    fn dialect_returns_none_for_non_shelly_topic() {
        let d = ShellyGen1;
        assert!(d.parse("homeassistant/light/x/config", b"{}").is_none());
    }

    #[test]
    fn encode_set_on_maps_to_relay_zero_command() {
        let d = ShellyGen1;
        let e = d
            .encode_command("shelly:shellyplug-s-aabbcc", &MqttCommand::SetOn(false))
            .expect("some");
        assert_eq!(e.topic, "shellies/shellyplug-s-aabbcc/relay/0/command");
        assert_eq!(&e.payload, b"off");
    }
}
