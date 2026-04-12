//! HTTP controller for WiFi smart devices with local REST APIs.
//!
//! Supported platforms:
//! - Shelly (Gen1 + Gen2/3 RPC)
//! - WLED (JSON API)
//! - Tasmota (command API)
//! - ESPHome (native web server API)
//! - Philips Hue Bridge (CLIP v2)
//! - Generic (simple GET on/off endpoints)

use log::{debug, warn};
use reqwest::Client;

use super::{Device, DeviceCommand, DevicePlatform, DeviceState};

/// Execute a command on an HTTP-controlled device.
pub async fn execute(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    match device.platform {
        DevicePlatform::Shelly => shelly(client, device, command).await,
        DevicePlatform::Wled => wled(client, device, command).await,
        DevicePlatform::Tasmota => tasmota(client, device, command).await,
        DevicePlatform::Esphome => esphome(client, device, command).await,
        DevicePlatform::HueBridge => hue(client, device, command).await,
        _ => generic(client, device, command).await,
    }
}

// ── Shelly (Gen2/3 RPC + Gen1 fallback) ─────────────────────────────────

async fn shelly(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let gen = device.metadata.get("gen").and_then(|v| v.as_u64()).unwrap_or(2);

    if gen >= 2 {
        shelly_gen2(client, base, &device.id, command).await
    } else {
        shelly_gen1(client, base, &device.id, command).await
    }
}

async fn shelly_gen2(client: &Client, base: &str, device_id: &str, command: &DeviceCommand) -> DeviceState {
    let (method, params) = match command {
        DeviceCommand::TurnOn => ("Switch.Set", serde_json::json!({"id": 0, "on": true})),
        DeviceCommand::TurnOff => ("Switch.Set", serde_json::json!({"id": 0, "on": false})),
        DeviceCommand::Toggle => ("Switch.Toggle", serde_json::json!({"id": 0})),
        DeviceCommand::SetBrightness { brightness } => {
            ("Light.Set", serde_json::json!({"id": 0, "on": true, "brightness": brightness}))
        }
        DeviceCommand::Status => ("Switch.GetStatus", serde_json::json!({"id": 0})),
        _ => return DeviceState::err(device_id, "unsupported command for Shelly"),
    };

    let url = format!("{}/rpc", base);
    let body = serde_json::json!({"id": 1, "method": method, "params": params});

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            let result = json.get("result").cloned().unwrap_or_default();
            let mut state = DeviceState::ok(device_id);
            state.is_on = result.get("output").and_then(|v| v.as_bool());
            state.raw = Some(result);
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach Shelly device — check it's powered on and connected to WiFi")),
    }
}

async fn shelly_gen1(client: &Client, base: &str, device_id: &str, command: &DeviceCommand) -> DeviceState {
    let path = match command {
        DeviceCommand::TurnOn => "/relay/0?turn=on",
        DeviceCommand::TurnOff => "/relay/0?turn=off",
        DeviceCommand::Toggle => "/relay/0?turn=toggle",
        DeviceCommand::Status => "/status",
        _ => return DeviceState::err(device_id, "unsupported for Shelly Gen1"),
    };

    match client.get(&format!("{}{}", base, path)).send().await {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            let mut state = DeviceState::ok(device_id);
            state.is_on = json.get("ison").and_then(|v| v.as_bool());
            state.raw = Some(json);
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach Shelly Gen1 device — check it's powered on and connected to WiFi")),
    }
}

// ── WLED ─────────────────────────────────────────────────────────────────

async fn wled(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let device_id = &device.id;

    let body = match command {
        DeviceCommand::TurnOn => serde_json::json!({"on": true}),
        DeviceCommand::TurnOff => serde_json::json!({"on": false}),
        DeviceCommand::Toggle => serde_json::json!({"on": "t"}),
        DeviceCommand::SetBrightness { brightness } => {
            serde_json::json!({"on": true, "bri": (*brightness as u32 * 255) / 100})
        }
        DeviceCommand::SetColor { r, g, b } => {
            serde_json::json!({"on": true, "seg": [{"col": [[r, g, b]]}]})
        }
        DeviceCommand::Status => {
            let url = format!("{}/json/state", base);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let json: serde_json::Value = resp.json().await.unwrap_or_default();
                    let mut state = DeviceState::ok(device_id);
                    state.is_on = json.get("on").and_then(|v| v.as_bool());
                    state.brightness = json.get("bri").and_then(|v| v.as_u64()).map(|b| ((b * 100) / 255) as u8);
                    state.raw = Some(json);
                    return state;
                }
                Err(e) => return DeviceState::err(device_id, format!("Can't reach WLED device — check it's powered on and connected to WiFi")),
            }
        }
        _ => return DeviceState::err(device_id, "unsupported for WLED"),
    };

    let url = format!("{}/json/state", base);
    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            let mut state = DeviceState::ok(device_id);
            state.is_on = json.get("on").and_then(|v| v.as_bool());
            state.raw = Some(json);
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach WLED device — check it's powered on and connected to WiFi")),
    }
}

// ── Tasmota ──────────────────────────────────────────────────────────────

async fn tasmota(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let device_id = &device.id;

    let cmnd = match command {
        DeviceCommand::TurnOn => "Power ON",
        DeviceCommand::TurnOff => "Power OFF",
        DeviceCommand::Toggle => "Power TOGGLE",
        DeviceCommand::SetBrightness { brightness } => {
            let dimmer = (*brightness).min(100);
            return tasmota_cmd(client, base, device_id, &format!("Dimmer {}", dimmer)).await;
        }
        DeviceCommand::SetColorTemp { kelvin } => {
            let k = (*kelvin).max(2000).min(6500);
            let mireds = (1_000_000u32 / k).min(500).max(153);
            return tasmota_cmd(client, base, device_id, &format!("CT {}", mireds)).await;
        }
        DeviceCommand::Status => "Status 0",
        _ => return DeviceState::err(device_id, "unsupported for Tasmota"),
    };

    tasmota_cmd(client, base, device_id, cmnd).await
}

