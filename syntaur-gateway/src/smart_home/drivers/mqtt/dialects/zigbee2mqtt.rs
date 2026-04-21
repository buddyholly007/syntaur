//! Zigbee2MQTT dialect — critical for v1 because native Zigbee support
//! is deferred to v1.x. This is the ONLY path a Syntaur install sees
//! existing-Zigbee-setup devices on.
//!
//! Topics (v1 handles first; Phase C wires the rest):
//!   - `zigbee2mqtt/bridge/devices`  — retained JSON ARRAY, inventory [v1]
//!   - `zigbee2mqtt/bridge/info`     — retained JSON, coord + version info
//!   - `zigbee2mqtt/bridge/state`    — retained, online/offline LWT [Phase C]
//!   - `zigbee2mqtt/bridge/event`    — join/leave events [Phase C]
//!   - `zigbee2mqtt/<friendly_name>` — per-device state [Phase C]
//!
//! Kind inference walks `definition.exposes` (Z2M publishes structured
//! capability metadata) rather than string-matching model IDs, so new
//! devices classify correctly as long as Z2M knows them.
//!
//! Spec: https://www.zigbee2mqtt.io/guide/usage/mqtt_topics_and_messages.html

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct Zigbee2Mqtt;

impl Dialect for Zigbee2Mqtt {
    fn id(&self) -> &'static str {
        "z2m"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "zigbee2mqtt/bridge/devices",
            // Phase C adds: zigbee2mqtt/bridge/state, bridge/event,
            // zigbee2mqtt/+ (per-device state).
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if topic != "zigbee2mqtt/bridge/devices" {
            return None;
        }
        let payload_s = std::str::from_utf8(payload).ok()?;
        let candidates = parse_bridge_devices(payload_s)?;
        if candidates.is_empty() {
            return None;
        }
        Some(DialectMessage::Discoveries(candidates))
    }
}

/// Parse `zigbee2mqtt/bridge/devices`. Returns `None` on invalid JSON
/// or non-array payloads. The Z2M coordinator itself appears in the
/// array (`type` == "Coordinator") and is skipped — it's not a device
/// from the user's perspective.
pub fn parse_bridge_devices(payload: &str) -> Option<Vec<ScanCandidate>> {
    let arr: Vec<Value> = serde_json::from_str(payload).ok()?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        if let Some(c) = device_to_candidate(&entry) {
            out.push(c);
        }
    }
    Some(out)
}

fn device_to_candidate(entry: &Value) -> Option<ScanCandidate> {
    let dtype = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if dtype == "Coordinator" {
        return None;
    }
    let ieee = entry.get("ieee_address").and_then(|v| v.as_str())?;
    let friendly = entry
        .get("friendly_name")
        .and_then(|v| v.as_str())
        .unwrap_or(ieee);
    let manufacturer = entry
        .get("manufacturer")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let kind = infer_kind(entry);
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("z2m:{}", ieee),
        name: friendly.to_string(),
        kind,
        vendor: manufacturer,
        ip: None,
        mac: None,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "zigbee2mqtt",
            "ieee_address": ieee,
            "type": dtype,
            "model_id": entry.get("model_id"),
        }),
    })
}

