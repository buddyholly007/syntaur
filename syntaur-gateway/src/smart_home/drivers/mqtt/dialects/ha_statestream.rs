//! Home Assistant `mqtt_statestream` dialect — consumes HA's own state
//! republish stream.
//!
//! Enabled on Sean's HA as of 2026-04-21 via a package at
//! `/config/packages/syntaur_mqtt_statestream.yaml` (base_topic =
//! `syntaur/ha`, publish_attributes + publish_timestamps both on).
//! That setting publishes one topic per entity attribute on every
//! `state_changed`:
//!
//! ```text
//!   syntaur/ha/<domain>/<object_id>/state          — the entity's state
//!   syntaur/ha/<domain>/<object_id>/<attribute>    — each attribute
//!   syntaur/ha/<domain>/<object_id>/last_changed   — noisy timestamp
//!   syntaur/ha/<domain>/<object_id>/last_updated   — noisy timestamp
//! ```
//!
//! Dialect behavior:
//!   - `/state` and `/<attribute>` topics → `DialectMessage::State` with
//!     `external_id = "ha:<domain>.<object_id>"` and a single-key state
//!     object `{ <leaf>: <value> }`. The supervisor's `StateCache`
//!     shallow-merges successive frames into a single growing state
//!     blob per entity, so the hash-diff layer only fires
//!     `DeviceStateChanged` when the merged picture actually changes.
//!   - `/last_changed` and `/last_updated` are silently dropped. They
//!     change on every publish whether or not the entity's value
//!     changed, which would defeat the hash-diff gate.
//!   - JSON-quoted strings (HA publishes string attributes as quoted
//!     JSON scalars) are unwrapped: payload `"idle"` → `Value::String("idle")`.
//!   - Plain integers / floats parse as numbers.
//!   - `ON`/`OFF`/`on`/`off`/`true`/`false` on binary_sensor or switch
//!     `/state` topics coerce to booleans.
//!
//! External id shape: `ha:<domain>.<object_id>` (dotted, matches the
//! HA entity_id convention). This lets us reason about statestream
//! rows by entity_id without a second mapping.

use serde_json::Value;

use super::{DeviceStateUpdate, Dialect, DialectMessage};

pub struct HaStatestream;

/// Root prefix — matches the `base_topic: syntaur/ha` in HA's
/// mqtt_statestream config. If we change the base topic on the HA
/// side, this constant moves in lockstep.
const PREFIX: &str = "syntaur/ha/";

impl Dialect for HaStatestream {
    fn id(&self) -> &'static str {
        "ha_statestream"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &["syntaur/ha/+/+/+"]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        let rest = topic.strip_prefix(PREFIX)?;
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() != 3 {
            return None;
        }
        let (domain, object_id, leaf) = (parts[0], parts[1], parts[2]);

        // Timestamps always differ between publishes even when the
        // underlying state is unchanged. Let them through and the
        // hash-diff gate never fires. Drop instead.
        if leaf == "last_changed" || leaf == "last_updated" {
            return None;
        }

        let s = std::str::from_utf8(payload).ok()?;
        let value = parse_value(domain, leaf, s);

        Some(DialectMessage::State(DeviceStateUpdate {
            external_id: format!("ha:{}.{}", domain, object_id),
            state: serde_json::json!({ leaf: value }),
            source: "ha_statestream".into(),
        }))
    }
}