async fn tasmota_cmd(client: &Client, base: &str, device_id: &str, cmnd: &str) -> DeviceState {
    let url = format!("{}/cm?cmnd={}", base, urlencoding::encode(cmnd));
    match client.get(&url).send().await {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            let mut state = DeviceState::ok(device_id);
            state.is_on = json.get("POWER").and_then(|v| v.as_str()).map(|s| s == "ON");
            state.raw = Some(json);
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach Tasmota device — check it's powered on and connected to WiFi")),
    }
}

// ── ESPHome ──────────────────────────────────────────────────────────────

async fn esphome(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let device_id = &device.id;
    let entity = device.metadata.get("entity").and_then(|v| v.as_str()).unwrap_or("light");

    match command {
        DeviceCommand::TurnOn => {
            esphome_post(client, base, device_id, entity, "turn_on", None).await
        }
        DeviceCommand::TurnOff => {
            esphome_post(client, base, device_id, entity, "turn_off", None).await
        }
        DeviceCommand::Toggle => {
            esphome_post(client, base, device_id, entity, "toggle", None).await
        }
        DeviceCommand::SetBrightness { brightness } => {
            let body = serde_json::json!({"brightness": (*brightness as f32 / 100.0) * 255.0});
            esphome_post(client, base, device_id, entity, "turn_on", Some(body)).await
        }
        DeviceCommand::Status => {
            esphome_status(client, base, device_id).await
        }
        _ => DeviceState::err(device_id, "unsupported for ESPHome"),
    }
}

async fn esphome_post(
    client: &Client, base: &str, device_id: &str, entity: &str,
    action: &str, body: Option<serde_json::Value>,
) -> DeviceState {
    let url = format!("{}/{}/{}", base, entity, action);
    let req = if let Some(b) = body {
        client.post(&url).json(&b)
    } else {
        client.post(&url)
    };
    match req.send().await {
        Ok(_) => DeviceState::ok(device_id),
        Err(e) => DeviceState::err(device_id, format!("Can't reach ESPHome device — check it's powered on and connected to WiFi")),
    }
}

async fn esphome_status(client: &Client, base: &str, device_id: &str) -> DeviceState {
    match client.get(base).send().await {
        Ok(resp) => {
            let mut state = DeviceState::ok(device_id);
            state.raw = resp.json().await.ok();
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't get ESPHome device status — check it's powered on and connected to WiFi")),
    }
}

// ── Philips Hue Bridge ───────────────────────────────────────────────────

async fn hue(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let device_id = &device.id;
    let light_id = device.metadata.get("light_id").and_then(|v| v.as_str()).unwrap_or("1");
    let api_key = device.auth.as_deref().unwrap_or("");

    let url = format!("{}/api/{}/lights/{}/state", base, api_key, light_id);

    let body = match command {
        DeviceCommand::TurnOn => serde_json::json!({"on": true}),
        DeviceCommand::TurnOff => serde_json::json!({"on": false}),
        DeviceCommand::SetBrightness { brightness } => {
            serde_json::json!({"on": true, "bri": (*brightness as u32 * 254) / 100})
        }
        DeviceCommand::SetColorTemp { kelvin } => {
            let k = (*kelvin).max(2000).min(6500);
            let mireds = 1_000_000u32 / k;
            serde_json::json!({"on": true, "ct": mireds})
        }
        DeviceCommand::Status => {
            let status_url = format!("{}/api/{}/lights/{}", base, api_key, light_id);
            match client.get(&status_url).send().await {
                Ok(resp) => {
                    let json: serde_json::Value = resp.json().await.unwrap_or_default();
                    let mut state = DeviceState::ok(device_id);
                    state.is_on = json.get("state").and_then(|s| s.get("on")).and_then(|v| v.as_bool());
                    state.brightness = json.get("state").and_then(|s| s.get("bri"))
                        .and_then(|v| v.as_u64()).map(|b| ((b * 100) / 254) as u8);
                    state.raw = Some(json);
                    return state;
                }
                Err(e) => return DeviceState::err(device_id, format!("Can't reach Hue Bridge — check it's powered on and connected to the network")),
            }
        }
        _ => return DeviceState::err(device_id, "unsupported for Hue"),
    };

    match client.put(&url).json(&body).send().await {
        Ok(_) => {
            let mut state = DeviceState::ok(device_id);
            state.is_on = match command {
                DeviceCommand::TurnOn | DeviceCommand::SetBrightness { .. } => Some(true),
                DeviceCommand::TurnOff => Some(false),
                _ => None,
            };
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach Hue Bridge — check it's powered on and connected to the network")),
    }
}

// ── Generic (simple on/off URL) ──────────────────────────────────────────

async fn generic(client: &Client, device: &Device, command: &DeviceCommand) -> DeviceState {
    let base = device.endpoint.trim_end_matches('/');
    let device_id = &device.id;

    let path = match command {
        DeviceCommand::TurnOn => "/on",
        DeviceCommand::TurnOff => "/off",
        DeviceCommand::Toggle => "/toggle",
        DeviceCommand::Status => "/status",
        _ => return DeviceState::err(device_id, "unsupported for generic device"),
    };

    match client.get(&format!("{}{}", base, path)).send().await {
        Ok(resp) => {
            let mut state = DeviceState::ok(device_id);
            state.raw = resp.json().await.ok();
            state
        }
        Err(e) => DeviceState::err(device_id, format!("Can't reach device — check it's powered on and connected to the network")),
    }
}
