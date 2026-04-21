//! Home Assistant MQTT Discovery dialect.
//!
//! Topic:   `homeassistant/<component>/<object_id>/config`
//!          `homeassistant/<component>/<node_id>/<object_id>/config`
//! Payload: JSON — `name`, `unique_id`, `state_topic`, `command_topic`,
//!          `device`, optional component-specific keys (brightness,
//!          device_class, value_template, ...).
//!
//! This is the most common smart-home MQTT dialect — Tasmota, ESPHome,
//! Shelly Gen2+, and Zigbee2MQTT all publish HA-compatible discovery
//! messages when configured to. The `DialectRouter` registers HA
//! LAST so a device's native dialect gets first crack at its topic;
//! the scan pipeline's `external_id`-keyed dedupe handles any
//! remaining overlap.
//!
//! Spec: https://www.home-assistant.io/integrations/mqtt/

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct HaDiscovery;

impl Dialect for HaDiscovery {
    fn id(&self) -> &'static str {
        "ha"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "homeassistant/+/+/config",
            "homeassistant/+/+/+/config",
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if !topic.starts_with("homeassistant/") {
            return None;
        }
        let payload_s = std::str::from_utf8(payload).ok()?;
        parse_config(topic, payload_s).map(DialectMessage::Discovery)
    }
}

/// Parse one HA discovery config message. Public for unit tests and
/// for callers that want to drive the parser directly without going
/// through `DialectRouter`.
pub fn parse_config(topic: &str, payload: &str) -> Option<ScanCandidate> {
    // `homeassistant/<component>/<node_id>/[<object_id>/]config`
    let parts: Vec<&str> = topic.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let component = parts[1];
    // Last non-"config" segment is the object id.
    let object_id = if parts.len() == 4 { parts[2] } else { parts[3] };

    let j: Value = serde_json::from_str(payload).ok()?;
    let name = j
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| object_id.to_string());
    let unique_id = j
        .get("unique_id")
        .or_else(|| j.get("uniq_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| topic.to_string());
    let kind = component_to_kind(component);
    let vendor = j
        .get("device")
        .and_then(|d| d.get("manufacturer"))
        .or_else(|| j.get("device").and_then(|d| d.get("mf")))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("ha:{}", unique_id),
        name,
        kind,
        vendor,
        ip: None,
        mac: None,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "ha_discovery",
            "topic": topic,
            "component": component,
            "payload": j,
        }),
    })
}

/// Map HA's component identifier → smart_home_devices.kind.
pub fn component_to_kind(component: &str) -> String {
    match component {
        "light" => "light",
        "switch" => "switch",
        "lock" => "lock",
        "climate" => "thermostat",
        "binary_sensor" => "sensor_motion",
        "sensor" => "sensor_climate",
        "camera" => "camera",
        "fan" => "fan",
        "cover" => "cover",
        "vacuum" => "vacuum",
        "media_player" => "media_player",
        _ => "unknown",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ha_light_discovery() {
        let payload = r#"{"name":"Bedroom Lamp","unique_id":"bed_lamp_01","command_topic":"zigbee2mqtt/bedroom_lamp/set","device":{"manufacturer":"IKEA"}}"#;
        let c = parse_config("homeassistant/light/bed_lamp_01/config", payload)
            .expect("candidate");
        assert_eq!(c.driver, "mqtt");
        assert_eq!(c.kind, "light");
        assert_eq!(c.name, "Bedroom Lamp");
        assert_eq!(c.vendor.as_deref(), Some("IKEA"));
        assert!(c.external_id.contains("bed_lamp_01"));
    }

    #[test]
    fn parse_ha_binary_sensor_is_motion() {
        let payload = r#"{"name":"Hall PIR","unique_id":"hall_pir_1","device_class":"motion"}"#;
        let c = parse_config("homeassistant/binary_sensor/hall_pir_1/config", payload)
            .expect("candidate");
        assert_eq!(c.kind, "sensor_motion");
    }

    #[test]
    fn component_to_kind_fallback_unknown() {
        assert_eq!(component_to_kind("something-new"), "unknown");
    }

    #[test]
    fn dialect_returns_none_for_non_ha_topic() {
        let d = HaDiscovery;
        assert!(d.parse("tasmota/discovery/aabbcc/config", b"{}").is_none());
    }

    #[test]
    fn dialect_parses_via_trait_surface() {
        let d = HaDiscovery;
        let payload = br#"{"name":"Kitchen","unique_id":"kit_1"}"#;
        let m = d
            .parse("homeassistant/switch/kit_1/config", payload)
            .expect("some");
        match m {
            DialectMessage::Discovery(c) => {
                assert_eq!(c.kind, "switch");
                assert_eq!(c.name, "Kitchen");
            }
            other => panic!("expected Discovery, got {:?}", std::mem::discriminant(&other)),
        }
    }
}
