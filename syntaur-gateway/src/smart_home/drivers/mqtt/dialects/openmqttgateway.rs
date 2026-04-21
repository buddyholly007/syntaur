//! OpenMQTTGateway (OMG) dialect — BLE/RF/IR bridge output.
//!
//! OMG is an ESP32/-style firmware that sniffs radio protocols (BLE,
//! 433MHz RF, 868MHz RF, IR, LoRa) and republishes frames to MQTT.
//! Useful for legacy sensors that don't speak Wi-Fi / Zigbee / Z-Wave
//! directly (Oregon temp probes, Govee BLE thermometers, cheap motion
//! sensors, etc.).
//!
//! Topics seen in the wild (gateway id varies — `+` wildcard):
//!   - `home/+/BTtoMQTT/<mac>`           — BLE presence / sensor JSON  [v1]
//!   - `home/+/RFtoMQTT`                 — 433MHz (not scoped per-device)
//!   - `home/+/RF2toMQTT`, `SRFBtoMQTT`  — other RF bands
//!   - `home/+/IRtoMQTT`                 — IR capture
//!   - `home/+/LORAtoMQTT`               — LoRa packets
//!   - `home/+/LWT`                      — gateway presence
//!
//! v1 parses only BLE frames: MAC is a stable `external_id`, payload
//! JSON is rich enough to infer `kind`. RF/IR/LoRa payloads use
//! per-protocol identifiers that aren't reliable across gateway
//! restarts, so skipping them in v1 avoids creating phantom devices.
//!
//! BLE frames are NOT retained by OMG — they fire on each chirp. The
//! 5-second scan window will miss infrequent beacons (AirTags chirp
//! every ~2s, Govee sensors every ~10-30s). Phase C's long-running
//! subscriber captures every frame, not a 5-second slice.
//!
//! Spec: https://docs.openmqttgateway.com/

use serde_json::Value;

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct OpenMqttGateway;

impl Dialect for OpenMqttGateway {
    fn id(&self) -> &'static str {
        "omg"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "home/+/BTtoMQTT/+",
            // RF / IR / LoRa intentionally omitted — see module docs.
            // Phase C adds home/+/LWT for gateway availability.
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        let parts: Vec<&str> = topic.split('/').collect();
        if parts.len() != 4 || parts[0] != "home" || parts[2] != "BTtoMQTT" {
            return None;
        }
        let mac = parts[3];
        let Ok(payload_s) = std::str::from_utf8(payload) else {
            return None;
        };
        parse_ble(mac, payload_s).map(DialectMessage::Discovery)
    }
}

/// Build a ScanCandidate from a BLE frame.
pub fn parse_ble(mac: &str, payload: &str) -> Option<ScanCandidate> {
    let j: Value = serde_json::from_str(payload).ok()?;
    let model_id = j
        .get("model_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let model = j.get("model").and_then(|v| v.as_str()).map(str::to_string);
    let name = j
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| model.clone())
        .or_else(|| model_id.clone())
        .unwrap_or_else(|| format!("BLE {}", mac));
    let kind = infer_kind(&j).to_string();
    let mac_upper = mac.to_ascii_uppercase();
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("omg_ble:{}", mac_upper),
        name,
        kind,
        vendor: model_id.or(model),
        ip: None,
        mac: Some(mac_upper),
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "openmqttgateway_ble",
            "topic_mac": mac,
            "payload": j,
        }),
    })
}

