//! Smart home device control — direct protocol control, no Home Assistant.
//!
//! Supported protocols:
//! - HTTP REST: Shelly, WLED, Tasmota, ESPHome, Philips Hue bridge
//! - Matter: via python-matter-server WebSocket (optional)
//! - MQTT: Zigbee2MQTT, Tasmota MQTT (optional)
//!
//! Device registry persisted in SQLite. mDNS discovery for auto-setup.

pub mod discovery;
pub mod http;
pub mod matter;
pub mod mdns_reflector;
pub mod mqtt;
pub mod registry;

use serde::{Deserialize, Serialize};

// ── Device abstraction ──────────────────────────────────────────────────

/// Supported device protocols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceProtocol {
    /// WiFi device with local HTTP API (Shelly, WLED, Tasmota, ESPHome).
    Http,
    /// Matter device via python-matter-server WebSocket.
    Matter,
    /// MQTT device (Zigbee2MQTT, Tasmota MQTT mode).
    Mqtt,
}

/// What a device can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCapability {
    OnOff,
    Brightness,
    ColorTemp,
    Color,
    Temperature,
    Humidity,
    Power,
    Energy,
}

/// Known device platform (for protocol-specific command formatting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Shelly,
    Wled,
    Tasmota,
    Esphome,
    HueBridge,
    Matter,
    Zigbee2mqtt,
    Generic,
}

impl std::fmt::Display for DevicePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shelly => write!(f, "shelly"),
            Self::Wled => write!(f, "wled"),
            Self::Tasmota => write!(f, "tasmota"),
            Self::Esphome => write!(f, "esphome"),
            Self::HueBridge => write!(f, "hue_bridge"),
            Self::Matter => write!(f, "matter"),
            Self::Zigbee2mqtt => write!(f, "zigbee2mqtt"),
            Self::Generic => write!(f, "generic"),
        }
    }
}

/// A registered smart device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub room: String,
    pub protocol: DeviceProtocol,
    pub platform: DevicePlatform,
    /// Network endpoint (IP:port, URL, or MQTT topic).
    pub endpoint: String,
    pub capabilities: Vec<DeviceCapability>,
    /// Optional auth token or API key.
    pub auth: Option<String>,
    /// Platform-specific metadata (e.g. Shelly gen, WLED segment, Matter node_id).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Command sent to a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DeviceCommand {
    TurnOn,
    TurnOff,
    Toggle,
    SetBrightness { brightness: u8 },
    SetColorTemp { kelvin: u32 },
    SetColor { r: u8, g: u8, b: u8 },
    Status,
}

/// Response from a device command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceState {
    pub device_id: String,
    pub is_on: Option<bool>,
    pub brightness: Option<u8>,
    pub color_temp_kelvin: Option<u32>,
    pub temperature: Option<f32>,
    pub humidity: Option<f32>,
    pub power_watts: Option<f32>,
    pub raw: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl DeviceState {
    pub fn ok(device_id: &str) -> Self {
        Self {
            device_id: device_id.to_string(),
            is_on: None,
            brightness: None,
            color_temp_kelvin: None,
            temperature: None,
            humidity: None,
            power_watts: None,
            raw: None,
            error: None,
        }
    }

    pub fn err(device_id: &str, error: impl Into<String>) -> Self {
        Self {
            device_id: device_id.to_string(),
            error: Some(error.into()),
            ..Self::ok(device_id)
        }
    }
}
