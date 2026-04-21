//! Tasmota dialect — discovery + runtime (STATE/SENSOR/POWER/LWT).
//!
//! Discovery path (one-shot inventory):
//!   `tasmota/discovery/<mac>/config` with a JSON payload containing
//!   `dn` (display name), `fn` (array of friendly names), `ip`, `mac`,
//!   `hn` (hostname), `t` / `tp` (MQTT topic / prefix).
//!
//! Runtime path (Phase C — streamed into the supervisor's state cache):
//!   - `tele/<topic>/STATE`   — periodic JSON state dump (power, dimmer, wifi)
//!   - `tele/<topic>/SENSOR`  — sensor readings (DS18B20, DHT, ENERGY, …)
//!   - `tele/<topic>/LWT`     — `Online` / `Offline` availability
//!   - `stat/<topic>/POWER`   — relay state on change (`ON` / `OFF`)
//!   - `stat/<topic>/POWER1+` — multi-relay devices
//!
//! The runtime `<topic>` is the user-configured Tasmota MQTT topic
//! (default `tasmota_<last6mac>` — configurable, often renamed per
//! device). We emit runtime events under `external_id =
//! "tasmota_topic:<topic>"`; the discovery path indexes by MAC
//! (`tasmota:<mac>`). Phase D's reconciliation maps the two via the
//! discovery payload's `t` field (see `details.payload.t`). Until that
//! lands, a Tasmota device discovered via `tasmota/discovery/...` and
//! producing runtime updates will surface as two related records —
//! acceptable for v1 because the user-facing UI is still in scaffold.
//!
//! Spec: https://tasmota.github.io/docs/MQTT/

use serde_json::Value;

use super::{DeviceStateUpdate, Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct Tasmota;

impl Dialect for Tasmota {
    fn id(&self) -> &'static str {
        "tasmota"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "tasmota/discovery/+/config",
            "tele/+/STATE",
            "tele/+/SENSOR",
            "tele/+/LWT",
            "stat/+/POWER",
            "stat/+/POWER+",
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if topic.starts_with("tasmota/discovery/") {
            let payload_s = std::str::from_utf8(payload).ok()?;
            return parse_discovery(topic, payload_s).map(DialectMessage::Discovery);
        }
        if let Some(rest) = topic.strip_prefix("tele/") {
            // rest = "<topic>/STATE" | "<topic>/SENSOR" | "<topic>/LWT"
            let (dev_topic, suffix) = split_last(rest)?;
            return match suffix {
                "LWT" => parse_lwt(dev_topic, payload),
                "STATE" => parse_state(dev_topic, payload),
                "SENSOR" => parse_sensor(dev_topic, payload),
                _ => None,
            };
        }
        if let Some(rest) = topic.strip_prefix("stat/") {
            let (dev_topic, suffix) = split_last(rest)?;
            if suffix == "POWER" || (suffix.starts_with("POWER") && suffix.len() > 5) {
                return parse_power(dev_topic, suffix, payload);
            }
        }
        None
    }
}

fn split_last(s: &str) -> Option<(&str, &str)> {
    let idx = s.rfind('/')?;
    Some((&s[..idx], &s[idx + 1..]))
}

fn dev_external_id(dev_topic: &str) -> String {
    format!("tasmota_topic:{}", dev_topic)
}

fn parse_lwt(dev_topic: &str, payload: &[u8]) -> Option<DialectMessage> {
    let s = std::str::from_utf8(payload).ok()?.trim();
    let online = match s {
        "Online" | "online" => true,
        "Offline" | "offline" => false,
        _ => return None,
    };
    Some(DialectMessage::Availability {
        external_id: dev_external_id(dev_topic),
        online,
    })
}

