//! Tasmota dialect — discovery today, STATE/SENSOR/POWER/LWT in Phase B4.
//!
//! v1 handles the discovery path:
//!   `tasmota/discovery/<mac>/config` with a JSON payload containing
//!   `dn` (display name), `fn` (array of friendly names), `ip`, `mac`,
//!   `hn` (hostname).
//!
//! Phase B4 extends this dialect to parse the runtime topics:
//!   - `tele/<topic>/STATE`   — periodic JSON state dump (power, dimmer, wifi)
//!   - `tele/<topic>/SENSOR`  — sensor readings (DS18B20, DHT, ENERGY, …)
//!   - `tele/<topic>/LWT`     — `Online` / `Offline` availability
//!   - `stat/<topic>/POWER`   — relay state on change (`ON` / `OFF`)
//!   - `stat/<topic>/POWER1+` — multi-relay devices
//!
//! Spec: https://tasmota.github.io/docs/MQTT/

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct Tasmota;

impl Dialect for Tasmota {
    fn id(&self) -> &'static str {
        "tasmota"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "tasmota/discovery/+/config",
            // Phase B4 adds tele/+/STATE, tele/+/SENSOR, tele/+/LWT,
            // stat/+/POWER, stat/+/POWER+ here.
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if !topic.starts_with("tasmota/discovery/") {
            return None;
        }
        let payload_s = std::str::from_utf8(payload).ok()?;
        parse_discovery(topic, payload_s).map(DialectMessage::Discovery)
    }
}

/// Parse one Tasmota `tasmota/discovery/<mac>/config` message.
pub fn parse_discovery(topic: &str, payload: &str) -> Option<ScanCandidate> {
    let parts: Vec<&str> = topic.split('/').collect();
    let mac = parts.get(2).copied().unwrap_or("").to_string();
    let j: Value = serde_json::from_str(payload).ok()?;
    let name = j
        .get("dn")
        .or_else(|| j.get("fn").and_then(|v| v.get(0)))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Tasmota {}", mac));
    let ip = j
        .get("ip")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("tasmota:{}", mac),
        name,
        kind: "plug".into(),
        vendor: Some("Tasmota".into()),
        ip,
        mac: if mac.is_empty() { None } else { Some(mac) },
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "tasmota",
            "topic": topic,
            "payload": j,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tasmota_discovery_extracts_ip_and_mac() {
        let payload = r#"{"ip":"192.168.1.50","dn":"Kitchen Plug","hn":"tasmota-001","mac":"AABBCCDDEEFF"}"#;
        let c = parse_discovery("tasmota/discovery/AABBCCDDEEFF/config", payload)
            .expect("candidate");
        assert_eq!(c.name, "Kitchen Plug");
        assert_eq!(c.vendor.as_deref(), Some("Tasmota"));
        assert_eq!(c.ip.as_deref(), Some("192.168.1.50"));
        assert_eq!(c.mac.as_deref(), Some("AABBCCDDEEFF"));
    }

    #[test]
    fn dialect_returns_none_for_non_tasmota_topic() {
        let d = Tasmota;
        assert!(d.parse("shellies/foo/announce", b"{}").is_none());
    }
}
