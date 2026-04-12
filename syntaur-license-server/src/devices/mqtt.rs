//! MQTT controller for Zigbee2MQTT and Z-Wave JS devices.
//!
//! Zigbee2MQTT: publish JSON to `zigbee2mqtt/{device_name}/set`
//! Z-Wave JS UI: publish to `zwave/{node_id}/set`
//!
//! Covers: 5200+ Zigbee devices, 4000+ Z-Wave devices — any device
//! supported by Zigbee2MQTT or Z-Wave JS works automatically.

use log::{debug, info, warn};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::{Device, DeviceCommand, DevicePlatform, DeviceState};

/// Persistent MQTT connection for device control.
pub struct MqttController {
    client: Arc<Mutex<Option<rumqttc::AsyncClient>>>,
    broker_url: String,
    broker_port: u16,
}

impl MqttController {
    pub fn new(broker_url: &str, broker_port: u16) -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            broker_url: broker_url.to_string(),
            broker_port,
        }
    }

    /// Connect to the MQTT broker (lazy — called on first command).
    async fn ensure_connected(&self) -> Result<rumqttc::AsyncClient, String> {
        let mut guard = self.client.lock().await;
        if let Some(ref client) = *guard {
            return Ok(client.clone());
        }

        let mut options = rumqttc::MqttOptions::new(
            format!("syntaur-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x")),
            &self.broker_url,
            self.broker_port,
        );
        options.set_keep_alive(Duration::from_secs(30));

        let (client, mut eventloop) = rumqttc::AsyncClient::new(options, 64);

        // Spawn the event loop in the background
        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(_) => {}
                    Err(e) => {
                        warn!("[mqtt] event loop error: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });

        // Wait a moment for connection to establish
        tokio::time::sleep(Duration::from_millis(500)).await;

        info!("[mqtt] connected to {}:{}", self.broker_url, self.broker_port);
        *guard = Some(client.clone());
        Ok(client)
    }

    /// Execute a command on an MQTT-controlled device.
    pub async fn execute(&self, device: &Device, command: &DeviceCommand) -> DeviceState {
        let client = match self.ensure_connected().await {
            Ok(c) => c,
            Err(e) => return DeviceState::err(&device.id, format!("mqtt connect: {}", e)),
        };

        let topic = device.metadata.get("topic")
            .and_then(|v| v.as_str())
            .map(|t| format!("{}/set", t))
            .unwrap_or_else(|| match device.platform {
                DevicePlatform::Zigbee2mqtt => format!("zigbee2mqtt/{}/set", device.name),
                _ => format!("zwave/{}/set", device.name),
            });

        let payload = match command {
            DeviceCommand::TurnOn => serde_json::json!({"state": "ON"}),
            DeviceCommand::TurnOff => serde_json::json!({"state": "OFF"}),
            DeviceCommand::Toggle => serde_json::json!({"state": "TOGGLE"}),
            DeviceCommand::SetBrightness { brightness } => {
                // Zigbee2MQTT brightness: 0-254
                let bri = (*brightness as u32 * 254) / 100;
                serde_json::json!({"state": "ON", "brightness": bri})
            }
            DeviceCommand::SetColorTemp { kelvin } => {
                // Zigbee2MQTT color_temp in mireds
                let mireds = 1_000_000u32 / (*kelvin).max(2000).min(6500);
                serde_json::json!({"state": "ON", "color_temp": mireds})
            }
            DeviceCommand::SetColor { r, g, b } => {
                serde_json::json!({"state": "ON", "color": {"r": r, "g": g, "b": b}})
            }
            DeviceCommand::Status => {
                // Publish a get request to read state
                let get_topic = topic.replace("/set", "/get");
                let _ = client.publish(
                    &get_topic,
                    rumqttc::QoS::AtMostOnce,
                    false,
                    serde_json::json!({"state": ""}).to_string(),
                ).await;
                return DeviceState::ok(&device.id);
            }
        };

        let payload_str = payload.to_string();
        debug!("[mqtt] publish {} → {}", topic, payload_str);

        match client.publish(&topic, rumqttc::QoS::AtMostOnce, false, payload_str).await {
            Ok(_) => {
                let mut state = DeviceState::ok(&device.id);
                state.is_on = match command {
                    DeviceCommand::TurnOn | DeviceCommand::SetBrightness { .. }
                    | DeviceCommand::SetColor { .. } | DeviceCommand::SetColorTemp { .. } => Some(true),
                    DeviceCommand::TurnOff => Some(false),
                    _ => None,
                };
                state
            }
            Err(e) => DeviceState::err(&device.id, format!("mqtt publish: {}", e)),
        }
    }
}