/// `tele/<topic>/STATE` ships a JSON blob with POWER/Dimmer/Wifi/etc.
/// We pass the parsed JSON through as-is — the state cache hash-diffs on
/// whatever the device chose to include. Subsequent UI can cherry-pick
/// fields without the dialect needing to know the schema up front.
fn parse_state(dev_topic: &str, payload: &[u8]) -> Option<DialectMessage> {
    let s = std::str::from_utf8(payload).ok()?;
    let v: Value = serde_json::from_str(s).ok()?;
    Some(DialectMessage::State(DeviceStateUpdate {
        external_id: dev_external_id(dev_topic),
        state: v,
        source: "tasmota".into(),
    }))
}

/// `tele/<topic>/SENSOR` ships periodic sensor readings. Stored under
/// the `sensor` key so a device that publishes both STATE and SENSOR
/// surfaces both readings in the cache without clobbering.
fn parse_sensor(dev_topic: &str, payload: &[u8]) -> Option<DialectMessage> {
    let s = std::str::from_utf8(payload).ok()?;
    let v: Value = serde_json::from_str(s).ok()?;
    Some(DialectMessage::State(DeviceStateUpdate {
        external_id: dev_external_id(dev_topic),
        state: serde_json::json!({ "sensor": v }),
        source: "tasmota".into(),
    }))
}

/// `stat/<topic>/POWER` (or `POWER1`/`POWER2`/…) — plain `ON`/`OFF`.
/// Nested under `{"relays": {"POWER1": "ON"}}` so multi-relay devices
/// don't collapse each other's state.
fn parse_power(dev_topic: &str, suffix: &str, payload: &[u8]) -> Option<DialectMessage> {
    let s = std::str::from_utf8(payload).ok()?.trim();
    let on = match s {
        "ON" | "on" | "1" => true,
        "OFF" | "off" | "0" => false,
        _ => return None,
    };
    Some(DialectMessage::State(DeviceStateUpdate {
        external_id: dev_external_id(dev_topic),
        state: serde_json::json!({ "relays": { suffix: if on { "ON" } else { "OFF" } } }),
        source: "tasmota".into(),
    }))
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

    #[test]
    fn parse_tele_state_produces_state_variant() {
        let d = Tasmota;
        let payload = br#"{"Time":"2026-04-21T10:00:00","POWER":"ON","Dimmer":60}"#;
        let msg = d
            .parse("tele/kitchen_plug/STATE", payload)
            .expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "tasmota_topic:kitchen_plug");
                assert_eq!(u.source, "tasmota");
                assert_eq!(u.state["POWER"], "ON");
                assert_eq!(u.state["Dimmer"], 60);
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parse_stat_power_plain_text() {
        let d = Tasmota;
        let msg = d.parse("stat/kitchen_plug/POWER", b"OFF").expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "tasmota_topic:kitchen_plug");
                assert_eq!(u.state["relays"]["POWER"], "OFF");
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parse_stat_power_multi_relay_keeps_suffix() {
        let d = Tasmota;
        let msg = d.parse("stat/strip/POWER2", b"ON").expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.state["relays"]["POWER2"], "ON");
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parse_lwt_online_offline() {
        let d = Tasmota;
        let on = d.parse("tele/plug/LWT", b"Online").expect("some");
        let off = d.parse("tele/plug/LWT", b"Offline").expect("some");
        match on {
            DialectMessage::Availability { external_id, online } => {
                assert_eq!(external_id, "tasmota_topic:plug");
                assert!(online);
            }
            _ => panic!("expected Availability"),
        }
        match off {
            DialectMessage::Availability { online, .. } => assert!(!online),
            _ => panic!("expected Availability"),
        }
    }

    #[test]
    fn parse_sensor_nests_under_sensor_key() {
        let d = Tasmota;
        let payload = br#"{"Time":"2026-04-21","DS18B20":{"Id":"A","Temperature":22.3}}"#;
        let msg = d.parse("tele/outside/SENSOR", payload).expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "tasmota_topic:outside");
                assert_eq!(u.state["sensor"]["DS18B20"]["Temperature"], 22.3);
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parse_lwt_rejects_unknown_payload() {
        let d = Tasmota;
        assert!(d.parse("tele/plug/LWT", b"maybe").is_none());
    }
}