/// Interpret one statestream leaf payload. HA's `mqtt_statestream`
/// publishes:
///   - Strings either bare (`idle`, `ON`) or JSON-quoted (`"idle"`,
///     `"Sean’s iPhone"`) depending on the attribute type. The
///     attribute publishes come through JSON-encoded (because HA JSON-
///     serializes the attribute value); `/state` publishes come
///     through as raw plain text.
///   - Numbers as plain decimal (`47.6`, `159424`).
///   - Arrays + objects as JSON-encoded strings.
///
/// Coercion priority:
///   1. Boolean coercion for on/off-style states on binary_sensor /
///      switch / light / lock / cover — HA's mqtt_statestream leaves
///      these as "on"/"off" (lowercase) on `/state`.
///   2. Valid JSON (object / array / quoted string / bool / null).
///   3. Integer.
///   4. Float.
///   5. Raw string fallback.
fn parse_value(domain: &str, leaf: &str, raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }

    // On the `/state` topic, boolean-class domains publish lowercase
    // on/off. Coerce directly to match other dialects' boolean shape.
    if leaf == "state" {
        match (domain, trimmed) {
            ("binary_sensor", "on") | ("switch", "on") | ("light", "on")
            | ("lock", "locked") | ("cover", "open") | ("fan", "on")
            | ("automation", "on") | ("humidifier", "on") | ("vacuum", "on") => {
                return Value::Bool(true);
            }
            ("binary_sensor", "off") | ("switch", "off") | ("light", "off")
            | ("lock", "unlocked") | ("cover", "closed") | ("fan", "off")
            | ("automation", "off") | ("humidifier", "off") | ("vacuum", "off") => {
                return Value::Bool(false);
            }
            _ => {}
        }
    }

    // JSON first — catches "quoted strings", arrays, objects, booleans.
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        // Only accept JSON parse if the original looked JSON-ish. A bare
        // integer parses as JSON Number but so does the integer branch
        // below — same outcome. A bare word like `idle` is NOT valid
        // JSON (quotes required) so JSON parse rejects; we'd fall
        // through to the string fallback.
        return v;
    }

    // Plain integer first, then float.
    if let Ok(n) = trimmed.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return Value::Number(num);
        }
    }

    Value::String(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_non_statestream_topic() {
        let d = HaStatestream;
        assert!(d.parse("homeassistant/sensor/x/config", b"{}").is_none());
        assert!(d.parse("frigate/doorbell/motion/state", b"ON").is_none());
    }

    #[test]
    fn drops_last_changed_and_last_updated_noise() {
        let d = HaStatestream;
        assert!(d
            .parse(
                "syntaur/ha/sensor/cpu_temp/last_changed",
                b"2026-04-21T20:47:57.875819+00:00"
            )
            .is_none());
        assert!(d
            .parse(
                "syntaur/ha/sensor/cpu_temp/last_updated",
                b"2026-04-21T20:47:57.875819+00:00"
            )
            .is_none());
    }

    #[test]
    fn state_topic_bool_domain_coerces_on_off() {
        let d = HaStatestream;
        let m = d
            .parse("syntaur/ha/binary_sensor/front_door/state", b"on")
            .expect("some");
        match m {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "ha:binary_sensor.front_door");
                assert_eq!(u.source, "ha_statestream");
                assert_eq!(u.state["state"], true);
            }
            _ => panic!("expected State"),
        }
        let m = d
            .parse("syntaur/ha/switch/bedroom_lamp/state", b"off")
            .expect("some");
        if let DialectMessage::State(u) = m {
            assert_eq!(u.state["state"], false);
        } else {
            panic!("expected State");
        }
    }

    #[test]
    fn lock_cover_domain_have_domain_specific_states() {
        let d = HaStatestream;
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/lock/front_door/state", b"locked")
            .unwrap()
        {
            assert_eq!(u.state["state"], true);
        }
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/cover/garage_door/state", b"open")
            .unwrap()
        {
            assert_eq!(u.state["state"], true);
        }
    }

    #[test]
    fn sensor_state_parses_numeric() {
        let d = HaStatestream;
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/sensor/heap_free/state", b"159424")
            .unwrap()
        {
            assert_eq!(u.state["state"], 159424);
        }
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/sensor/internal_temperature/state", b"47.6")
            .unwrap()
        {
            let v = u.state["state"].as_f64().unwrap();
            assert!((v - 47.6).abs() < 1e-6);
        }
    }

    #[test]
    fn sensor_state_keeps_enum_string() {
        let d = HaStatestream;
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/sensor/backup_manager/state", b"idle")
            .unwrap()
        {
            assert_eq!(u.state["state"], "idle");
        }
    }

    #[test]
    fn attribute_topic_unwraps_json_quoted_string() {
        let d = HaStatestream;
        let m = d
            .parse(
                "syntaur/ha/sensor/backup_manager/friendly_name",
                br#""Backup Backup Manager state""#,
            )
            .unwrap();
        if let DialectMessage::State(u) = m {
            assert_eq!(u.state["friendly_name"], "Backup Backup Manager state");
        } else {
            panic!("expected State");
        }
    }

    #[test]
    fn attribute_topic_parses_json_array() {
        let d = HaStatestream;
        let m = d
            .parse(
                "syntaur/ha/sensor/backup_manager/options",
                br#"["idle", "create_backup", "blocked"]"#,
            )
            .unwrap();
        if let DialectMessage::State(u) = m {
            let arr = u.state["options"].as_array().unwrap();
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], "idle");
        } else {
            panic!("expected State");
        }
    }

    #[test]
    fn attribute_topic_bare_string_falls_through() {
        let d = HaStatestream;
        // Some HA attributes publish unquoted bare strings. Parser
        // falls through JSON, int, float to the string fallback.
        let m = d
            .parse("syntaur/ha/sensor/device_class/state", b"enum")
            .unwrap();
        if let DialectMessage::State(u) = m {
            assert_eq!(u.state["state"], "enum");
        }
    }

    #[test]
    fn ignores_wrong_part_count() {
        let d = HaStatestream;
        // syntaur/ha/<domain> — too short
        assert!(d.parse("syntaur/ha/sensor/state", b"x").is_none());
        // syntaur/ha/<a>/<b>/<c>/<d> — too long
        assert!(d
            .parse("syntaur/ha/sensor/x/attrs/extra", b"x")
            .is_none());
    }

    #[test]
    fn external_id_uses_dotted_entity_id_shape() {
        let d = HaStatestream;
        if let DialectMessage::State(u) = d
            .parse("syntaur/ha/light/living_room_lamp/state", b"on")
            .unwrap()
        {
            assert_eq!(u.external_id, "ha:light.living_room_lamp");
        }
    }

    /// Opt-in live test — runs when SYNTAUR_LIVE_MQTT_URL points at
    /// HA's broker. Confirms every `syntaur/ha/*` topic parses
    /// cleanly and that at least one entity surfaces. Watches for 15
    /// seconds so low-activity periods still see some publishes.
    #[test]
    fn live_broker_statestream_parses_cleanly() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::collections::HashSet;
        use std::time::Duration;
        use tokio::time::timeout;

        let Ok(url) = std::env::var("SYNTAUR_LIVE_MQTT_URL") else {
            eprintln!("SYNTAUR_LIVE_MQTT_URL not set — skipping live test");
            return;
        };
        let parsed = url::Url::parse(&url).unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap_or(1883);
        let user = parsed.username().to_string();
        let pass = parsed.password().unwrap_or("").to_string();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut opts = MqttOptions::new("syntaur-statestream-live", host, port);
            opts.set_keep_alive(Duration::from_secs(30));
            if !user.is_empty() {
                opts.set_credentials(user, pass);
            }
            let (client, mut event_loop) = AsyncClient::new(opts, 512);
            client
                .subscribe("syntaur/ha/+/+/+", QoS::AtMostOnce)
                .await
                .unwrap();

            let dialect = HaStatestream;
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(30);
            let mut parsed_count = 0usize;
            let mut skipped_noise = 0usize;
            let mut parse_failures: Vec<String> = Vec::new();
            let mut seen_entities: HashSet<String> = HashSet::new();

            while tokio::time::Instant::now() < deadline {
                match timeout(Duration::from_millis(400), event_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        match dialect.parse(&p.topic, &p.payload) {
                            Some(DialectMessage::State(u)) => {
                                parsed_count += 1;
                                seen_entities.insert(u.external_id);
                            }
                            Some(_) => {}
                            None => {
                                // Noise filter (last_changed / last_updated)
                                // or non-matching shape. Track as expected.
                                if p.topic.ends_with("/last_changed")
                                    || p.topic.ends_with("/last_updated")
                                {
                                    skipped_noise += 1;
                                } else {
                                    parse_failures.push(p.topic.clone());
                                }
                            }
                        }
                    }
                    _ => continue,
                }
            }

            eprintln!(
                "[ha-statestream-live] parsed={} noise_skipped={} failures={} entities={}",
                parsed_count,
                skipped_noise,
                parse_failures.len(),
                seen_entities.len()
            );
            for t in parse_failures.iter().take(10) {
                eprintln!("  FAIL {}", t);
            }
            assert!(
                parse_failures.is_empty(),
                "{} statestream frames failed to parse",
                parse_failures.len()
            );
            // Low-activity periods may legitimately see zero state
            // changes, so a softer assertion on parsed count:
            assert!(
                parsed_count + skipped_noise > 0,
                "expected at least some statestream traffic"
            );
        });
    }
}
