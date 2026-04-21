//! Home Assistant MQTT-Discovery publisher — advertises Syntaur-owned
//! devices under `homeassistant/<component>/syntaur/<device_id>/config`
//! so HA (and anything else that speaks the HA Discovery dialect, like
//! openHAB or Node-RED) auto-surfaces them without manual config.
//!
//! v1 scope (Phase F-1):
//!   - On start, walk `smart_home_devices` and publish a retained
//!     config frame for every commissioned device. Kind → HA component
//!     is mapped conservatively; unknown kinds fall back to
//!     `binary_sensor` as a generic surface.
//!   - Subscribe to `SmartHomeEvent::DeviceStateChanged`; on every
//!     emission, republish the device's state to its `state_topic`.
//!   - Publishes flow through `MqttSupervisor::publish_retained` so
//!     whichever broker the session holds (embedded :1884 or the
//!     user's upstream Mosquitto) receives the configs. No extra
//!     rumqttc client.
//!
//! Out of scope for v1:
//!   - Command-topic round-trip. We don't subscribe to HA's
//!     `command_topic` publishes and translate them back to driver
//!     control calls yet; that's a Phase F-2 / control-path story.
//!   - Device removal. When a row is deleted from `smart_home_devices`
//!     we don't publish an empty retained payload to purge the HA
//!     discovery topic. Logged as a known gap; the user's HA will
//!     show the device until its retained config is cleared manually.
//!   - Per-user broker ACL. Since the v1 embedded broker is noauth +
//!     localhost-only (or shared-password per E-2), we publish under
//!     `syntaur/u/<user_id>/device/<id>/state` as a topic-namespace
//!     convention rather than an enforced ACL boundary.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::MqttSupervisor;
use crate::smart_home::events::{self, SmartHomeEvent};

/// Map `smart_home_devices.kind` → HA MQTT Discovery component.
///
/// <https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery>
///
/// Conservative default is `binary_sensor`: nothing breaks if HA
/// interprets a truly-unknown device as a binary sensor; it just
/// renders as "on/off" until the user's surface provides richer
/// capabilities.
pub fn kind_to_ha_component(kind: &str) -> &'static str {
    match kind {
        "plug" | "switch" => "switch",
        "light" => "light",
        "lock" => "lock",
        "cover" => "cover",
        "thermostat" | "climate" => "climate",
        "fan" => "fan",
        "sensor_motion"
        | "sensor_contact"
        | "sensor_presence"
        | "sensor_occupancy" => "binary_sensor",
        "sensor_climate" | "sensor_temperature" | "sensor_humidity" | "sensor" => "sensor",
        _ => "binary_sensor",
    }
}

/// Retained discovery topic for a given device.
pub fn discovery_topic(component: &str, device_id: i64) -> String {
    format!("homeassistant/{}/syntaur/{}/config", component, device_id)
}

/// Per-device state topic (owner-scoped by convention; see module doc).
pub fn state_topic(user_id: i64, device_id: i64) -> String {
    format!("syntaur/u/{}/device/{}/state", user_id, device_id)
}

/// Build the HA Discovery config payload for one commissioned device.
/// JSON shape is deliberately minimal — HA fills in sensible defaults
/// for fields like `availability_topic` and `payload_on` / `payload_off`.
pub fn build_discovery_config(
    component: &str,
    device_id: i64,
    user_id: i64,
    name: &str,
    driver: &str,
) -> Value {
    let unique_id = format!("syntaur-{}", device_id);
    let mut body = serde_json::json!({
        "name": name,
        "unique_id": unique_id,
        "state_topic": state_topic(user_id, device_id),
        "device": {
            "identifiers": [unique_id],
            "manufacturer": "Syntaur",
            "model": format!("{} driver", driver),
            "name": name,
        }
    });
    // Only sensor/binary_sensor need an explicit value_template default
    // when the payload is JSON. Lights + switches use payload_on/off
    // which HA infers from the state_topic body.
    if component == "sensor" || component == "binary_sensor" {
        body.as_object_mut().unwrap().insert(
            "value_template".into(),
            Value::String("{{ value_json.value | default(value) }}".into()),
        );
    }
    body
}

/// Phase F-1 publisher. Holds a supervisor handle, a db_path (for
/// the commissioning walk), and spawns a tokio task on
/// [`HADiscoveryPublisher::spawn`].
pub struct HADiscoveryPublisher {
    supervisor: Arc<MqttSupervisor>,
    db_path: PathBuf,
}

impl HADiscoveryPublisher {
    pub fn new(supervisor: Arc<MqttSupervisor>, db_path: PathBuf) -> Self {
        Self {
            supervisor,
            db_path,
        }
    }

