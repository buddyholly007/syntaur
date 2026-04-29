//! Matter device control via python-matter-server WebSocket.
//!
//! Connects directly to the python-matter-server WebSocket at
//! ws://127.0.0.1:5580/ws (SSH tunneled from syntaur-server to HA's
//! internal Docker bridge). Bypasses HA REST for Matter device commands.
//!
//! ## Architecture
//!
//! python-matter-server manages the Matter fabric (commissioning, CASE
//! sessions, attribute subscriptions). We connect as a WebSocket client
//! and send device_command messages to control devices. The server
//! handles all the Matter protocol details.
//!
//! When rs-matter ships CASE initiator support, this module can be
//! swapped to a direct rs-matter controller. The Tool interface stays
//! the same — only the backend changes.
//!
//! ## Prerequisites
//!
//! SSH tunnel must be active on syntaur-server:
//! ```
//! ssh -fNL 5580:172.30.32.1:5580 buddyholly007@192.168.1.3
//! ```
//! TODO: set up autossh or a systemd service for persistence.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, info, warn};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

const MATTER_WS_URL: &str = "ws://127.0.0.1:5580/ws";

// Matter cluster IDs
const CLUSTER_ON_OFF: u32 = 6;
const CLUSTER_LEVEL_CONTROL: u32 = 8;
const CLUSTER_COLOR_CONTROL: u32 = 768; // 0x300
const CLUSTER_DOOR_LOCK: u32 = 257; // 0x101

/// Load the room mapping from disk.
/// Looks in ~/.syntaur first (persistent config), falls back to /tmp/syntaur.
fn load_room_mapping() -> Option<HashMap<String, Value>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let primary = format!("{}/.syntaur/matter_rooms.json", home);
    let fallback = "/tmp/syntaur/matter_rooms.json";

    let content = match std::fs::read_to_string(&primary) {
        Ok(c) => c,
        Err(_) => match std::fs::read_to_string(fallback) {
            Ok(c) => {
                warn!("[matter] loaded rooms from fallback {}", fallback);
                c
            }
            Err(_) => {
                warn!("[matter] room mapping not found at {} or {}", primary, fallback);
                return None;
            }
        },
    };

    match serde_json::from_str(&content) {
        Ok(m) => Some(m),
        Err(e) => {
            warn!("[matter] failed to parse room mapping: {}", e);
            None
        }
    }
}

/// Get the friendly device label for a node ID from the room mapping.
fn device_label(mapping: &HashMap<String, Value>, node_id: u64) -> Option<String> {
    let nid_str = node_id.to_string();
    for (_, entry) in mapping {
        if let Some(devices) = entry.get("devices").and_then(|v| v.as_object()) {
            if let Some(label) = devices.get(&nid_str).and_then(|v| v.as_str()) {
                return Some(label.to_string());
            }
        }
    }
    None
}

