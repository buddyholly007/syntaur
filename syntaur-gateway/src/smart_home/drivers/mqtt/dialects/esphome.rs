//! ESPHome dialect — optional MQTT discovery path.
//!
//! ESPHome's primary wire protocol is its native noise-encrypted API.
//! When MQTT mode is enabled in firmware, ESPHome publishes a
//! per-device discovery envelope at `esphome/discover/<hostname>` plus
//! per-entity state on `<topic_prefix>/{binary_sensor,sensor,switch,…}`.
//! This dialect parses the discovery envelope; per-entity state lands
//! with Phase C when the long-running subscriber starts consuming
//! individual entity topics.
//!
//! Spec: https://esphome.io/components/mqtt/

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct EspHome;

impl Dialect for EspHome {
    fn id(&self) -> &'static str {
        "esphome"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &["esphome/discover/+"]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if !topic.starts_with("esphome/discover/") {
            return None;
        }
        let payload_s = std::str::from_utf8(payload).ok()?;
        parse_discover(topic, payload_s).map(DialectMessage::Discovery)
    }
}

/// Parse an `esphome/discover/<hostname>` frame. Accepts empty-object
/// payloads (some firmwares publish `{}` as a presence beacon).
pub fn parse_discover(topic: &str, payload: &str) -> Option<ScanCandidate> {
    let parts: Vec<&str> = topic.split('/').collect();
    let host = parts.get(2).copied().unwrap_or("").to_string();
    let j: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("esphome:{}", host),
        name: host.clone(),
        kind: "unknown".into(),
        vendor: Some("ESPHome".into()),
        ip: None,
        mac: None,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "esphome",
            "topic": topic,
            "payload": j,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_esphome_discover_builds_candidate() {
        let c = parse_discover("esphome/discover/livingroom_sensor", "{}")
            .expect("candidate");
        assert_eq!(c.vendor.as_deref(), Some("ESPHome"));
        assert_eq!(c.external_id, "esphome:livingroom_sensor");
        assert_eq!(c.name, "livingroom_sensor");
    }

    #[test]
    fn dialect_returns_none_for_non_esphome_topic() {
        let d = EspHome;
        assert!(d.parse("whatever/else", b"{}").is_none());
    }
}
