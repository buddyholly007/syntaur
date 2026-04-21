//! Shelly Gen2+ dialect — Plus / Pro / Gen3 devices.
//!
//! Shelly's second-generation firmware speaks JSON-RPC over MQTT:
//!   - `shellyplus<id>/online`                      — retained `true`/`false` presence
//!   - `shellyplus<id>/status/<component>:<idx>`    — retained JSON state (per component)
//!   - `shellyplus<id>/rpc`                         — bidirectional JSON-RPC
//!   - `shellyplus<id>/events/rpc`                  — RPC notifications (non-retained)
//!
//! v1 parses just the `online` presence: when a Gen2 device connects,
//! it publishes `true` retained to `<device_id>/online`. That's enough
//! to surface the device as a `ScanCandidate`. Kind is inferred from
//! the model prefix embedded in the device id (`shellyplus1pm-...` →
//! relay w/ power meter). Users rename in the confirm-scan step.
//!
//! Phase C adds the full story: per-component state parsing from
//! `status/<component>:<idx>` topics drives `DeviceStateChanged`
//! events, RPC notifications flow through as `BridgeEvent`s, and
//! availability flips on `online=false` become `Availability` events.
//! Phase D's control path publishes RPC `Switch.Set` / `Light.Set` /
//! `Cover.GoToPosition` commands to `<device_id>/rpc`.
//!
//! Spec: https://shelly-api-docs.shelly.cloud/gen2/ComponentsAndServices/Mqtt/
//! Device catalog: https://shelly-api-docs.shelly.cloud/gen2/Devices/

use super::{Dialect, DialectMessage};
use crate::smart_home::scan::ScanCandidate;

pub struct ShellyGen2;

impl Dialect for ShellyGen2 {
    fn id(&self) -> &'static str {
        "shelly_gen2"
    }

    fn subscribe_topics(&self) -> &'static [&'static str] {
        &[
            "shellyplus+/online",
            // Phase C adds shellyplus+/status/+, shellyplus+/events/rpc
            // so long-running subscribers see component state +
            // notifications without a re-subscribe cost.
        ]
    }

    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        if !topic.starts_with("shellyplus") || !topic.ends_with("/online") {
            return None;
        }
        let Ok(payload_s) = std::str::from_utf8(payload) else {
            return None;
        };
        // Payload is the bare string "true" or "false". Only "true"
        // (device present) counts as discovery — an "offline" LWT frame
        // shouldn't create a phantom device. Phase C's Availability
        // variant will route the "false" side through to presence.
        if payload_s.trim() != "true" {
            return None;
        }
        parse_online(topic).map(DialectMessage::Discovery)
    }
}

/// Build a `ScanCandidate` from a `shellyplus<id>/online` topic.
pub fn parse_online(topic: &str) -> Option<ScanCandidate> {
    let device_id = topic.strip_suffix("/online")?;
    let hyphen = device_id.find('-')?;
    let model = &device_id[..hyphen];
    let mac_segment = &device_id[hyphen + 1..];

    let kind = infer_kind(model).to_string();
    let mac = if mac_segment.len() >= 12 {
        Some(mac_segment[..12].to_ascii_uppercase())
    } else {
        None
    };

    Some(ScanCandidate {
        driver: "mqtt".into(),
        external_id: format!("shelly_gen2:{}", device_id),
        name: device_id.to_string(),
        kind,
        vendor: Some("Shelly".into()),
        ip: None,
        mac,
        details: serde_json::json!({
            "source": "mqtt",
            "schema": "shelly_gen2",
            "topic": topic,
            "model_prefix": model,
        }),
    })
}

/// Map Shelly Gen2+ model prefix → smart_home_devices.kind.
///
/// Prefixes are ordered longest-first so `shellyplus1pm` wins before
/// `shellyplus1`. Unknown models default to "switch" — the most
/// conservative shape (on/off capability exposed, user can retype).
fn infer_kind(model: &str) -> &'static str {
    const MODEL_KIND_PREFIXES: &[(&str, &str)] = &[
        ("shellyplusht", "sensor_climate"),
        ("shellyplustrv", "thermostat"),
        ("shellyplusplugus", "plug"),
        ("shellyplusplugs", "plug"),
        ("shellyplusrgbw", "light"),
        ("shellypluswalldisplay", "thermostat"),
        ("shellypluswallswitch", "switch"),
        ("shellyplusi4", "switch"),
        ("shellyplus1pm", "switch"),
        ("shellyplus2pm", "switch"),
        ("shellyplus1l", "light"),
        ("shellyplus2l", "light"),
        ("shellyplus1", "switch"),
        ("shellyplus2", "switch"),
        ("shellypluspmmini", "switch"),
        ("shellyplus", "switch"),
    ];
    for (prefix, kind) in MODEL_KIND_PREFIXES {
        if model.starts_with(prefix) {
            return kind;
        }
    }
    "switch"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_online_true_builds_candidate() {
        let d = ShellyGen2;
        let m = d
            .parse("shellyplus1pm-aabbccddeeff/online", b"true")
            .expect("some");
        match m {
            DialectMessage::Discovery(c) => {
                assert_eq!(c.vendor.as_deref(), Some("Shelly"));
                assert_eq!(c.kind, "switch");
                assert_eq!(c.external_id, "shelly_gen2:shellyplus1pm-aabbccddeeff");
                assert_eq!(c.mac.as_deref(), Some("AABBCCDDEEFF"));
            }
            other => panic!("expected Discovery, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn parse_online_false_returns_none() {
        let d = ShellyGen2;
        assert!(d
            .parse("shellyplus1-aabbccddeeff/online", b"false")
            .is_none());
    }

    #[test]
    fn dialect_ignores_gen1_topic() {
        let d = ShellyGen2;
        assert!(d.parse("shellies/shellyplug-s-aa/announce", b"{}").is_none());
    }

    #[test]
    fn infer_kind_ht_sensor() {
        assert_eq!(infer_kind("shellyplusht"), "sensor_climate");
    }

    #[test]
    fn infer_kind_trv_is_thermostat() {
        assert_eq!(infer_kind("shellyplustrv"), "thermostat");
    }

    #[test]
    fn infer_kind_plug_variants() {
        assert_eq!(infer_kind("shellyplusplugs"), "plug");
        assert_eq!(infer_kind("shellyplusplugus"), "plug");
    }

    #[test]
    fn infer_kind_dimmer_1l_is_light() {
        assert_eq!(infer_kind("shellyplus1l"), "light");
    }

    #[test]
    fn infer_kind_rgbw_is_light() {
        assert_eq!(infer_kind("shellyplusrgbw"), "light");
    }

    #[test]
    fn infer_kind_1pm_wins_over_1() {
        // Prefix ordering — longest-first inside MODEL_KIND_PREFIXES.
        // `shellyplus1pm` must not fall through to the generic
        // `shellyplus1` arm and back to switch only because of that.
        assert_eq!(infer_kind("shellyplus1pm"), "switch");
        assert_eq!(infer_kind("shellyplus1"), "switch");
    }

    #[test]
    fn infer_kind_unknown_model_defaults_to_switch() {
        assert_eq!(infer_kind("shellyplusfuturemodel"), "switch");
    }

    #[test]
    fn parse_online_malformed_device_id_returns_none() {
        // No hyphen separator → can't split model from MAC.
        assert!(parse_online("shellyplus_no_hyphen/online").is_none());
    }
}
