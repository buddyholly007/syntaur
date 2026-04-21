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
    // Prefer device_class when present — HA Discovery carries it
    // alongside the component to disambiguate (a `binary_sensor` with
    // device_class="door" isn't a motion detector). `unit_of_measurement`
    // disambiguates numeric sensors (°C → climate, dBm → wifi_signal).
    let device_class = j
        .get("device_class")
        .or_else(|| j.get("dev_cla"))
        .and_then(|v| v.as_str());
    let unit = j
        .get("unit_of_measurement")
        .or_else(|| j.get("unit_of_meas"))
        .and_then(|v| v.as_str());
    let kind = refine_kind(component, device_class, unit);
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

/// Map HA's component identifier → smart_home_devices.kind. Used as
/// a fallback when no device_class / unit is available to refine.
pub fn component_to_kind(component: &str) -> String {
    match component {
        "light" => "light",
        "switch" => "switch",
        "lock" => "lock",
        "climate" => "thermostat",
        "binary_sensor" => "sensor_binary",
        "sensor" => "sensor",
        "camera" => "camera",
        "fan" => "fan",
        "cover" => "cover",
        "vacuum" => "vacuum",
        "media_player" => "media_player",
        "button" => "button",
        "number" => "sensor",
        "select" => "select",
        "text" => "text",
        "update" => "update",
        _ => "unknown",
    }
    .to_string()
}

