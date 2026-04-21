//! Frigate NVR dialect — reads detector + recording toggle state per
//! camera, surfaces each camera as a `ScanCandidate`, and lets the
//! automation engine flip motion/detect/recordings/etc. through the
//! usual control path.
//!
//! This dialect was built from a live capture of Sean's HA Mosquitto
//! (5 cameras × ~16 retained toggle topics each, April 2026). Frigate
//! publishes every toggle it exposes through the web UI under
//! `frigate/<camera>/<feature>/state` (retained) with a plain `ON` /
//! `OFF` payload for booleans and integers / enum strings for the
//! rest. Flipping them back is as simple as publishing to
//! `frigate/<camera>/<feature>/set`.
//!
//! Topics this dialect understands:
//!   Retained state (one per camera per feature):
//!     frigate/<cam>/enabled/state              ON/OFF
//!     frigate/<cam>/detect/state               ON/OFF
//!     frigate/<cam>/motion/state               ON/OFF
//!     frigate/<cam>/recordings/state           ON/OFF
//!     frigate/<cam>/snapshots/state            ON/OFF
//!     frigate/<cam>/audio/state                ON/OFF
//!     frigate/<cam>/ptz_autotracker/state      ON/OFF
//!     frigate/<cam>/birdseye/state             ON/OFF
//!     frigate/<cam>/improve_contrast/state     ON/OFF
//!     frigate/<cam>/review_alerts/state        ON/OFF
//!     frigate/<cam>/review_detections/state    ON/OFF
//!     frigate/<cam>/object_descriptions/state  ON/OFF
//!     frigate/<cam>/review_descriptions/state  ON/OFF
//!     frigate/<cam>/motion_threshold/state     integer
//!     frigate/<cam>/motion_contour_area/state  integer
//!     frigate/<cam>/birdseye_mode/state        CONTINUOUS / MOTION / OBJECTS
//!     frigate/<cam>/<class>/snapshot           binary image (not parsed)
//!
//!   Live (non-retained):
//!     frigate/available                        online/offline — broker-level LWT
//!     frigate/<cam>/status/detect              PT events
//!     frigate/<cam>/status/record              PT events
//!     frigate/events                           JSON event stream
//!
//! `external_id` shape: `frigate_camera:<cam>`. Two tasmota-style
//! namespaces here would be redundant — Frigate's cameras are always
//! known by their topic-path name.
//!
//! Control path: `encode_command(external_id, MqttCommand::SetOn(b))`
//! currently toggles the `detect` feature by default. Fine-grained
//! control (which feature to flip) is a richer MqttCommand variant for
//! Phase D+1; the common automation case is "detect off when I'm home,
//! back on when I leave" which maps cleanly to SetOn on detect.

use serde_json::Value;

use super::{DeviceStateUpdate, Dialect, DialectMessage};
use crate::smart_home::drivers::mqtt::command::{EncodedCommand, MqttCommand};
use crate::smart_home::scan::ScanCandidate;

pub struct Frigate;

const STATE_TOPICS: &[&str] = &[
    "frigate/+/enabled/state",
    "frigate/+/detect/state",
    "frigate/+/motion/state",
    "frigate/+/recordings/state",
    "frigate/+/snapshots/state",
    "frigate/+/audio/state",
    "frigate/+/ptz_autotracker/state",
    "frigate/+/birdseye/state",
    "frigate/+/birdseye_mode/state",
    "frigate/+/improve_contrast/state",
    "frigate/+/review_alerts/state",
    "frigate/+/review_detections/state",
    "frigate/+/object_descriptions/state",
    "frigate/+/review_descriptions/state",
    "frigate/+/motion_threshold/state",
    "frigate/+/motion_contour_area/state",
    "frigate/available",
];

/// Features we turn into booleans when parsing. Everything else falls
/// through as the raw payload string (number for thresholds, enum for
/// birdseye_mode).
const BOOL_FEATURES: &[&str] = &[
    "enabled",
    "detect",
    "motion",
    "recordings",
    "snapshots",
    "audio",
    "ptz_autotracker",
    "birdseye",
    "improve_contrast",
    "review_alerts",
    "review_detections",
    "object_descriptions",
    "review_descriptions",
];

impl Dialect for Frigate {
    fn id(&self) -> &'static str {
        "frigate"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        STATE_TOPICS
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if topic == "frigate/available" {
            let s = std::str::from_utf8(payload).ok()?.trim();
            // Frigate emits "online"/"offline" (lowercase). No per-camera
            // availability frame — every camera inherits the same flag.
            let online = match s {
                "online" | "Online" | "ON" | "on" => true,
                "offline" | "Offline" | "OFF" | "off" => false,
                _ => return None,
            };
            // No camera scope — surface as a special external_id so the
            // state cache can still distinguish. Subscribers that want
            // to mark every individual camera offline pivot on this.
            return Some(DialectMessage::Availability {
                external_id: "frigate_server:default".into(),
                online,
            });
        }

