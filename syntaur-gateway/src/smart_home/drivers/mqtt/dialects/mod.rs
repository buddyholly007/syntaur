//! MQTT topic-schema dialect registry.
//!
//! Each supported smart-home MQTT flavor implements [`Dialect`]. The
//! [`DialectRouter`] fans an incoming `(topic, payload)` through every
//! registered dialect — first `Some(..)` wins. Dialects are stateless
//! and thread-safe; the MQTT supervisor (Phase C) keeps one router for
//! the lifetime of a broker connection.
//!
//! v1 dialects (in registration order — order matters because the
//! router short-circuits on the first match, so more-specific prefixes
//! should come before general fallbacks):
//!   - `ha_discovery`  — Home Assistant MQTT discovery (the lingua franca)
//!   - `tasmota`       — Tasmota discovery (SetOption19=0 mode)
//!   - `shelly_gen1`   — `shellies/<id>/announce` legacy stack
//!   - `esphome`       — ESPHome's optional MQTT discovery
//!
//! Phase B additions:
//!   - `shelly_gen2`       — RPC-over-MQTT, modern Plus/Pro/Gen3
//!   - `zigbee2mqtt`       — bridge/devices inventory + per-device state
//!   - `openmqttgateway`   — BLE/RF/IR bridge
//!
//! Phase C enriches [`DialectMessage`] with `State`, `Availability`,
//! and `BridgeEvent` variants so the long-running subscriber can drive
//! `SmartHomeEvent::DeviceStateChanged`. v1 covers `Discovery` only —
//! enough for the one-shot scan to populate `smart_home_devices`.

use serde_json::Value;

use super::command::{EncodedCommand, MqttCommand};
use crate::smart_home::scan::ScanCandidate;

pub mod esphome;
pub mod frigate;
pub mod ha_discovery;
pub mod ha_statestream;
pub mod openmqttgateway;
pub mod shelly_gen1;
pub mod shelly_gen2;
pub mod tasmota;
pub mod zigbee2mqtt;

/// Fresh state published by a dialect on the runtime path. The supervisor's
/// hash-diff layer (`state.rs`) consumes these and emits
/// `SmartHomeEvent::DeviceStateChanged` only on non-empty diffs. `source`
/// matches `metadata_json.schema` / `Dialect::id()` so subscribers can
/// filter ("z2m", "shelly_gen2", "tasmota", ...).
#[derive(Debug, Clone)]
pub struct DeviceStateUpdate {
    pub external_id: String,
    pub state: Value,
    pub source: String,
}

/// A dialect-specific message extracted from one MQTT frame.
///
/// Variants:
///   - `Discovery` — one candidate per frame (HA, Tasmota discovery,
///     Shelly, ESPHome).
///   - `Discoveries` — many candidates per frame (Z2M `bridge/devices`
///     publishes the whole inventory as one JSON array).
///   - `State` — Phase C runtime update (Tasmota tele/STATE, Z2M
///     per-device state, Shelly status, etc.) — flows into the
///     supervisor's state cache.
///   - `Availability` — LWT / presence signal. `online=false` marks
///     the device unreachable; subscribers (dashboards, automations)
///     react without a full state refresh.
///   - `BridgeEvent` — dialect-level control-plane signal that is NOT
///     a state change (z2m `bridge/event` join/leave, bridge/state
///     coordinator up/down). Opaque JSON preserved for diagnostics.
///
/// Kept non-exhaustive so matches remain forward-compatible — always
/// include a `_` fallback.
#[non_exhaustive]
pub enum DialectMessage {
    Discovery(ScanCandidate),
    Discoveries(Vec<ScanCandidate>),
    State(DeviceStateUpdate),
    Availability { external_id: String, online: bool },
    BridgeEvent(Value),
}

/// Parser surface for one smart-home MQTT dialect.
pub trait Dialect: Send + Sync {
    /// Stable short identifier — "ha", "tasmota", "shelly_gen1", "z2m", …
    /// Matches `smart_home_devices.metadata_json.schema` for devices
    /// ingested via this dialect.
    fn id(&self) -> &'static str;