/// Infer `kind` from OMG's structured BLE payload.
///
/// Field conventions (from OMG's BLE decoders):
///   - `tempc` / `temperature` / `humidity`  → sensor_climate
///   - `motion` / `presence` / `occupancy`   → sensor_motion
///   - `contact` / `opening`                 → sensor_contact
///   - bare beacon (rssi + battery, no data) → sensor_motion (presence proxy)
fn infer_kind(j: &Value) -> &'static str {
    let has = |k: &str| j.get(k).is_some();
    if has("contact") || has("opening") {
        return "sensor_contact";
    }
    if has("motion") || has("presence") || has("occupancy") {
        return "sensor_motion";
    }
    if has("tempc") || has("temperature") || has("humidity") || has("pressure") {
        return "sensor_climate";
    }
    // Bare presence beacon — no data, just a MAC + rssi + battery.
    if has("rssi") && has("battery") {
        return "sensor_motion";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_ble_govee_th_sensor() {
        let payload = json!({
            "id": "AA:BB:CC:DD:EE:FF",
            "model": "H5075",
            "model_id": "GVH5075",
            "tempc": 22.4,
            "hum": 48.5,
            "temperature": 22.4,
            "humidity": 48.5,
            "battery": 80,
            "rssi": -72
        })
        .to_string();
        let c = parse_ble("AA:BB:CC:DD:EE:FF", &payload).expect("candidate");
        assert_eq!(c.kind, "sensor_climate");
        assert_eq!(c.mac.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(c.external_id, "omg_ble:AA:BB:CC:DD:EE:FF");
        assert_eq!(c.vendor.as_deref(), Some("GVH5075"));
    }

    #[test]
    fn parse_ble_motion_beacon() {
        let payload = json!({"motion": true, "rssi": -60}).to_string();
        let c = parse_ble("11:22:33:44:55:66", &payload).expect("candidate");
        assert_eq!(c.kind, "sensor_motion");
    }

    #[test]
    fn parse_ble_contact_sensor() {
        let payload = json!({"contact": false, "battery": 90, "rssi": -55}).to_string();
        let c = parse_ble("ab:cd:ef:12:34:56", &payload).expect("candidate");
        assert_eq!(c.kind, "sensor_contact");
        // MAC uppercased for stable external_id.
        assert_eq!(c.mac.as_deref(), Some("AB:CD:EF:12:34:56"));
    }

    #[test]
    fn parse_ble_bare_beacon_counts_as_presence() {
        let payload = json!({"rssi": -78, "battery": 55}).to_string();
        let c = parse_ble("DE:AD:BE:EF:00:01", &payload).expect("candidate");
        assert_eq!(c.kind, "sensor_motion");
    }

    #[test]
    fn parse_ble_data_free_payload_is_unknown() {
        let payload = json!({"rssi": -80}).to_string(); // no battery either
        let c = parse_ble("00:11:22:33:44:55", &payload).expect("candidate");
        assert_eq!(c.kind, "unknown");
    }

    #[test]
    fn parse_ble_name_fallback_chain() {
        // name > model > model_id > "BLE <mac>"
        let with_name = json!({"name": "Kitchen Window", "rssi": -70}).to_string();
        assert_eq!(parse_ble("aa:00", &with_name).unwrap().name, "Kitchen Window");

        let with_model = json!({"model": "Eve Door", "rssi": -70}).to_string();
        assert_eq!(parse_ble("aa:00", &with_model).unwrap().name, "Eve Door");

        let only_id = json!({"model_id": "EVEDOOR2", "rssi": -70}).to_string();
        assert_eq!(parse_ble("aa:00", &only_id).unwrap().name, "EVEDOOR2");

        let nothing = json!({"rssi": -70}).to_string();
        assert_eq!(parse_ble("aa:00", &nothing).unwrap().name, "BLE aa:00");
    }

    #[test]
    fn dialect_accepts_home_prefix_ble_topic() {
        let d = OpenMqttGateway;
        let payload = br#"{"motion": true}"#;
        let m = d
            .parse("home/OMG_main/BTtoMQTT/AABBCCDDEEFF", payload)
            .expect("some");
        match m {
            DialectMessage::Discovery(c) => {
                assert_eq!(c.mac.as_deref(), Some("AABBCCDDEEFF"));
                assert_eq!(c.kind, "sensor_motion");
            }
            other => panic!("expected Discovery, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn dialect_ignores_rf_and_ir_topics() {
        let d = OpenMqttGateway;
        assert!(d.parse("home/OMG_main/RFtoMQTT", b"{}").is_none());
        assert!(d.parse("home/OMG_main/IRtoMQTT", b"{}").is_none());
    }

    #[test]
    fn dialect_ignores_wrong_prefix() {
        let d = OpenMqttGateway;
        assert!(d.parse("notmyhome/x/BTtoMQTT/aa", b"{}").is_none());
    }

    #[test]
    fn dialect_ignores_malformed_topic_depth() {
        let d = OpenMqttGateway;
        // Too few segments.
        assert!(d.parse("home/OMG/BTtoMQTT", b"{}").is_none());
        // Too many.
        assert!(d.parse("home/OMG/BTtoMQTT/aa/extra", b"{}").is_none());
    }
}