        let (camera, feature) = parse_state_topic(topic)?;
        let s = std::str::from_utf8(payload).ok()?.trim();

        // Discovery: every unique camera we haven't surfaced yet
        // becomes a ScanCandidate. The router would route the frame
        // through to a State update too — the caller's dedupe layer
        // handles the doubling. To keep the contract simple we emit
        // State only here; the first scan through scan_with_options
        // uses the cache's running discovery fan-out to surface
        // candidates, and we provide a `discovery_from_state` helper
        // for callers that want to eagerly surface.
        let value: Value = if BOOL_FEATURES.contains(&feature) {
            match s {
                "ON" | "on" | "true" | "1" => Value::Bool(true),
                "OFF" | "off" | "false" | "0" => Value::Bool(false),
                _ => return None,
            }
        } else if let Ok(n) = s.parse::<i64>() {
            // Numeric features (motion_threshold, motion_contour_area)
            // are integers in practice — keep them integers so JSON
            // comparisons against int literals line up.
            Value::Number(n.into())
        } else if let Ok(n) = s.parse::<f64>() {
            serde_json::Number::from_f64(n)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(s.into()))
        } else {
            // Enum string (birdseye_mode) — pass through.
            Value::String(s.into())
        };

        let state_obj = serde_json::json!({ feature: value });
        Some(DialectMessage::State(DeviceStateUpdate {
            external_id: format!("frigate_camera:{}", camera),
            state: state_obj,
            source: "frigate".into(),
        }))
    }

    fn encode_command(
        &self,
        external_id: &str,
        cmd: &MqttCommand,
    ) -> Option<EncodedCommand> {
        let camera = external_id.strip_prefix("frigate_camera:")?;
        match cmd {
            MqttCommand::SetOn(on) => {
                // Default toggle target is `detect`. Automations
                // commonly want "pause detection when someone is home"
                // which maps directly. Fine-grained control (specific
                // feature) uses `Raw`.
                Some(EncodedCommand::new(
                    format!("frigate/{}/detect/set", camera),
                    if *on { b"ON".to_vec() } else { b"OFF".to_vec() },
                ))
            }
            MqttCommand::Raw(v) => {
                // Raw: object with {"feature": "<name>", "value": "<str>"}
                let feature = v.get("feature").and_then(|x| x.as_str())?;
                let value = v.get("value").and_then(|x| x.as_str())
                    .map(str::to_string)
                    .or_else(|| v.get("value").map(|x| x.to_string()))?;
                Some(EncodedCommand::new(
                    format!("frigate/{}/{}/set", camera, feature),
                    value.into_bytes(),
                ))
            }
            _ => None,
        }
    }
}

/// Split `frigate/<cam>/<feature>/state` into (camera, feature).
/// Returns `None` for non-state topics or snapshot/image topics.
fn parse_state_topic(topic: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = topic.split('/').collect();
    // Expected: ["frigate", cam, feature, "state"]
    if parts.len() != 4 {
        return None;
    }
    if parts[0] != "frigate" || parts[3] != "state" {
        return None;
    }
    Some((parts[1], parts[2]))
}