    /// Topics the dialect wants subscribed. Returned as a static slice
    /// so the router can concatenate without allocation. Overlaps with
    /// other dialects' subscriptions are fine — brokers dedupe.
    fn subscribe_topics(&self) -> &'static [&'static str];

    /// Attempt to parse one frame. Dialects return `None` for topics
    /// outside their schema; the router tries each dialect in
    /// registration order and takes the first `Some`.
    fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage>;

    /// Encode an `MqttCommand` for one of this dialect's devices.
    ///
    /// `external_id` is the canonical `smart_home_devices.external_id`
    /// value (already includes the dialect-specific prefix — see each
    /// dialect's `parse` for the format). Returning `None` signals
    /// "this command is not applicable to this device" (wrong dialect,
    /// unsupported vocabulary for this device class, etc.) — the
    /// router tries the next dialect.
    ///
    /// Default implementation returns `None` so discovery-only
    /// dialects (ESPHome, OpenMQTTGateway, HA Discovery) don't need
    /// to implement anything. Phase D wires Tasmota, Z2M,
    /// Shelly Gen1, and Shelly Gen2 — see each dialect for the wire
    /// format.
    fn encode_command(
        &self,
        _external_id: &str,
        _cmd: &MqttCommand,
    ) -> Option<EncodedCommand> {
        None
    }
}

/// Ordered collection of dialects.
pub struct DialectRouter {
    dialects: Vec<Box<dyn Dialect>>,
}

impl DialectRouter {
    /// Default router with all v1 dialects registered in priority
    /// order. More-specific dialects (Tasmota, Shelly Gen1) come before
    /// the HA-Discovery fallback so a device publishing both its
    /// native topics AND HA-Discovery is surfaced once, by its native
    /// dialect — the scan pipeline's dedupe layer catches any
    /// remaining duplicates by `external_id`.
    pub fn v1() -> Self {
        let dialects: Vec<Box<dyn Dialect>> = vec![
            Box::new(tasmota::Tasmota),
            Box::new(shelly_gen1::ShellyGen1),
            Box::new(shelly_gen2::ShellyGen2),
            Box::new(esphome::EspHome),
            Box::new(zigbee2mqtt::Zigbee2Mqtt),
            Box::new(openmqttgateway::OpenMqttGateway),
            Box::new(frigate::Frigate),
            Box::new(ha_statestream::HaStatestream),
            Box::new(ha_discovery::HaDiscovery),
        ];
        Self { dialects }
    }

    /// Union of every dialect's `subscribe_topics()` in registration
    /// order. Caller forwards each to the broker client.
    pub fn subscribe_topics(&self) -> Vec<&'static str> {
        self.dialects
            .iter()
            .flat_map(|d| d.subscribe_topics().iter().copied())
            .collect()
    }

    /// Parse one frame — first matching dialect wins.
    pub fn parse(&self, topic: &str, payload: &[u8]) -> Option<DialectMessage> {
        for d in &self.dialects {
            if let Some(m) = d.parse(topic, payload) {
                return Some(m);
            }
        }
        None
    }

    /// Encode one command — first dialect that claims the device wins.
    /// Dispatch layer hands back `None` if no dialect matches (the
    /// device's `external_id` prefix isn't recognized, or the dialect
    /// doesn't support the requested command).
    pub fn encode_command(
        &self,
        external_id: &str,
        cmd: &MqttCommand,
    ) -> Option<EncodedCommand> {
        for d in &self.dialects {
            if let Some(e) = d.encode_command(external_id, cmd) {
                return Some(e);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_router_subscribes_to_all_dialect_topics() {
        let r = DialectRouter::v1();
        let topics = r.subscribe_topics();
        // One sample from each dialect.
        assert!(topics.iter().any(|t| t.starts_with("homeassistant/")));
        assert!(topics.iter().any(|t| t.starts_with("tasmota/")));
        assert!(topics.iter().any(|t| t.starts_with("shellies/")));
        assert!(topics.iter().any(|t| t.starts_with("esphome/")));
        assert!(topics.iter().any(|t| t.starts_with("zigbee2mqtt/")));
        assert!(topics.iter().any(|t| t.starts_with("shellyplus")));
        assert!(topics.iter().any(|t| t.starts_with("home/+/BTtoMQTT/")));
        assert!(topics.iter().any(|t| t.starts_with("frigate/")));
    }

    #[test]
    fn v1_router_returns_none_for_unknown_topic() {
        let r = DialectRouter::v1();
        assert!(r.parse("whatever/stuff", b"{}").is_none());
    }
}
