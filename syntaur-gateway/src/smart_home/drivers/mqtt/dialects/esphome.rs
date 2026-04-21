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
///
/// Modern ESPHome firmwares (2026+) publish a rich payload:
///   `{"ip":"…","name":"…","friendly_name":"…","port":6053,"version":"…",
///     "mac":"dcb4d9179454","platform":"ESP32","board":"esp32-s3-devkitc-1",
///     "network":"wifi","api_encryption":"Noise_NNpsk0_25519_ChaChaPoly_SHA256"}`
///
/// MAC is lowercase hex with no separators — we normalize to the
/// colonated uppercase form (`DC:B4:D9:17:94:54`) to stay consistent
/// with the other dialects.
pub fn parse_discover(topic: &str, payload: &str) -> Option<ScanCandidate> {
    let parts: Vec<&str> = topic.split('/').collect();
    let host = parts.get(2).copied().unwrap_or("").to_string();
    let j: Value = serde_json::from_str(payload).unwrap_or(Value::Null);

    let friendly = j
        .get("friendly_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = friendly.clone().unwrap_or_else(|| host.clone());

    let ip = j
        .get("ip")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mac = j
        .get("mac")
        .and_then(|v| v.as_str())
        .and_then(normalize_mac);

    // Infer a slightly richer kind from the friendly_name when we
    // recognize a well-known role. BT proxies are common enough to
    // earn their own kind; everything else stays "unknown" until a
    // real signal lands.
    let kind = infer_kind_from_names(&host, friendly.as_deref())
        .unwrap_or("unknown")
        .to_string();

    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("esphome:{}", host),
        name,
        kind,
        vendor: Some("ESPHome".into()),
        ip,
        mac,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "esphome",
            "topic": topic,
            "payload": j,
        }),
    })
}

/// Normalize a MAC to `AA:BB:CC:DD:EE:FF` form. Accepts bare 12-hex
/// (ESPHome's style) and colonated inputs.
fn normalize_mac(raw: &str) -> Option<String> {
    let trimmed: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if trimmed.len() != 12 {
        return None;
    }
    let up = trimmed.to_ascii_uppercase();
    Some(format!(
        "{}:{}:{}:{}:{}:{}",
        &up[0..2],
        &up[2..4],
        &up[4..6],
        &up[6..8],
        &up[8..10],
        &up[10..12],
    ))
}

fn infer_kind_from_names(host: &str, friendly: Option<&str>) -> Option<&'static str> {
    let blob = format!(
        "{} {}",
        host.to_ascii_lowercase(),
        friendly.map(|s| s.to_ascii_lowercase()).unwrap_or_default()
    );
    // Key off common device-class hints in the name. Kept conservative
    // — one wrong kind is worse than a generic "unknown".
    if blob.contains("bt proxy")
        || blob.contains("ble proxy")
        || blob.contains("bluetooth proxy")
        || blob.starts_with("proxy-")
        || blob.contains(" proxy ")
    {
        return Some("bluetooth_proxy");
    }
    if blob.contains("plug") {
        return Some("plug");
    }
    if blob.contains("light") || blob.contains("lamp") {
        return Some("light");
    }
    None
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

    #[test]
    fn parses_full_modern_payload() {
        let payload = r#"{"ip":"192.168.20.229","name":"proxy-kids","friendly_name":"Kids Bath BT Proxy","port":6053,"version":"2026.4.1","mac":"dcb4d9179454","platform":"ESP32","board":"esp32-s3-devkitc-1","network":"wifi","api_encryption":"Noise_NNpsk0_25519_ChaChaPoly_SHA256"}"#;
        let c = parse_discover("esphome/discover/proxy-kids", payload).expect("candidate");
        assert_eq!(c.external_id, "esphome:proxy-kids");
        assert_eq!(c.name, "Kids Bath BT Proxy");
        assert_eq!(c.ip.as_deref(), Some("192.168.20.229"));
        assert_eq!(c.mac.as_deref(), Some("DC:B4:D9:17:94:54"));
        assert_eq!(c.kind, "bluetooth_proxy");
    }

    #[test]
    fn normalizes_mac_to_colon_uppercase() {
        assert_eq!(
            normalize_mac("dcb4d9179454"),
            Some("DC:B4:D9:17:94:54".to_string())
        );
        assert_eq!(
            normalize_mac("DC:B4:D9:17:94:54"),
            Some("DC:B4:D9:17:94:54".to_string())
        );
        assert!(normalize_mac("nope").is_none());
    }

    #[test]
    fn infer_kind_picks_bluetooth_proxy_from_friendly_name() {
        assert_eq!(
            infer_kind_from_names("proxy-kids", Some("Kids Bath BT Proxy")),
            Some("bluetooth_proxy")
        );
        assert_eq!(
            infer_kind_from_names("livingroom_lamp", Some("Living Room Lamp")),
            Some("light")
        );
        assert_eq!(
            infer_kind_from_names("random_sensor", Some("Random")),
            None
        );
    }

    /// Opt-in live test — runs when SYNTAUR_LIVE_MQTT_URL points at a
    /// broker hosting `esphome/discover/*` retained frames. Used
    /// locally against Sean's HA Mosquitto to validate the dialect
    /// picks up both BT proxies.
    #[test]
    fn live_broker_surfaces_every_proxy() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::collections::HashMap;
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
            let mut opts = MqttOptions::new("syntaur-esphome-live", host, port);
            opts.set_keep_alive(Duration::from_secs(30));
            if !user.is_empty() {
                opts.set_credentials(user, pass);
            }
            let (client, mut event_loop) = AsyncClient::new(opts, 64);
            client
                .subscribe("esphome/discover/+", QoS::AtMostOnce)
                .await
                .unwrap();

            let dialect = EspHome;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut by_name: HashMap<String, bool> = HashMap::new();
            while tokio::time::Instant::now() < deadline {
                match timeout(Duration::from_millis(400), event_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        match dialect.parse(&p.topic, &p.payload) {
                            Some(DialectMessage::Discovery(c)) => {
                                // Must have extracted ip + mac and
                                // recognized it as a BT proxy from the
                                // friendly_name.
                                assert!(c.ip.is_some(), "missing ip on {}", c.external_id);
                                assert!(
                                    c.mac.as_deref().unwrap_or("").contains(':'),
                                    "mac not normalized on {}",
                                    c.external_id
                                );
                                by_name.insert(c.external_id.clone(), c.kind == "bluetooth_proxy");
                            }
                            _ => {}
                        }
                    }
                    _ => continue,
                }
            }

            eprintln!(
                "[esphome-live] candidates={} all_bt_proxy={}",
                by_name.len(),
                by_name.values().all(|v| *v)
            );
            for k in by_name.keys() {
                eprintln!("  {}", k);
            }
            assert!(by_name.len() >= 2, "expected ≥2 ESPHome devices");
            assert!(
                by_name.values().all(|v| *v),
                "expected every surfaced device to be recognized as bluetooth_proxy"
            );
        });
    }
}
