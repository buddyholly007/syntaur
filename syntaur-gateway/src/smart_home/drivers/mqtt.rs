//! MQTT driver — pure-Rust client for device discovery + control via a
//! user-supplied broker (Mosquitto on the user's LAN, the HA add-on, a
//! cloud broker, etc.). No in-process broker in v1; that can ship in
//! v1.x if users ask for it.
//!
//! Scan behavior: connect, subscribe to the common discovery topic
//! patterns for a short window, parse each retained message into a
//! `ScanCandidate`, disconnect. If the broker is unreachable or no
//! config is present, return an empty vec (graceful degrade).
//!
//! Supported retained-discovery patterns:
//!   `homeassistant/<component>/<node_id>/<object_id>/config` — the
//!     ubiquitous Home Assistant MQTT discovery schema used by z2m,
//!     Tasmota, ESPHome, Shelly Gen2, and most bridged devices.
//!   `tasmota/discovery/<mac>/config`
//!   `shellies/<id>/announce` — Shelly Gen1 one-shot announcement.
//!   `esphome/discover/<hostname>` — optional, some ESPHome builds.
//!
//! Configuration: we read broker details from an optional
//! `SMART_HOME_MQTT_URL` environment variable of the form
//! `mqtt://[user:pass@]host[:port]`. A proper `/settings/mqtt` page is
//! deferred — keeping config surface tight while the driver matures.

use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde_json::Value;

use crate::smart_home::scan::ScanCandidate;

const DEFAULT_SCAN_SECONDS: u64 = 5;
const TOPICS: &[&str] = &[
    "homeassistant/+/+/config",
    "homeassistant/+/+/+/config",
    "tasmota/discovery/+/config",
    "shellies/+/announce",
    "esphome/discover/+",
];

/// Top-level scan entry. Returns empty if no broker configured or the
/// connection fails — Matter/Wi-Fi scan paths must not be poisoned by a
/// flaky MQTT broker.
pub async fn scan() -> Vec<ScanCandidate> {
    let Some(opts) = mqtt_options_from_env() else {
        log::debug!("[smart_home::mqtt] SMART_HOME_MQTT_URL not set, skipping scan");
        return Vec::new();
    };
    scan_with_options(opts, Duration::from_secs(DEFAULT_SCAN_SECONDS)).await
}

/// Public so integration tests (and a future UI "test connection"
/// button) can exercise a specific broker without going through env.
pub async fn scan_with_options(opts: MqttOptions, window: Duration) -> Vec<ScanCandidate> {
    let (client, mut event_loop) = AsyncClient::new(opts, 32);
    for topic in TOPICS {
        if let Err(e) = client.subscribe(*topic, QoS::AtMostOnce).await {
            log::warn!("[smart_home::mqtt] subscribe {} failed: {}", topic, e);
        }
    }

    let mut candidates: Vec<ScanCandidate> = Vec::new();
    let deadline = tokio::time::Instant::now() + window;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let next = tokio::time::timeout(remaining, event_loop.poll()).await;
        let event = match next {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                log::info!("[smart_home::mqtt] event loop error, ending scan: {}", e);
                break;
            }
            Err(_) => break, // scan window elapsed
        };
        if let Event::Incoming(Incoming::Publish(publish)) = event {
            let topic = publish.topic.clone();
            let payload_str = std::str::from_utf8(&publish.payload)
                .unwrap_or("")
                .to_string();
            if let Some(c) = parse_discovery_message(&topic, &payload_str) {
                candidates.push(c);
            }
        }
    }

    let _ = client.disconnect().await;
    dedupe(candidates)
}

