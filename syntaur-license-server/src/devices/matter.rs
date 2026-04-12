//! Matter device controller via python-matter-server WebSocket.
//!
//! Ported from Syntaur's production matter.rs. Talks directly to
//! python-matter-server at ws://<host>:5580/ws, bypassing Home Assistant.
//!
//! Supports: on/off, brightness, color temperature, device status, node listing.
//! Cluster IDs: 6 (ON_OFF), 8 (LEVEL_CONTROL), 768 (COLOR_CONTROL).

use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{Device, DeviceCommand, DeviceState};

/// Room mapping loaded from config or auto-generated from device names.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomMapping {
    pub friendly_name: String,
    #[serde(default)]
    pub node_ids: Vec<u64>,
    #[serde(default)]
    pub bulb_ids: Vec<u64>,
    #[serde(default)]
    pub devices: HashMap<String, String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Matter controller connected to python-matter-server.
pub struct MatterController {
    ws_url: String,
}

impl MatterController {
    pub fn new(ws_url: &str) -> Self {
        Self {
            ws_url: ws_url.to_string(),
        }
    }

    /// Execute a command on a Matter device by node ID.
    pub async fn execute(&self, device: &Device, command: &DeviceCommand) -> DeviceState {
        let node_id = device
            .metadata
            .get("node_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if node_id == 0 {
            return DeviceState::err(&device.id, "no node_id in device metadata");
        }

        match command {
            DeviceCommand::TurnOn => {
                self.device_command(node_id, 1, 6, "On", serde_json::json!({}), &device.id)
                    .await
            }
            DeviceCommand::TurnOff => {
                self.device_command(node_id, 1, 6, "Off", serde_json::json!({}), &device.id)
                    .await
            }
            DeviceCommand::Toggle => {
                self.device_command(node_id, 1, 6, "Toggle", serde_json::json!({}), &device.id)
                    .await
            }
            DeviceCommand::SetBrightness { brightness } => {
                let level = (*brightness as u32 * 254) / 100;
                self.device_command(
                    node_id,
                    1,
                    8,
                    "MoveToLevelWithOnOff",
                    serde_json::json!({"level": level, "transitionTime": 0}),
                    &device.id,
                )
                .await
            }
            DeviceCommand::SetColorTemp { kelvin } => {
                let k = (*kelvin).max(2000).min(6500);
                let mireds = 1_000_000u32 / k;
                self.device_command(
                    node_id,
                    1,
                    768,
                    "MoveToColorTemperature",
                    serde_json::json!({"colorTemperatureMireds": mireds, "transitionTime": 0}),
                    &device.id,
                )
                .await
            }
            DeviceCommand::Status => self.get_node_status(node_id, &device.id).await,
            _ => DeviceState::err(&device.id, "unsupported Matter command"),
        }
    }

    /// Send a device_command to python-matter-server.
    async fn device_command(
        &self,
        node_id: u64,
        endpoint_id: u64,
        cluster_id: u64,
        command_name: &str,
        payload: serde_json::Value,
        device_id: &str,
    ) -> DeviceState {
        let msg = serde_json::json!({
            "message_id": "1",
            "command": "device_command",
            "args": {
                "node_id": node_id,
                "endpoint_id": endpoint_id,
                "cluster_id": cluster_id,
                "command_name": command_name,
                "payload": payload,
            }
        });

        match self.send_command(&msg).await {
            Ok(resp) => {
                let mut state = DeviceState::ok(device_id);
                if resp.get("error_code").and_then(|v| v.as_u64()).unwrap_or(0) != 0 {
                    state.error = resp
                        .get("details")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
                state.raw = Some(resp);
                state
            }
            Err(e) => DeviceState::err(device_id, e),
        }
    }

    /// Get status of a single node.
    async fn get_node_status(&self, node_id: u64, device_id: &str) -> DeviceState {
        let msg = serde_json::json!({
            "message_id": "1",
            "command": "get_node",
            "args": {"node_id": node_id}
        });

        match self.send_command(&msg).await {
            Ok(resp) => {
                let result = resp.get("result").cloned().unwrap_or_default();
                let attrs = result
                    .get("attributes")
                    .cloned()
                    .unwrap_or_default();

                let mut state = DeviceState::ok(device_id);
                // ON_OFF cluster: attribute "1/6/0"
                state.is_on = attrs.get("1/6/0").and_then(|v| v.as_bool());
                // LEVEL_CONTROL cluster: attribute "1/8/0"
                if let Some(level) = attrs.get("1/8/0").and_then(|v| v.as_u64()) {
                    state.brightness = Some(((level * 100) / 254) as u8);
                }
                state.raw = Some(result);
                state
            }
            Err(e) => DeviceState::err(device_id, e),
        }
    }

    /// List all nodes on the Matter fabric.
    pub async fn list_nodes(&self) -> Result<Vec<MatterNode>, String> {
        let msg = serde_json::json!({
            "message_id": "1",
            "command": "get_nodes",
            "args": {}
        });

        let resp = self.send_command(&msg).await?;
        let result = resp.get("result").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let nodes: Vec<MatterNode> = result
            .iter()
            .filter_map(|node| {
                let node_id = node.get("node_id")?.as_u64()?;
                let available = node.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
                let attrs = node.get("attributes").cloned().unwrap_or_default();

                // Extract friendly name from BasicInformation cluster (0/40/5)
                let name = attrs
                    .get("0/40/5")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();

                // Product name from (0/40/4)
                let product = attrs
                    .get("0/40/4")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                Some(MatterNode {
                    node_id,
                    name,
                    product,
                    available,
                    is_on: attrs.get("1/6/0").and_then(|v| v.as_bool()),
                    brightness: attrs.get("1/8/0").and_then(|v| v.as_u64()).map(|l| ((l * 100) / 254) as u8),
                })
            })
            .collect();

        Ok(nodes)
    }

    /// Send a command to python-matter-server and get the response.
    async fn send_command(
        &self,
        msg: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let connector = native_tls::TlsConnector::new().map_err(|e| format!("tls: {}", e))?;

        let (mut ws, _) = tokio_tungstenite::connect_async(&self.ws_url)
            .await
            .map_err(|e| format!("matter ws connect: {}", e))?;

        // Read and discard server info message
        if let Some(Ok(_)) = ws.next().await {}

        // Send command
        let text = msg.to_string();
        debug!("[matter] sending: {}", &text[..text.len().min(200)]);
        ws.send(WsMessage::Text(text.into()))
            .await
            .map_err(|e| format!("matter ws send: {}", e))?;

        // Read response
        let resp = ws
            .next()
            .await
            .ok_or("matter ws: no response")?
            .map_err(|e| format!("matter ws read: {}", e))?;

        let _ = ws.close(None).await;

        match resp {
            WsMessage::Text(text) => {
                serde_json::from_str(&text)
                    .map_err(|e| format!("matter parse: {} (raw: {}...)", e, &text[..text.len().min(200)]))
            }
            _ => Err("matter: unexpected message type".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MatterNode {
    pub node_id: u64,
    pub name: String,
    pub product: String,
    pub available: bool,
    pub is_on: Option<bool>,
    pub brightness: Option<u8>,
}