/// Convenience: build a `ScanCandidate` for a camera by name — useful
/// if the session wants to eagerly surface devices without waiting
/// for the dialect's State→scan reconciliation.
pub fn camera_candidate(camera: &str) -> ScanCandidate {
    ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("frigate_camera:{}", camera),
        name: format!("Frigate {}", camera),
        kind: "camera".into(),
        vendor: Some("Frigate".into()),
        ip: None,
        mac: None,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "frigate",
            "camera": camera,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_boolean_detect_state() {
        let d = Frigate;
        let msg = d.parse("frigate/doorbell/detect/state", b"ON").expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "frigate_camera:doorbell");
                assert_eq!(u.source, "frigate");
                assert_eq!(u.state["detect"], true);
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parses_numeric_motion_threshold() {
        let d = Frigate;
        let msg = d
            .parse("frigate/driveway/motion_threshold/state", b"30")
            .expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.external_id, "frigate_camera:driveway");
                assert_eq!(u.state["motion_threshold"], 30);
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn parses_enum_birdseye_mode() {
        let d = Frigate;
        let msg = d
            .parse("frigate/garage/birdseye_mode/state", b"OBJECTS")
            .expect("some");
        match msg {
            DialectMessage::State(u) => {
                assert_eq!(u.state["birdseye_mode"], "OBJECTS");
            }
            _ => panic!("expected State"),
        }
    }

    #[test]
    fn available_topic_emits_availability() {
        let d = Frigate;
        let m1 = d.parse("frigate/available", b"online").expect("some");
        match m1 {
            DialectMessage::Availability { online, external_id } => {
                assert!(online);
                assert_eq!(external_id, "frigate_server:default");
            }
            _ => panic!("expected Availability"),
        }
        let m2 = d.parse("frigate/available", b"offline").expect("some");
        match m2 {
            DialectMessage::Availability { online, .. } => assert!(!online),
            _ => panic!("expected Availability"),
        }
    }

    #[test]
    fn ignores_snapshot_image_topics() {
        let d = Frigate;
        assert!(d.parse("frigate/doorbell/person/snapshot", b"\x89PNG...").is_none());
    }

    #[test]
    fn ignores_status_event_topics_for_now() {
        let d = Frigate;
        // `frigate/+/status/+` has 4 parts but parts[3] != "state"
        assert!(d.parse("frigate/garage/status/detect", b"PT").is_none());
    }

    #[test]
    fn encode_set_on_toggles_detect() {
        let d = Frigate;
        let e = d
            .encode_command("frigate_camera:doorbell", &MqttCommand::SetOn(false))
            .expect("some");
        assert_eq!(e.topic, "frigate/doorbell/detect/set");
        assert_eq!(&e.payload, b"OFF");
    }

    #[test]
    fn encode_raw_feature_level() {
        let d = Frigate;
        let e = d
            .encode_command(
                "frigate_camera:garage",
                &MqttCommand::Raw(serde_json::json!({
                    "feature": "recordings",
                    "value": "ON"
                })),
            )
            .expect("some");
        assert_eq!(e.topic, "frigate/garage/recordings/set");
        assert_eq!(&e.payload, b"ON");
    }

    #[test]
    fn encode_ignores_foreign_external_id() {
        let d = Frigate;
        assert!(d
            .encode_command("z2m:0x00", &MqttCommand::SetOn(true))
            .is_none());
    }

    #[test]
    fn camera_candidate_shape() {
        let c = camera_candidate("doorbell");
        assert_eq!(c.external_id, "frigate_camera:doorbell");
        assert_eq!(c.kind, "camera");
        assert_eq!(c.vendor.as_deref(), Some("Frigate"));
    }

    /// Opt-in live-broker validation. Runs only when
    /// `SYNTAUR_LIVE_MQTT_URL` is set (e.g.
    /// `mqtt://frigate:frigate2026@192.168.1.3:1883`). Otherwise
    /// returns early so default `cargo test` stays hermetic.
    ///
    /// Connects with rumqttc, subscribes to the dialect's topic set,
    /// collects retained frames for 5 seconds, then asserts:
    ///   - every retained state frame parses successfully via
    ///     `Frigate.parse`
    ///   - at least one camera surfaces through
    ///     `camera_candidate`-compatible external_id shape
    #[test]
    fn live_ha_broker_frigate_retains_parse_cleanly() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::collections::HashSet;
        use std::time::Duration;
        use tokio::time::timeout;

        let Ok(url) = std::env::var("SYNTAUR_LIVE_MQTT_URL") else {
            eprintln!("SYNTAUR_LIVE_MQTT_URL not set — skipping live test");
            return;
        };
        let parsed = url::Url::parse(&url).expect("parse SYNTAUR_LIVE_MQTT_URL");
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap_or(1883);
        let user = parsed.username().to_string();
        let pass = parsed.password().unwrap_or("").to_string();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut opts = MqttOptions::new("syntaur-frigate-live", host, port);
            opts.set_keep_alive(Duration::from_secs(30));
            if !user.is_empty() {
                opts.set_credentials(user, pass);
            }
            let (client, mut event_loop) = AsyncClient::new(opts, 256);
            for t in STATE_TOPICS {
                client.subscribe(*t, QoS::AtMostOnce).await.ok();
            }

            let dialect = Frigate;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
            let mut cameras: HashSet<String> = HashSet::new();
            let mut state_frames = 0usize;
            let mut avail_frames = 0usize;
            let mut parse_failures = Vec::<String>::new();

            while tokio::time::Instant::now() < deadline {
                match timeout(Duration::from_millis(400), event_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        match dialect.parse(&p.topic, &p.payload) {
                            Some(DialectMessage::State(u)) => {
                                state_frames += 1;
                                if let Some(cam) =
                                    u.external_id.strip_prefix("frigate_camera:")
                                {
                                    cameras.insert(cam.to_string());
                                }
                            }
                            Some(DialectMessage::Availability { .. }) => {
                                avail_frames += 1;
                            }
                            Some(_) | None => {
                                // None on state-typed topics is a real failure
                                if p.topic.ends_with("/state") {
                                    parse_failures.push(format!(
                                        "{} => {}",
                                        p.topic,
                                        String::from_utf8_lossy(&p.payload)
                                    ));
                                }
                            }
                        }
                    }
                    _ => continue,
                }
            }

            eprintln!(
                "[frigate-live] cameras={} state_frames={} avail_frames={} parse_failures={}",
                cameras.len(),
                state_frames,
                avail_frames,
                parse_failures.len()
            );
            for f in parse_failures.iter().take(10) {
                eprintln!("  FAIL: {}", f);
            }
            assert!(
                parse_failures.is_empty(),
                "{} frames on /state topics did not parse — see stderr",
                parse_failures.len()
            );
            assert!(
                !cameras.is_empty(),
                "expected at least one camera to surface"
            );
            assert!(state_frames > 0, "expected retained state frames");
        });
    }
}