fn mqtt_options_from_env() -> Option<MqttOptions> {
    let url = std::env::var("SMART_HOME_MQTT_URL").ok()?;
    let parsed = url::Url::parse(&url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port().unwrap_or(1883);
    let mut opts = MqttOptions::new("syntaur-smart-home-scan", host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !parsed.username().is_empty() {
        opts.set_credentials(
            parsed.username().to_string(),
            parsed.password().unwrap_or("").to_string(),
        );
    }
    Some(opts)
}

/// Parse a single retained discovery message into a `ScanCandidate`.
/// Public for unit testing.
pub fn parse_discovery_message(topic: &str, payload: &str) -> Option<ScanCandidate> {
    if topic.starts_with("homeassistant/") {
        return parse_ha_discovery(topic, payload);
    }
    if topic.starts_with("tasmota/discovery/") {
        return parse_tasmota_discovery(topic, payload);
    }
    if topic.starts_with("shellies/") && topic.ends_with("/announce") {
        return parse_shelly_announce(topic, payload);
    }
    if topic.starts_with("esphome/discover/") {
        return parse_esphome_discovery(topic, payload);
    }
    None
}

fn parse_ha_discovery(topic: &str, payload: &str) -> Option<ScanCandidate> {
    // `homeassistant/<component>/<node_id>/[<object_id>/]config`
    let parts: Vec<&str> = topic.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let component = parts[1];
    // The "last non-config" segment is the object id — [2] for 4-seg
    // topics, [3] for 5-seg topics.
    let object_id = if parts.len() == 4 {
        parts[2]
    } else {
        parts[3]
    };
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
    let kind = ha_component_to_kind(component);
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

fn ha_component_to_kind(component: &str) -> String {
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

fn parse_tasmota_discovery(topic: &str, payload: &str) -> Option<ScanCandidate> {
    // `tasmota/discovery/<mac>/config`
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
        details: serde_json::json!({ "source": "mqtt", "schema": "tasmota", "topic": topic, "payload": j }),
    })
}

fn parse_shelly_announce(topic: &str, payload: &str) -> Option<ScanCandidate> {
    // `shellies/<id>/announce`
    let parts: Vec<&str> = topic.split('/').collect();
    let id = parts.get(1).copied().unwrap_or("").to_string();
    let j: Value = serde_json::from_str(payload).ok()?;
    let ip = j
        .get("ip")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mac = j
        .get("mac")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("shelly:{}", id),
        name: id.clone(),
        kind: "plug".into(),
        vendor: Some("Shelly".into()),
        ip,
        mac,
        details: serde_json::json!({ "source": "mqtt", "schema": "shelly_gen1", "topic": topic, "payload": j }),
    })
}

fn parse_esphome_discovery(topic: &str, payload: &str) -> Option<ScanCandidate> {
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
        details: serde_json::json!({ "source": "mqtt", "schema": "esphome", "topic": topic, "payload": j }),
    })
}

fn dedupe(mut v: Vec<ScanCandidate>) -> Vec<ScanCandidate> {
    v.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    v.dedup_by(|a, b| a.external_id == b.external_id);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ha_light_discovery() {
        let payload = r#"{"name":"Bedroom Lamp","unique_id":"bed_lamp_01","command_topic":"zigbee2mqtt/bedroom_lamp/set","device":{"manufacturer":"IKEA"}}"#;
        let c = parse_discovery_message(
            "homeassistant/light/bed_lamp_01/config",
            payload,
        )
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
        let c = parse_discovery_message(
            "homeassistant/binary_sensor/hall_pir_1/config",
            payload,
        )
        .expect("candidate");
        assert_eq!(c.kind, "sensor_motion");
    }

    #[test]
    fn parse_tasmota_discovery_extracts_ip_and_mac() {
        let payload = r#"{"ip":"192.168.1.50","dn":"Kitchen Plug","hn":"tasmota-001","mac":"AABBCCDDEEFF"}"#;
        let c = parse_discovery_message(
            "tasmota/discovery/AABBCCDDEEFF/config",
            payload,
        )
        .expect("candidate");
        assert_eq!(c.name, "Kitchen Plug");
        assert_eq!(c.vendor.as_deref(), Some("Tasmota"));
        assert_eq!(c.ip.as_deref(), Some("192.168.1.50"));
        assert_eq!(c.mac.as_deref(), Some("AABBCCDDEEFF"));
    }

    #[test]
    fn parse_shelly_gen1_announce() {
        let payload = r#"{"id":"shellyplug-s-aabbcc","ip":"192.168.1.60","mac":"AA:BB:CC:DD:EE:FF","fw_ver":"20250101"}"#;
        let c = parse_discovery_message(
            "shellies/shellyplug-s-aabbcc/announce",
            payload,
        )
        .expect("candidate");
        assert_eq!(c.vendor.as_deref(), Some("Shelly"));
        assert_eq!(c.ip.as_deref(), Some("192.168.1.60"));
        assert_eq!(c.kind, "plug");
    }

    #[test]
    fn parse_discovery_ignores_unknown_topic() {
        assert!(parse_discovery_message("whatever/stuff", "{}").is_none());
    }

    #[test]
    fn ha_component_to_kind_fallback_unknown() {
        assert_eq!(ha_component_to_kind("something-new"), "unknown");
    }

    #[test]
    fn dedupe_collapses_duplicates_by_external_id() {
        let c = |id: &str| ScanCandidate {
            driver: "mqtt".into(),
            external_id: id.into(),
            name: id.into(),
            kind: "unknown".into(),
            vendor: None,
            ip: None,
            mac: None,
            details: serde_json::Value::Null,
        };
        let out = dedupe(vec![c("a"), c("b"), c("a")]);
        assert_eq!(out.len(), 2);
    }
}