    /// Detached task:
    ///   1. Small grace period so the supervisor's sessions have had a
    ///      chance to connect. Publishes before any session is live
    ///      are dropped (no matching handle); we pay an unnecessary
    ///      walk and continue — not a correctness bug, just wasted work.
    ///   2. Walk every commissioned device, publish retained HA
    ///      Discovery config.
    ///   3. Subscribe to the event bus; on DeviceStateChanged emit a
    ///      retained frame on the matching state_topic.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            self.publish_all_device_configs().await;
            self.run_state_loop().await;
        })
    }

    async fn publish_all_device_configs(&self) {
        let devices = match self.supervisor.list_commissioned_devices().await {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "[smart_home::ha_discovery] device walk failed: {} — skipping initial config publish",
                    e
                );
                return;
            }
        };
        for (device_id, user_id, name, kind, driver, _external_id) in devices {
            let component = kind_to_ha_component(&kind);
            let topic = discovery_topic(component, device_id);
            let payload = build_discovery_config(
                component, device_id, user_id, &name, &driver,
            );
            let bytes = match serde_json::to_vec(&payload) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!(
                        "[smart_home::ha_discovery] serialize config device_id={}: {}",
                        device_id,
                        e
                    );
                    continue;
                }
            };
            let n = self.supervisor.publish_retained(topic.clone(), bytes).await;
            log::debug!(
                "[smart_home::ha_discovery] published {} to {} sessions",
                topic,
                n
            );
        }
    }

    async fn run_state_loop(&self) {
        let mut rx = events::bus().subscribe();
        loop {
            match rx.recv().await {
                Ok(SmartHomeEvent::DeviceStateChanged {
                    user_id,
                    device_id,
                    state,
                    ..
                }) => {
                    let topic = state_topic(user_id, device_id);
                    let bytes = match serde_json::to_vec(&state) {
                        Ok(b) => b,
                        Err(e) => {
                            log::warn!(
                                "[smart_home::ha_discovery] serialize state device_id={}: {}",
                                device_id,
                                e
                            );
                            continue;
                        }
                    };
                    let _ = self.supervisor.publish_retained(topic, bytes).await;
                }
                Ok(_other) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!(
                        "[smart_home::ha_discovery] event bus lagged by {} — catching up",
                        n
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    log::info!("[smart_home::ha_discovery] event bus closed, exiting");
                    return;
                }
            }
        }
    }

    // db_path is kept for a planned Phase F-2 incremental republish
    // that watches for device CRUD events once we add them to the bus.
    // Silence the dead-code warning in the meantime.
    #[allow(dead_code)]
    fn db_path(&self) -> &PathBuf {
        &self.db_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_map_covers_core_classes() {
        assert_eq!(kind_to_ha_component("plug"), "switch");
        assert_eq!(kind_to_ha_component("switch"), "switch");
        assert_eq!(kind_to_ha_component("light"), "light");
        assert_eq!(kind_to_ha_component("lock"), "lock");
        assert_eq!(kind_to_ha_component("cover"), "cover");
        assert_eq!(kind_to_ha_component("thermostat"), "climate");
        assert_eq!(kind_to_ha_component("climate"), "climate");
        assert_eq!(kind_to_ha_component("fan"), "fan");
        assert_eq!(kind_to_ha_component("sensor_motion"), "binary_sensor");
        assert_eq!(kind_to_ha_component("sensor_contact"), "binary_sensor");
        assert_eq!(kind_to_ha_component("sensor_climate"), "sensor");
    }

    #[test]
    fn unknown_kind_defaults_to_binary_sensor() {
        assert_eq!(kind_to_ha_component("fancy_new_thing"), "binary_sensor");
    }

    #[test]
    fn discovery_topic_shape_is_homeassistant_prefixed() {
        assert_eq!(
            discovery_topic("switch", 7),
            "homeassistant/switch/syntaur/7/config"
        );
    }

    #[test]
    fn state_topic_namespaces_by_user_and_device() {
        assert_eq!(
            state_topic(3, 42),
            "syntaur/u/3/device/42/state"
        );
    }

    #[test]
    fn discovery_config_carries_unique_id_and_device_block() {
        let v = build_discovery_config("switch", 42, 1, "Kitchen Plug", "mqtt");
        assert_eq!(v["name"], "Kitchen Plug");
        assert_eq!(v["unique_id"], "syntaur-42");
        assert_eq!(v["state_topic"], "syntaur/u/1/device/42/state");
        assert_eq!(v["device"]["manufacturer"], "Syntaur");
        assert_eq!(v["device"]["identifiers"][0], "syntaur-42");
        // Switches do NOT get value_template — HA infers from on/off.
        assert!(v.get("value_template").is_none());
    }

    #[test]
    fn discovery_config_sensor_has_value_template() {
        let v = build_discovery_config("sensor", 9, 1, "Office Temp", "mqtt");
        assert!(v["value_template"].is_string());
    }

    #[test]
    fn discovery_config_binary_sensor_has_value_template() {
        let v = build_discovery_config("binary_sensor", 9, 1, "Front Door", "matter");
        assert!(v["value_template"].is_string());
    }
}