/// Refine `component_to_kind` using the payload's `device_class` (HA's
/// authoritative type hint) and `unit_of_measurement` (numeric sensor
/// fallback). When both are absent we fall back to the component map.
pub fn refine_kind(
    component: &str,
    device_class: Option<&str>,
    unit: Option<&str>,
) -> String {
    // binary_sensor device_class → sensor_<type>. Covers motion,
    // occupancy, presence, door/window, leak, smoke, gas, plug, etc.
    // Unrecognized classes stay as "sensor_binary" which is still
    // an accurate-enough surface for the UI.
    if component == "binary_sensor" {
        let k = match device_class.unwrap_or("") {
            "motion" | "occupancy" | "presence" => "sensor_motion",
            "door" | "window" | "opening" | "garage_door" => "sensor_contact",
            "moisture" | "leak" => "sensor_leak",
            "smoke" => "sensor_smoke",
            "gas" | "carbon_monoxide" | "co" => "sensor_gas",
            "plug" | "power" | "battery_charging" | "connectivity" => "sensor_binary",
            "" => "sensor_binary",
            _ => "sensor_binary",
        };
        return k.to_string();
    }

    // sensor component: device_class first, then unit sniffing.
    if component == "sensor" || component == "number" {
        if let Some(dc) = device_class {
            let k = match dc {
                "temperature" => "sensor_temperature",
                "humidity" => "sensor_humidity",
                "pressure" | "atmospheric_pressure" => "sensor_pressure",
                "illuminance" => "sensor_illuminance",
                "power" | "energy" | "current" | "voltage" | "apparent_power"
                | "reactive_power" => "sensor_power",
                "signal_strength" => "sensor_signal",
                "battery" => "sensor_battery",
                "timestamp" | "duration" => "sensor_time",
                "data_size" => "sensor_data_size",
                "frequency" => "sensor_frequency",
                _ => "sensor",
            };
            return k.to_string();
        }
        if let Some(u) = unit {
            let u = u.trim();
            // Unit hints for untyped numeric sensors (heap sizes etc.).
            return match u {
                "°C" | "°F" | "K" => "sensor_temperature",
                "%" => "sensor".into(),
                "dBm" => "sensor_signal",
                "B" | "KB" | "MB" | "GB" => "sensor_data_size",
                "ms" | "s" | "min" | "h" | "d" => "sensor_time",
                "W" | "kW" | "Wh" | "kWh" | "A" | "V" | "VA" => "sensor_power",
                "hPa" | "Pa" | "kPa" | "inHg" | "mmHg" | "psi" => "sensor_pressure",
                "lx" | "lm" => "sensor_illuminance",
                _ => "sensor",
            }
            .to_string();
        }
    }

    component_to_kind(component)
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
    fn refine_kind_uses_device_class_for_binary_sensor() {
        assert_eq!(refine_kind("binary_sensor", Some("door"), None), "sensor_contact");
        assert_eq!(refine_kind("binary_sensor", Some("occupancy"), None), "sensor_motion");
        assert_eq!(refine_kind("binary_sensor", Some("moisture"), None), "sensor_leak");
        assert_eq!(refine_kind("binary_sensor", None, None), "sensor_binary");
    }

    #[test]
    fn refine_kind_uses_device_class_for_sensor() {
        assert_eq!(refine_kind("sensor", Some("temperature"), None), "sensor_temperature");
        assert_eq!(refine_kind("sensor", Some("power"), None), "sensor_power");
        assert_eq!(refine_kind("sensor", Some("battery"), None), "sensor_battery");
    }

    #[test]
    fn refine_kind_falls_back_to_unit_for_untyped_sensor() {
        assert_eq!(refine_kind("sensor", None, Some("B")), "sensor_data_size");
        assert_eq!(refine_kind("sensor", None, Some("dBm")), "sensor_signal");
        assert_eq!(refine_kind("sensor", None, Some("°C")), "sensor_temperature");
        assert_eq!(refine_kind("sensor", None, Some("s")), "sensor_time");
        assert_eq!(refine_kind("sensor", None, Some("unknown-unit")), "sensor");
    }

    #[test]
    fn refine_kind_covers_new_components() {
        assert_eq!(refine_kind("button", None, None), "button");
        assert_eq!(refine_kind("select", None, None), "select");
        assert_eq!(refine_kind("update", None, None), "update");
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

    /// Opt-in live test — subscribes to `homeassistant/+/+/+/config`
    /// on the broker at `SYNTAUR_LIVE_MQTT_URL` and asserts every
    /// retained frame parses. Validated locally against Sean's HA
    /// Mosquitto with ~30+ ESPHome-generated configs from the two
    /// BT proxies.
    #[test]
    fn live_broker_ha_discovery_configs_parse_cleanly() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::time::Duration;
        use tokio::time::timeout;

        let Ok(url) = std::env::var("SYNTAUR_LIVE_MQTT_URL") else {
            eprintln!("SYNTAUR_LIVE_MQTT_URL not set — skipping live test");
            return;
        };
        let parsed = url::Url::parse(&url).expect("parse url");
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap_or(1883);
        let user = parsed.username().to_string();
        let pass = parsed.password().unwrap_or("").to_string();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut opts = MqttOptions::new("syntaur-ha-live", host, port);
            opts.set_keep_alive(Duration::from_secs(30));
            if !user.is_empty() {
                opts.set_credentials(user, pass);
            }
            let (client, mut event_loop) = AsyncClient::new(opts, 256);
            for filter in ["homeassistant/+/+/config", "homeassistant/+/+/+/config"] {
                client.subscribe(filter, QoS::AtMostOnce).await.ok();
            }

            let dialect = HaDiscovery;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut parsed_count = 0usize;
            let mut parse_failures: Vec<String> = Vec::new();
            let mut kinds: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            while tokio::time::Instant::now() < deadline {
                match timeout(Duration::from_millis(400), event_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        match dialect.parse(&p.topic, &p.payload) {
                            Some(DialectMessage::Discovery(c)) => {
                                parsed_count += 1;
                                *kinds.entry(c.kind.clone()).or_insert(0) += 1;
                            }
                            _ => parse_failures.push(format!("{}", p.topic)),
                        }
                    }
                    _ => continue,
                }
            }

            eprintln!(
                "[ha-discovery-live] parsed={} failures={}",
                parsed_count,
                parse_failures.len()
            );
            for (k, v) in &kinds {
                eprintln!("  kind {}: {}", k, v);
            }
            for f in parse_failures.iter().take(10) {
                eprintln!("  FAIL {}", f);
            }
            assert!(
                parse_failures.is_empty(),
                "{} HA-Discovery configs failed to parse",
                parse_failures.len()
            );
            assert!(parsed_count > 0, "expected at least one HA Discovery config");
        });
    }
}
