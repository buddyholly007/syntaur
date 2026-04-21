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
}