/// Resolve a room name to Matter BULB node IDs from the mapping file.
/// Used for brightness/color commands that target smart bulbs, not switches.
fn resolve_room_bulbs(room: &str) -> Option<Vec<u64>> {
    let mapping = load_room_mapping()?;
    let room_lower = room.to_lowercase().trim().to_string();

    let extract_bulb_ids = |entry: &Value| -> Option<Vec<u64>> {
        entry.get("bulb_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
    };

    if let Some(entry) = mapping.get(&room_lower.replace(' ', "_")) {
        return extract_bulb_ids(entry);
    }

    for (_, entry) in &mapping {
        let friendly = entry.get("friendly_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if friendly == room_lower || friendly.contains(&room_lower) || room_lower.contains(&friendly) {
            return extract_bulb_ids(entry);
        }
        if let Some(aliases) = entry.get("aliases").and_then(|v| v.as_array()) {
            for alias in aliases {
                let a = alias.as_str().unwrap_or("").to_lowercase();
                if a == room_lower || a.contains(&room_lower) || room_lower.contains(&a) {
                    return extract_bulb_ids(entry);
                }
            }
        }
    }
    None
}

/// Resolve a room name to Matter node IDs (switches/dimmers only) from the mapping file.
/// Falls back to fuzzy matching on friendly_name and aliases.
fn resolve_room(room: &str) -> Option<Vec<u64>> {
    let mapping = load_room_mapping()?;
    let room_lower = room.to_lowercase().trim().to_string();

    let extract_ids = |entry: &Value| -> Option<Vec<u64>> {
        entry.get("node_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
    };

    // Try exact area_id match
    if let Some(entry) = mapping.get(&room_lower.replace(' ', "_")) {
        return extract_ids(entry);
    }

    // Try fuzzy match on friendly_name and aliases
    for (_, entry) in &mapping {
        let friendly = entry.get("friendly_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if friendly == room_lower || friendly.contains(&room_lower) || room_lower.contains(&friendly) {
            return extract_ids(entry);
        }

        if let Some(aliases) = entry.get("aliases").and_then(|v| v.as_array()) {
            for alias in aliases {
                let a = alias.as_str().unwrap_or("").to_lowercase();
                if a == room_lower || a.contains(&room_lower) || room_lower.contains(&a) {
                    return extract_ids(entry);
                }
            }
        }
    }

    None
}

/// Pure-Rust Matter WebSocket client using tokio-tungstenite.
///
/// Connects on demand, sends one command, reads the response, and
/// disconnects. For voice-driven commands (one at a time, seconds apart),
/// connect-per-call is fine and avoids connection management complexity.
///
/// Zero Python in this path — fully Rust from voice_chat → router →
/// matter_command → tokio-tungstenite → python-matter-server.
async fn matter_command(
    _client: &reqwest::Client, // unused now, kept for API compat
    command: &str,
    args: Value,
) -> Result<Value, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (mut ws, _) = connect_async(MATTER_WS_URL)
        .await
        .map_err(|e| format!("matter ws connect: {}", e))?;

    // First message from server is ServerInfo — read and discard
    if let Some(Ok(msg)) = ws.next().await {
        debug!("[matter] server info: {}", msg.to_text().unwrap_or("(binary)").chars().take(100).collect::<String>());
    }

    // Send command
    let req = json!({
        "message_id": "1",
        "command": command,
        "args": args,
    });
    ws.send(Message::Text(req.to_string()))
        .await
        .map_err(|e| format!("matter ws send: {}", e))?;

    // Read response
    let resp_msg = ws.next().await
        .ok_or_else(|| "matter ws: connection closed before response".to_string())?
        .map_err(|e| format!("matter ws recv: {}", e))?;

    let resp_text = resp_msg.to_text()
        .map_err(|e| format!("matter ws: non-text response: {}", e))?;

    let resp: Value = serde_json::from_str(resp_text)
        .map_err(|e| format!("matter ws parse: {} raw={}", e, resp_text.chars().take(200).collect::<String>()))?;

    // Close cleanly
    let _ = ws.close(None).await;

    if let Some(error_code) = resp.get("error_code") {
        let details = resp.get("details").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Err(format!("matter error {}: {}", error_code, details));
    }

    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

// ── Public surface for smart_home module (v1 legacy-bridge wrapper) ─────
//
// The new Smart Home and Network module (`crate::smart_home`) wraps this
// file as a driver until the v1.1 pure-Rust Matter Controller lands.
// These helpers expose just enough to list commissioned nodes + look up
// their friendly labels + rooms without duplicating the WebSocket
// plumbing below.

/// List every commissioned Matter node via the python-matter-server
/// bridge at `MATTER_WS_URL`. Returns the raw node JSON array so the
/// caller can inspect `node_id`, `available`, `attributes`, etc.
///
/// Returns `Err(_)` on WS connection failure — callers that want to
/// silently degrade (e.g. the smart_home scan pipeline) should map to
/// an empty Vec.
pub async fn list_nodes() -> Result<Vec<Value>, String> {
    let nodes = matter_command(&reqwest::Client::new(), "get_nodes", json!({})).await?;
    Ok(nodes.as_array().cloned().unwrap_or_default())
}

/// Turn a Matter node on or off (OnOff cluster, endpoint 1).
pub async fn set_onoff(node_id: u64, on: bool) -> Result<(), String> {
    let cmd = if on { "on" } else { "off" };
    matter_command(
        &reqwest::Client::new(),
        "device_command",
        json!({
            "node_id": node_id,
            "endpoint_id": 1,
            "cluster_id": CLUSTER_ON_OFF,
            "command_name": cmd,
            "payload": {}
        }),
    )
    .await
    .map(|_| ())
}

/// Set a dimmable node's brightness. `level_fraction` ∈ [0.0, 1.0]; the
/// bridge speaks Matter LevelControl in 0..=254.
pub async fn set_level(node_id: u64, level_fraction: f64) -> Result<(), String> {
    let lvl = (level_fraction.clamp(0.0, 1.0) * 254.0).round() as u16;
    let lvl = lvl.min(254);
    matter_command(
        &reqwest::Client::new(),
        "device_command",
        json!({
            "node_id": node_id,
            "endpoint_id": 1,
            "cluster_id": CLUSTER_LEVEL_CONTROL,
            "command_name": "MoveToLevelWithOnOff",
            "payload": {"level": lvl, "transitionTime": 0}
        }),
    )
    .await
    .map(|_| ())
}

/// Set a tunable-white bulb's color temperature in Kelvin. The
/// ColorControl cluster speaks mireds (1e6 / kelvin).
pub async fn set_color_temp_kelvin(node_id: u64, kelvin: u32) -> Result<(), String> {
    let mireds = (1_000_000u32 / kelvin.max(1)) as u16;
    matter_command(
        &reqwest::Client::new(),
        "device_command",
        json!({
            "node_id": node_id,
            "endpoint_id": 1,
            "cluster_id": CLUSTER_COLOR_CONTROL,
            "command_name": "MoveToColorTemperature",
            "payload": {"colorTemperature": mireds, "transitionTime": 0}
        }),
    )
    .await
    .map(|_| ())
}

/// Lock a Matter DoorLock node (cluster 0x0101 command 0x00 LockDoor on
/// endpoint 1). The optional `pin_code` is the user PIN as bytes — most
/// admin-fabric writes accept `None`. python-matter-server's
/// `device_command` accepts the matter-server-compatible name `lock-door`
/// for the LockDoor command and routes the JSON payload through to
/// `OperationalState.LockDoor`.
///
/// Aqara U200 multi-admin: once Sean opens the Aqara-app commissioning
/// window and Path C completes the AddNOC flow, this helper drives lock
/// commands directly without touching Aqara cloud. See
/// [[projects/aqara_u200_direct_control]].
pub async fn lock_door(node_id: u64, pin_code: Option<Vec<u8>>) -> Result<(), String> {
    let mut payload = serde_json::Map::new();
    if let Some(pin) = pin_code {
        // python-matter-server expects octet-string fields as base64; mirror
        // its on-the-wire encoding for nullable PIN credentials. Spec §5.2.7.
        use base64::Engine;
        payload.insert(
            "pinCode".into(),
            Value::from(base64::engine::general_purpose::STANDARD.encode(pin)),
        );
    }
    matter_command(
        &reqwest::Client::new(),
        "device_command",
        json!({
            "node_id": node_id,
            "endpoint_id": 1,
            "cluster_id": CLUSTER_DOOR_LOCK,
            "command_name": "LockDoor",
            "payload": Value::Object(payload),
        }),
    )
    .await
    .map(|_| ())
}

/// Unlock a Matter DoorLock node (cluster 0x0101 command 0x01 UnlockDoor
/// on endpoint 1). PIN handling matches `lock_door` — most admin-fabric
/// writes accept `None`.
pub async fn unlock_door(node_id: u64, pin_code: Option<Vec<u8>>) -> Result<(), String> {
    let mut payload = serde_json::Map::new();
    if let Some(pin) = pin_code {
        use base64::Engine;
        payload.insert(
            "pinCode".into(),
            Value::from(base64::engine::general_purpose::STANDARD.encode(pin)),
        );
    }
    matter_command(
        &reqwest::Client::new(),
        "device_command",
        json!({
            "node_id": node_id,
            "endpoint_id": 1,
            "cluster_id": CLUSTER_DOOR_LOCK,
            "command_name": "UnlockDoor",
            "payload": Value::Object(payload),
        }),
    )
    .await
    .map(|_| ())
}

/// Pull live cluster attributes for one node from the bridge and project
/// them into the typed smart_home state JSON shape (on / level / locked /
/// temperature / humidity / motion / contact / battery). Unknown fields
/// are omitted rather than filled with defaults.
pub async fn get_node_state(node_id: u64) -> Result<Value, String> {
    let nodes = list_nodes().await?;
    let node = nodes
        .iter()
        .find(|n| n.get("node_id").and_then(|v| v.as_u64()) == Some(node_id))
        .ok_or_else(|| format!("node {} not known to bridge", node_id))?;

    let attrs = node
        .get("attributes")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut out = serde_json::Map::new();

    // OnOff cluster (0x0006) → on: bool
    if let Some(v) = attrs.get("1/6/0").and_then(|v| v.as_bool()) {
        out.insert("on".into(), Value::from(v));
    }
    // LevelControl cluster (0x0008) → level: f64 in [0,1]
    if let Some(v) = attrs.get("1/8/0").and_then(|v| v.as_u64()) {
        out.insert("level".into(), Value::from(v as f64 / 254.0));
    }
    // DoorLock cluster (0x0101 = 257) → locked: bool. Matter LockState
    // enum: 0 = not-fully-locked, 1 = locked, 2 = unlocked.
    if let Some(v) = attrs.get("1/257/0").and_then(|v| v.as_u64()) {
        out.insert("locked".into(), Value::from(v == 1));
    }
    // TemperatureMeasurement (0x0402 = 1026) — value is centi-degrees C.
    if let Some(v) = attrs.get("1/1026/0").and_then(|v| v.as_i64()) {
        out.insert("temperature".into(), Value::from(v as f64 / 100.0));
    }
    // RelativeHumidityMeasurement (0x0405 = 1029) — percent × 100.
    if let Some(v) = attrs.get("1/1029/0").and_then(|v| v.as_u64()) {
        out.insert("humidity".into(), Value::from(v as f64 / 100.0));
    }
    // OccupancySensing (0x0406 = 1030) → motion: bit 0 of the bitmap.
    if let Some(v) = attrs.get("1/1030/0").and_then(|v| v.as_u64()) {
        out.insert("motion".into(), Value::from((v & 0x01) != 0));
    }
    // PowerSource battery percent (0x002F = 47).
    if let Some(v) = attrs.get("0/47/12").and_then(|v| v.as_u64()) {
        out.insert("battery".into(), Value::from(v as f64 / 2.0));
    }
    // Always include availability so stale tiles show up.
    if let Some(v) = node.get("available").and_then(|v| v.as_bool()) {
        out.insert("available".into(), Value::from(v));
    }
    Ok(Value::Object(out))
}

/// Best-effort lookup of a friendly (device-label, room-area-id,
/// room-friendly-name) for a node_id against the `matter_rooms.json`
/// config. All three fall back to None if the mapping file is missing
/// or the node isn't listed.
pub fn lookup_node_metadata(node_id: u64) -> (Option<String>, Option<String>, Option<String>) {
    let Some(mapping) = load_room_mapping() else {
        return (None, None, None);
    };
    let nid_str = node_id.to_string();
    for (area_id, entry) in &mapping {
        let devices = entry.get("devices").and_then(|v| v.as_object());
        let in_room = devices
            .and_then(|d| d.get(&nid_str).and_then(|v| v.as_str()))
            .map(str::to_string);
        let node_ids_match = entry
            .get("node_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_u64() == Some(node_id)))
            .unwrap_or(false);
        let bulb_ids_match = entry
            .get("bulb_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_u64() == Some(node_id)))
            .unwrap_or(false);
        if in_room.is_some() || node_ids_match || bulb_ids_match {
            let friendly = entry
                .get("friendly_name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            return (in_room, Some(area_id.clone()), friendly);
        }
    }
    (None, None, None)
}

pub struct MatterTool;

#[async_trait]
impl Tool for MatterTool {
    fn name(&self) -> &str {
        "matter"
    }

    fn description(&self) -> &str {
        "Control smart home devices by room. Turn lights on/off, set brightness, \
         change color temperature. Always use room names (kitchen, office, master bedroom, etc.) \
         rather than individual device IDs. Use action=list to see all rooms and their switches."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "on", "off", "toggle", "brightness", "color_temp", "status"],
                    "description": "list=show all devices, on/off/toggle=switch state, brightness=set level 0-254, color_temp=set kelvin, status=read device state"
                },
                "room": {
                    "type": "string",
                    "description": "Room name (e.g. 'kitchen', 'office', 'master bedroom', 'living room'). Commands are sent to ALL nodes in the room. Preferred over node_id."
                },
                "node_id": {
                    "type": "integer",
                    "description": "Specific Matter node ID. Use room instead when possible."
                },
                "value": {
                    "type": "integer",
                    "description": "For brightness: level 0-254. For color_temp: Kelvin (2700-6500)."
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            network: true,
            idempotent: false,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");

        match action {
            "list" => {
                // Fetch live node states from Matter server
                let nodes = matter_command(
                    &reqwest::Client::new(),
                    "get_nodes",
                    json!({}),
                ).await?;

                let node_list = nodes.as_array().cloned().unwrap_or_default();
                if node_list.is_empty() {
                    return Ok(RichToolResult::text("No Matter devices found."));
                }

                // Build a lookup of node_id -> on/off state from live data
                let mut node_states: HashMap<u64, Option<bool>> = HashMap::new();
                for n in &node_list {
                    let nid = n.get("node_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let avail = n.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
                    if avail {
                        let attrs = n.get("attributes").cloned().unwrap_or_default();
                        let on_off = attrs.get("1/6/0").and_then(|v| v.as_bool());
                        node_states.insert(nid, on_off);
                    }
                }

                // Show rooms with their controllable devices (switches/dimmers only)
                let mapping = load_room_mapping().unwrap_or_default();
                let mut lines = Vec::new();

                // Sort rooms by friendly name
                let mut rooms: Vec<_> = mapping.iter().collect();
                rooms.sort_by_key(|(_, v)| v.get("friendly_name").and_then(|n| n.as_str()).unwrap_or(""));

                for (_, entry) in &rooms {
                    let friendly = entry.get("friendly_name").and_then(|v| v.as_str()).unwrap_or("?");
                    let switch_ids: Vec<u64> = entry.get("node_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
                        .unwrap_or_default();
                    let devices = entry.get("devices").and_then(|v| v.as_object());

                    let mut device_lines = Vec::new();
                    for nid in &switch_ids {
                        let label = devices
                            .and_then(|d| d.get(&nid.to_string()))
                            .and_then(|v| v.as_str())
                            .unwrap_or("switch");
                        let state = match node_states.get(nid) {
                            Some(Some(true)) => "on",
                            Some(Some(false)) => "off",
                            Some(None) => "off",
                            None => "offline",
                        };
                        device_lines.push(format!("    {} ({})", label, state));
                    }

                    if !device_lines.is_empty() {
                        lines.push(format!("  {}:", friendly));
                        lines.extend(device_lines);
                    }
                }

                Ok(RichToolResult::text(format!(
                    "Smart home devices by room:\n{}",
                    lines.join("\n")
                )))
            }
            "on" | "off" | "toggle" => {
                let room = args.get("room").and_then(|v| v.as_str()).unwrap_or("");
                let cmd = match action {
                    "on" => "On",
                    "off" => "Off",
                    "toggle" => "Toggle",
                    _ => unreachable!(),
                };

                let mapping = load_room_mapping().unwrap_or_default();

                // Resolve target nodes — either from room name or single node_id
                let node_ids = if !room.is_empty() {
                    resolve_room(room).ok_or_else(|| {
                        let known: Vec<String> = mapping.values()
                            .filter_map(|v| v.get("friendly_name").and_then(|n| n.as_str()))
                            .map(|s| s.to_lowercase())
                            .collect();
                        format!("matter: unknown room '{}'. Known rooms: {}.", room, known.join(", "))
                    })?
                } else if let Some(nid) = args.get("node_id").and_then(|v| v.as_u64()) {
                    vec![nid]
                } else {
                    return Err("matter: either 'room' or 'node_id' required".to_string());
                };

                let client = reqwest::Client::new();
                let mut errors = Vec::new();
                for nid in &node_ids {
                    if let Err(e) = matter_command(
                        &client,
                        "device_command",
                        json!({
                            "node_id": nid,
                            "endpoint_id": 1,
                            "cluster_id": CLUSTER_ON_OFF,
                            "command_name": cmd,
                            "payload": {}
                        }),
                    ).await {
                        let label = device_label(&mapping, *nid)
                            .unwrap_or_else(|| format!("node {}", nid));
                        errors.push(format!("{}: {}", label, e));
                    }
                }

                let target = if !room.is_empty() {
                    format!("{} ({} switch{})", room, node_ids.len(), if node_ids.len() == 1 { "" } else { "es" })
                } else {
                    device_label(&mapping, node_ids[0])
                        .unwrap_or_else(|| format!("node {}", node_ids[0]))
                };

                if errors.is_empty() {
                    info!("[matter] {} {}", cmd, target);
                    Ok(RichToolResult::text(format!("Turned {} {}.", action, target)))
                } else {
                    warn!("[matter] {} {}: {} errors", cmd, target, errors.len());
                    Ok(RichToolResult::text(format!(
                        "Turned {} {} but {} device{} failed: {}",
                        action, target, errors.len(),
                        if errors.len() == 1 { "" } else { "s" },
                        errors.join("; ")
                    )))
                }
            }
            "brightness" => {
                let room = args.get("room").and_then(|v| v.as_str()).unwrap_or("");
                let level = args.get("value").and_then(|v| v.as_u64()).unwrap_or(128);
                let level = level.min(254) as u8;

                // Target bulbs for brightness (not switches — switches don't dim)
                let node_ids = if !room.is_empty() {
                    let bulbs = resolve_room_bulbs(room).unwrap_or_default();
                    if !bulbs.is_empty() {
                        bulbs
                    } else {
                        // Fallback to switches if no bulbs defined
                        resolve_room(room).ok_or_else(|| format!("matter: unknown room '{}'", room))?
                    }
                } else if let Some(nid) = args.get("node_id").and_then(|v| v.as_u64()) {
                    vec![nid]
                } else {
                    return Err("matter: either 'room' or 'node_id' required".to_string());
                };

                let client = reqwest::Client::new();
                for nid in &node_ids {
                    let _ = matter_command(
                        &client,
                        "device_command",
                        json!({
                            "node_id": nid,
                            "endpoint_id": 1,
                            "cluster_id": CLUSTER_LEVEL_CONTROL,
                            "command_name": "MoveToLevelWithOnOff",
                            "payload": {"level": level, "transitionTime": 0}
                        }),
                    ).await;
                }

                let target = if !room.is_empty() { room.to_string() } else { format!("node {}", node_ids[0]) };
                info!("[matter] brightness {} = {}", target, level);
                Ok(RichToolResult::text(format!(
                    "Set {} brightness to {:.0}%.",
                    target,
                    (level as f64 / 254.0) * 100.0
                )))
            }
            "color_temp" => {
                let room = args.get("room").and_then(|v| v.as_str()).unwrap_or("");
                let kelvin = args.get("value").and_then(|v| v.as_u64()).unwrap_or(4000);
                let mireds = (1_000_000 / kelvin.max(1)) as u16;

                // Target bulbs for color_temp (not switches)
                let node_ids = if !room.is_empty() {
                    let bulbs = resolve_room_bulbs(room).unwrap_or_default();
                    if !bulbs.is_empty() {
                        bulbs
                    } else {
                        resolve_room(room).ok_or_else(|| format!("matter: unknown room '{}'", room))?
                    }
                } else if let Some(nid) = args.get("node_id").and_then(|v| v.as_u64()) {
                    vec![nid]
                } else {
                    return Err("matter: either 'room' or 'node_id' required".to_string());
                };

                let client = reqwest::Client::new();
                for nid in &node_ids {
                    let _ = matter_command(
                        &client,
                        "device_command",
                        json!({
                            "node_id": nid,
                            "endpoint_id": 1,
                            "cluster_id": CLUSTER_COLOR_CONTROL,
                            "command_name": "MoveToColorTemperature",
                            "payload": {"colorTemperatureMireds": mireds, "transitionTime": 0}
                        }),
                    ).await;
                }

                let target = if !room.is_empty() { room.to_string() } else { format!("node {}", node_ids[0]) };
                info!("[matter] color_temp {} = {}K ({}mireds)", target, kelvin, mireds);
                Ok(RichToolResult::text(format!(
                    "Set {} color temperature to {}K.",
                    target, kelvin
                )))
            }
            "status" => {
                // Status can work by room (all devices) or by node_id
                let room = args.get("room").and_then(|v| v.as_str()).unwrap_or("");
                let mapping = load_room_mapping().unwrap_or_default();

                let node_ids: Vec<u64> = if !room.is_empty() {
                    resolve_room(room).ok_or_else(|| format!("matter: unknown room '{}'", room))?
                } else if let Some(nid) = args.get("node_id").and_then(|v| v.as_u64()) {
                    vec![nid]
                } else {
                    return Err("matter: either 'room' or 'node_id' required for status".to_string());
                };

                let client = reqwest::Client::new();
                let mut parts = Vec::new();
                for nid in &node_ids {
                    let result = matter_command(
                        &client,
                        "get_node",
                        json!({"node_id": nid}),
                    ).await;

                    let label = device_label(&mapping, *nid)
                        .unwrap_or_else(|| format!("Node {}", nid));

                    match result {
                        Ok(node) => {
                            let attrs = node.get("attributes").cloned().unwrap_or_default();
                            let on_off = attrs.get("1/6/0").and_then(|v| v.as_bool());
                            let level = attrs.get("1/8/0").and_then(|v| v.as_u64());
                            let avail = node.get("available").and_then(|v| v.as_bool()).unwrap_or(false);

                            let state = if !avail {
                                "OFFLINE".to_string()
                            } else {
                                match on_off {
                                    Some(true) => "on".to_string(),
                                    Some(false) => "off".to_string(),
                                    None => "unknown".to_string(),
                                }
                            };
                            let mut line = format!("{}: {}", label, state);
                            if let Some(lvl) = level {
                                line.push_str(&format!(", brightness {:.0}%", (lvl as f64 / 254.0) * 100.0));
                            }
                            parts.push(line);
                        }
                        Err(e) => parts.push(format!("{}: error ({})", label, e)),
                    }
                }

                Ok(RichToolResult::text(parts.join("\n")))
            }
            other => Err(format!("matter: unknown action '{}'", other)),
        }
    }
}