/// Infer `smart_home_devices.kind` from the Z2M `definition.exposes`
/// capability array. Walks both top-level entries (sensors) and
/// composite entries' `features` (lights/switches/covers).
fn infer_kind(entry: &Value) -> String {
    let Some(exposes) = entry
        .get("definition")
        .and_then(|d| d.get("exposes"))
        .and_then(|v| v.as_array())
    else {
        return "unknown".into();
    };

    let has_feature = |name: &str| -> bool {
        exposes.iter().any(|e| {
            if e.get("name").and_then(|n| n.as_str()) == Some(name) {
                return true;
            }
            if let Some(features) = e.get("features").and_then(|v| v.as_array()) {
                return features
                    .iter()
                    .any(|f| f.get("name").and_then(|n| n.as_str()) == Some(name));
            }
            false
        })
    };

    let has_type = |t: &str| -> bool {
        exposes
            .iter()
            .any(|e| e.get("type").and_then(|v| v.as_str()) == Some(t))
    };

    // Most-specific first: locks/covers/thermostats before generic state.
    if has_type("lock") || has_feature("lock_state") {
        return "lock".into();
    }
    if has_type("cover") {
        return "cover".into();
    }
    if has_type("climate") {
        return "thermostat".into();
    }
    if has_type("fan") {
        return "fan".into();
    }
    if has_type("light") {
        return "light".into();
    }
    if has_feature("occupancy") || has_feature("motion") {
        return "sensor_motion".into();
    }
    if has_feature("contact") {
        return "sensor_contact".into();
    }
    if has_feature("temperature") || has_feature("humidity") {
        return "sensor_climate".into();
    }
    if has_type("switch") || has_feature("state") {
        return "switch".into();
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_device(friendly: &str, ieee: &str, exposes: Value) -> Value {
        json!({
            "ieee_address": ieee,
            "friendly_name": friendly,
            "type": "EndDevice",
            "manufacturer": "IKEA",
            "model_id": "TRADFRI bulb E27 WS",
            "definition": { "exposes": exposes },
        })
    }

    #[test]
    fn parse_bridge_devices_array() {
        let payload = json!([
            sample_device("living_room_light", "0x00158d0001abcdef", json!([
                {"type": "light", "features": [
                    {"name": "state", "property": "state"},
                    {"name": "brightness", "property": "brightness"}
                ]}
            ])),
            sample_device("kitchen_motion", "0x00158d0002abcdef", json!([
                {"name": "occupancy", "property": "occupancy"}
            ])),
        ])
        .to_string();
        let out = parse_bridge_devices(&payload).expect("array");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, "light");
        assert_eq!(out[0].name, "living_room_light");
        assert_eq!(out[0].external_id, "z2m:0x00158d0001abcdef");
        assert_eq!(out[1].kind, "sensor_motion");
    }

    #[test]
    fn parse_skips_coordinator_entry() {
        let payload = json!([
            {"ieee_address": "0x1111", "type": "Coordinator", "friendly_name": "Coordinator"},
            sample_device("plug", "0x00158d0003", json!([
                {"name": "state", "property": "state"}
            ])),
        ])
        .to_string();
        let out = parse_bridge_devices(&payload).expect("array");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "z2m:0x00158d0003");
    }

    #[test]
    fn infer_kind_lock() {
        let entry = sample_device("front_door", "0xFF", json!([{"type": "lock"}]));
        assert_eq!(infer_kind(&entry), "lock");
    }

    #[test]
    fn infer_kind_contact_sensor() {
        let entry = sample_device("window", "0xFF", json!([{"name": "contact"}]));
        assert_eq!(infer_kind(&entry), "sensor_contact");
    }

    #[test]
    fn infer_kind_climate_sensor() {
        let entry = sample_device("bedroom_th", "0xFF", json!([
            {"name": "temperature"},
            {"name": "humidity"},
            {"name": "battery"}
        ]));
        assert_eq!(infer_kind(&entry), "sensor_climate");
    }

    #[test]
    fn infer_kind_falls_back_unknown_for_empty_exposes() {
        let entry = json!({"ieee_address": "0xFF", "definition": {"exposes": []}});
        assert_eq!(infer_kind(&entry), "unknown");
    }

    #[test]
    fn dialect_ignores_non_bridge_devices_topic() {
        let d = Zigbee2Mqtt;
        assert!(d.parse("zigbee2mqtt/some_device", b"{}").is_none());
    }

    #[test]
    fn dialect_returns_discoveries_variant_with_many_candidates() {
        let d = Zigbee2Mqtt;
        let payload = json!([
            sample_device("a", "0x01", json!([{"name": "state"}])),
            sample_device("b", "0x02", json!([{"name": "state"}])),
            sample_device("c", "0x03", json!([{"name": "state"}])),
        ])
        .to_string();
        let msg = d
            .parse("zigbee2mqtt/bridge/devices", payload.as_bytes())
            .expect("some");
        match msg {
            DialectMessage::Discoveries(list) => assert_eq!(list.len(), 3),
            _ => panic!("expected Discoveries variant"),
        }
    }

    #[test]
    fn dialect_returns_none_for_empty_inventory() {
        let d = Zigbee2Mqtt;
        let msg = d.parse("zigbee2mqtt/bridge/devices", b"[]");
        assert!(msg.is_none());
    }
}
