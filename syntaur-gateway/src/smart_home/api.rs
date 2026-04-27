//! `/api/smart-home/*` HTTP handlers. Axum surface for the dashboard JS.
//!
//! Track A week 1 ships thin scaffolding: the room + device CRUD is live
//! (talks to the v57 tables), scan returns an empty report, automation
//! endpoints return 501-style payloads. Each endpoint gets filled in as
//! its track reaches the relevant week.
//!
//! DB access follows the canonical pattern in `music.rs` — clone the
//! path out of `AppState`, run `rusqlite::Connection::open` inside
//! `spawn_blocking` so the tokio runtime isn't blocked on SQLite.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use futures_util::Stream;
use std::convert::Infallible;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

use super::{
    automation::{Action, AutomationSpec},
    devices, diagnostics, energy, rooms, scan,
};

// ── helpers ─────────────────────────────────────────────────────────────

/// Principal placeholder — real auth is threaded in once the mutation
/// endpoints start doing anything consequential. Single-user installs
/// default to admin (user_id = 1) so `/smart-home` renders on first
/// boot without tripping over the missing session context.
fn current_user_id(_state: &std::sync::Arc<AppState>) -> i64 {
    1
}

fn err_500(msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    let s = msg.into();
    log::error!("[smart_home::api] {}", s);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": s })),
    )
}

// ── rooms ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct RoomCreateBody {
    pub name: String,
    pub zone: Option<String>,
}

pub async fn handle_list_rooms(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let rooms = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = rusqlite::Connection::open(&db)?;
        rooms::list_for_user(&conn, user_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "rooms": rooms })))
}

pub async fn handle_create_room(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<RoomCreateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let id = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        rooms::create(&conn, user_id, &body.name, body.zone.as_deref())
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "id": id })))
}

pub async fn handle_delete_room(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        rooms::delete(&conn, user_id, id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "deleted": n })))
}

#[derive(Debug, Deserialize)]
pub struct RoomPatchBody {
    pub name: Option<String>,
    pub sort_order: Option<i64>,
}

pub async fn handle_patch_room(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<RoomPatchBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let updated = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut touched = 0;
        if let Some(name) = body.name.as_deref() {
            touched += rooms::rename(&conn, user_id, id, name)?;
        }
        if let Some(sort_order) = body.sort_order {
            touched += rooms::set_sort_order(&conn, user_id, id, sort_order)?;
        }
        Ok(touched)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "updated": updated })))
}

// ── devices ─────────────────────────────────────────────────────────────

pub async fn handle_list_devices(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let devs = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = rusqlite::Connection::open(&db)?;
        devices::list_for_user(&conn, user_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "devices": devs })))
}

#[derive(Debug, Deserialize)]
pub struct AssignRoomBody {
    pub room_id: Option<i64>,
}

pub async fn handle_assign_device_room(
    State(state): State<std::sync::Arc<AppState>>,
    Path(device_id): Path<i64>,
    Json(body): Json<AssignRoomBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        devices::assign_room(&conn, user_id, device_id, body.room_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "updated": n })))
}

/// DELETE /api/smart-home/devices/{id} — remove a device. Publishes a
/// `SmartHomeEvent::DeviceRemoved` on success so the HA Discovery
/// publisher can purge the retained config topic. Idempotent — deleting
/// a missing device returns 404 without an event.
pub async fn handle_delete_device(
    State(state): State<std::sync::Arc<AppState>>,
    Path(device_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    // Read the device first so we can carry `kind` in the event
    // without forcing subscribers to do a second DB read after the
    // row is gone.
    let device = {
        let db = db.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
            let conn = rusqlite::Connection::open(&db)?;
            devices::get(&conn, user_id, device_id)
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .map_err(|e| err_500(format!("db error: {e}")))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "device not found" })),
            )
        })?
    };

    let deleted = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "DELETE FROM smart_home_devices WHERE user_id = ? AND id = ?",
            rusqlite::params![user_id, device_id],
        )
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;

    if deleted > 0 {
        crate::smart_home::events::publish(
            crate::smart_home::events::SmartHomeEvent::DeviceRemoved {
                user_id,
                device_id,
                kind: device.kind.clone(),
            },
        );
    }

    Ok(Json(json!({ "deleted": deleted })))
}

// ── scan ────────────────────────────────────────────────────────────────

pub async fn handle_scan(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let report = scan::run(user_id).await;
    Ok(Json(json!(report)))
}

/// POST /api/smart-home/scan/confirm — user confirmed a candidate from
/// the scan results. Upserts the candidate into smart_home_devices and
/// optionally assigns to a room. Idempotent on (user, driver, external_id).
#[derive(Debug, Deserialize)]
pub struct ScanConfirmBody {
    pub candidate: scan::ScanCandidate,
    pub room_id: Option<i64>,
    /// Optional user-supplied name override (the scanner's guess is
    /// often an IP or an opaque fullname; the user may type something
    /// friendlier in the card).
    pub name_override: Option<String>,
}

pub async fn handle_scan_confirm(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<ScanConfirmBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let device = tokio::task::spawn_blocking(move || -> rusqlite::Result<devices::Device> {
        let conn = rusqlite::Connection::open(&db)?;
        let c = &body.candidate;

        // Fold (vendor, ip, mac, details) into a single metadata blob so
        // nothing from the scan card is lost across the confirm boundary.
        let mut metadata = serde_json::Map::new();
        if let Some(v) = &c.vendor {
            metadata.insert("vendor".into(), serde_json::Value::String(v.clone()));
        }
        if let Some(ip) = &c.ip {
            metadata.insert("ip".into(), serde_json::Value::String(ip.clone()));
        }
        if let Some(mac) = &c.mac {
            metadata.insert("mac".into(), serde_json::Value::String(mac.clone()));
        }
        metadata.insert("scan_details".into(), c.details.clone());
        let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());

        let chosen_name = body
            .name_override
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| c.name.clone());

        let id = devices::upsert_from_scan(
            &conn,
            user_id,
            &c.driver,
            &c.external_id,
            &chosen_name,
            &c.kind,
            "{}", // capabilities — drivers fill this in on first successful control call
            &metadata_json,
        )?;

        if let Some(room_id) = body.room_id {
            devices::assign_room(&conn, user_id, id, Some(room_id))?;
        }

        // Re-read the canonical row so the UI gets the full Device shape
        // (including the room_id we may have just written).
        devices::get(&conn, user_id, id)?.ok_or_else(|| {
            rusqlite::Error::QueryReturnedNoRows
        })
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;

    // Phase 2C: kick off best-effort capability discovery in the
    // background for freshly-confirmed Matter devices. Doesn't gate
    // the HTTP response — discovery takes a few seconds (mDNS browse
    // + CASE handshake + ~30 attribute reads) and the user has
    // already moved on by the time it lands. Failures log and bail;
    // the user can re-trigger via POST /api/smart-home/devices/{id}/discover-caps.
    if device.driver == "matter" {
        if let Some(node_id) = device
            .external_id
            .strip_prefix("node:")
            .and_then(|s| s.parse::<u64>().ok())
        {
            spawn_auto_discover_caps(state.db_path.clone(), user_id, device.id, node_id);
        }
    }

    Ok(Json(json!({ "device": device })))
}

/// Best-effort background capability discovery for a freshly
/// commissioned Matter device. Spawned by `handle_scan_confirm` after
/// the device row lands. Does not block the caller; failures are
/// logged and silently swallowed so a missing fabric (or a flaky
/// device) doesn't break the confirm flow. Users can always retry
/// manually via POST `/api/smart-home/devices/{id}/discover-caps`.
fn spawn_auto_discover_caps(
    db_path: std::path::PathBuf,
    user_id: i64,
    device_id: i64,
    node_id: u64,
) {
    tokio::spawn(async move {
        // Brief settle period — newly commissioned devices sometimes
        // close their PASE side and reopen on operational addresses
        // a beat later. 2 s is enough to win most races without
        // making the user wait.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let fabric = match tokio::task::spawn_blocking(|| {
            let fabrics = syntaur_matter::list_fabrics()?;
            match fabrics.as_slice() {
                [single] => syntaur_matter::load_fabric(&single.label),
                [] => Err(syntaur_matter::MatterFabricError::Matter(
                    "no fabric configured".into(),
                )),
                _ => Err(syntaur_matter::MatterFabricError::Matter(
                    "multiple fabrics — auto-discover skipped, use explicit endpoint".into(),
                )),
            }
        })
        .await
        {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                log::info!(
                    "[auto-caps] device {device_id}: skipping (fabric resolve: {e:?})"
                );
                return;
            }
            Err(e) => {
                log::warn!("[auto-caps] device {device_id}: join error: {e}");
                return;
            }
        };

        log::info!(
            "[auto-caps] device {device_id} (node {node_id:#x}) on fabric {} — discovering",
            fabric.label
        );
        let caps = match syntaur_matter_ble::discover_capabilities_for_node(
            &fabric, node_id, None,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "[auto-caps] device {device_id}: discovery failed: {e:?} (user can retry via POST /api/smart-home/devices/{device_id}/discover-caps)"
                );
                return;
            }
        };

        let persisted = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
            let conn = rusqlite::Connection::open(&db_path)?;
            devices::set_capabilities(&conn, user_id, device_id, &caps)
        })
        .await;
        match persisted {
            Ok(Ok(_)) => log::info!("[auto-caps] device {device_id}: persisted"),
            Ok(Err(e)) => log::warn!("[auto-caps] device {device_id}: persist failed: {e}"),
            Err(e) => log::warn!("[auto-caps] device {device_id}: persist join error: {e}"),
        }
    });
}

// ── control ─────────────────────────────────────────────────────────────

/// Desired-state patch from the UI. Clients send the fields they want
/// changed — {on, level, locked, setpoint, color_temp_kelvin} etc.
/// Dispatch happens by `device.driver`. v1 wires `matter` through the
/// legacy bridge; other drivers return 501 with an upgrade hint until
/// their Track A week lands.
#[derive(Debug, Deserialize)]
pub struct ControlBody {
    pub device_id: i64,
    pub state: serde_json::Value,
}

pub async fn handle_control(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<ControlBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    // Look up the device so we know its driver + external_id before
    // dispatching. Not in the critical path, cheap query.
    let device_id = body.device_id;
    let device = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
        let conn = rusqlite::Connection::open(&db)?;
        devices::get(&conn, user_id, device_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "device not found" })),
        )
    })?;

    match device.driver.as_str() {
        "matter" => control_matter(&device, &body.state).await,
        "mqtt" => control_mqtt(&device, &body.state).await,
        other => Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": format!("driver '{}' not wired yet", other),
                "hint": "Non-Matter/MQTT driver control lands per the plan calendar (Zigbee week 5, BLE week 7, cameras week 9, cloud adapters week 11, Z-Wave week 13)."
            })),
        )),
    }
}

async fn control_mqtt(
    device: &devices::Device,
    desired: &serde_json::Value,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let dispatched = crate::smart_home::drivers::mqtt::dispatch_command(
        device.user_id,
        device.id,
        desired,
    )
    .await
    .map_err(|e| {
        // No installed supervisor / driver mismatch → 501 so the UI can
        // hint at setup. Everything else → 500 as a real dispatch fault.
        if e.contains("not installed") {
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "error": "mqtt supervisor not running",
                    "hint": "Add an MQTT broker in settings → smart_home_credentials (provider='mqtt')."
                })),
            )
        } else {
            err_500(format!("mqtt control: {e}"))
        }
    })?;

    // Optimistic state echo — automation + dashboard update without
    // waiting for the broker's own retained state publish to come back.
    crate::smart_home::events::publish(
        crate::smart_home::events::SmartHomeEvent::DeviceStateChanged {
            user_id: device.user_id,
            device_id: device.id,
            state: desired.clone(),
            source: "mqtt".to_string(),
        },
    );
    Ok(Json(json!({
        "ok": true,
        "driver": "mqtt",
        "dispatched": dispatched
    })))
}

async fn control_matter(
    device: &devices::Device,
    desired: &serde_json::Value,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // external_id is "node:<u64>" per drivers::matter::candidate_from_node.
    let node_id: u64 = device
        .external_id
        .strip_prefix("node:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("device external_id '{}' is not a Matter node reference", device.external_id)
                })),
            )
        })?;

    // Dispatch fields in a deterministic order: on/off first (so
    // MoveToLevelWithOnOff doesn't fight with an explicit `on: true`),
    // then level, then color_temp_kelvin, then setpoint/locked (not
    // wired for Matter yet).
    if let Some(on) = desired.get("on").and_then(|v| v.as_bool()) {
        crate::tools::matter::set_onoff(node_id, on)
            .await
            .map_err(|e| err_500(format!("matter set_onoff: {e}")))?;
    }
    if let Some(level) = desired.get("level").and_then(|v| v.as_f64()) {
        crate::tools::matter::set_level(node_id, level)
            .await
            .map_err(|e| err_500(format!("matter set_level: {e}")))?;
    }
    if let Some(kelvin) = desired.get("color_temp_kelvin").and_then(|v| v.as_u64()) {
        crate::tools::matter::set_color_temp_kelvin(node_id, kelvin as u32)
            .await
            .map_err(|e| err_500(format!("matter set_color_temp: {e}")))?;
    }
    if desired.get("locked").is_some() {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": "Matter lock control not wired in v1 (requires PIN handling; slated for v1.1 Controller)."
            })),
        ));
    }
    if desired.get("setpoint").is_some() {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": "Matter thermostat setpoint not wired in v1 (slated for v1.1 Controller)."
            })),
        ));
    }
    // Announce the state change so the automation engine's reactive
    // DeviceState trigger path + dashboard SSE consumers update.
    crate::smart_home::events::publish(
        crate::smart_home::events::SmartHomeEvent::DeviceStateChanged {
            user_id: device.user_id,
            device_id: device.id,
            state: desired.clone(),
            source: "matter".to_string(),
        },
    );
    Ok(Json(json!({ "ok": true, "node_id": node_id })))
}

/// POST /api/smart-home/devices/{id}/refresh-state — pull the live
/// state from whichever driver owns the device, persist it into
/// `smart_home_devices.state_json`, return the updated Device.
pub async fn handle_refresh_state(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    // 1. Read the device so we know which driver to poll.
    let device = {
        let db = db.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
            let conn = rusqlite::Connection::open(&db)?;
            devices::get(&conn, user_id, id)
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .map_err(|e| err_500(format!("db error: {e}")))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "device not found" })),
            )
        })?
    };

    // 2. Fetch fresh state from the driver. Matter is the only wired
    // driver in v1; others return 501 here too.
    let fresh_state = match device.driver.as_str() {
        "matter" => {
            let node_id: u64 = device
                .external_id
                .strip_prefix("node:")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "invalid Matter external_id" })),
                    )
                })?;
            crate::tools::matter::get_node_state(node_id)
                .await
                .map_err(|e| err_500(format!("matter get_node_state: {e}")))?
        }
        other => {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "error": format!("refresh-state: driver '{}' not wired yet", other)
                })),
            ));
        }
    };

    // 3. Persist + return the updated row.
    let state_json = serde_json::to_string(&fresh_state).unwrap_or_else(|_| "{}".to_string());
    let updated = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "UPDATE smart_home_devices
                SET state_json = ?, last_seen_at = ?
              WHERE user_id = ? AND id = ?",
            rusqlite::params![
                state_json,
                chrono::Utc::now().timestamp(),
                user_id,
                id
            ],
        )?;
        devices::get(&conn, user_id, id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;

    // Fresh state from the driver counts as a state transition —
    // announce for the reactive DeviceState trigger path.
    crate::smart_home::events::publish(
        crate::smart_home::events::SmartHomeEvent::DeviceStateChanged {
            user_id,
            device_id: id,
            state: fresh_state.clone(),
            source: device.driver.clone(),
        },
    );
    Ok(Json(json!({ "device": updated })))
}

/// POST /api/smart-home/devices/{id}/discover-caps — open a CASE
/// session against a Matter device's commissioned node, walk the
/// Descriptor cluster on every endpoint, and persist the resulting
/// `DeviceCapabilities` JSON on the device row. The Smart Home tile
/// UI and agent tool surface read `capabilities_json` to scope which
/// controls render and which tools the agent is offered for the
/// device.
///
/// Resolves the operational fabric automatically when the install has
/// exactly one fabric (the common case). Multi-fabric installs must
/// pass `fabric_label` in the body. `addr` is an optional `IP:PORT`
/// override that skips mDNS — useful when the device sits on a
/// different VLAN than the gateway and mDNS doesn't reflect across
/// subnets.
///
/// Driver-gated: only Matter devices are wired in v1; other drivers
/// return 501.
pub async fn handle_discover_caps(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    body: Option<Json<DiscoverCapsBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let device = {
        let db = db.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
            let conn = rusqlite::Connection::open(&db)?;
            devices::get(&conn, user_id, id)
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .map_err(|e| err_500(format!("db error: {e}")))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "device not found" })),
            )
        })?
    };

    if device.driver != "matter" {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": format!(
                    "discover-caps: driver '{}' not wired (Matter only in v1)",
                    device.driver
                )
            })),
        ));
    }

    let node_id: u64 = device
        .external_id
        .strip_prefix("node:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "device external_id '{}' is not a Matter node reference",
                        device.external_id
                    )
                })),
            )
        })?;

    // Fabric resolution + load both touch the disk. Off the async
    // executor so we don't stall the runtime on filesystem I/O.
    let fabric_label_hint = body.fabric_label.clone();
    let (fabric_label, fabric) =
        tokio::task::spawn_blocking(move || -> Result<(String, syntaur_matter::FabricHandle), (StatusCode, Json<serde_json::Value>)> {
            let label = match fabric_label_hint {
                Some(l) => l,
                None => {
                    let fabrics = syntaur_matter::list_fabrics().map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": format!("list fabrics: {e:?}") })),
                        )
                    })?;
                    match fabrics.as_slice() {
                        [single] => single.label.clone(),
                        [] => {
                            return Err((
                                StatusCode::FAILED_DEPENDENCY,
                                Json(json!({
                                    "error": "no Matter fabric configured. Run POST /api/smart-home/matter/fabric/init first."
                                })),
                            ));
                        }
                        _ => {
                            return Err((
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "error": "multiple fabrics present; pass fabric_label in body to disambiguate",
                                    "fabrics": fabrics.iter().map(|f| f.label.clone()).collect::<Vec<_>>(),
                                })),
                            ));
                        }
                    }
                }
            };
            let fabric = syntaur_matter::load_fabric(&label).map_err(|e| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": format!("load fabric {label}: {e:?}") })),
                )
            })?;
            Ok((label, fabric))
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))??;

    let addr_override = match body.addr.as_deref() {
        Some(s) => Some(s.parse::<std::net::SocketAddr>().map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("addr '{s}' parse: {e}") })),
            )
        })?),
        None => None,
    };

    log::info!(
        "[discover-caps] device {} (node {:#x}) on fabric {} (addr_override={:?})",
        id, node_id, fabric_label, addr_override
    );

    let caps = syntaur_matter_ble::discover_capabilities_for_node(&fabric, node_id, addr_override)
        .await
        .map_err(|e| err_500(format!("discover_capabilities_for_node: {e:?}")))?;

    let caps_for_persist = caps.clone();
    let updated = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<devices::Device>> {
        let conn = rusqlite::Connection::open(&db)?;
        devices::set_capabilities(&conn, user_id, id, &caps_for_persist)?;
        devices::get(&conn, user_id, id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;

    Ok(Json(json!({
        "ok": true,
        "device": updated,
        "capabilities": caps,
        "human": caps.render_human(),
    })))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiscoverCapsBody {
    /// Multi-fabric installs disambiguate here. Single-fabric installs
    /// can omit; we'll resolve via `list_fabrics()`.
    #[serde(default)]
    pub fabric_label: Option<String>,
    /// Optional `IP:PORT` override that skips operational mDNS.
    /// Standard Matter port is 5540. Useful cross-VLAN.
    #[serde(default)]
    pub addr: Option<String>,
}

// ── automation (CRUD — plan Week 6/10 milestone) ────────────────────────
//
// Stores the canonical AST in `smart_home_automations.spec_json` so the
// long-running engine (`smart_home::automation::AutomationEngine::spawn`)
// can pick up enable/disable toggles and schema edits on its next tick
// without restarting. The NL-compile path (`/automation/compile`) still
// returns 501 — the LLM round-trip lands in its own milestone and is
// orthogonal to the visual builder.

pub async fn handle_list_automations(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let rows = tokio::task::spawn_blocking(move || -> rusqlite::Result<serde_json::Value> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, source, spec_json, enabled,
                    last_run_at, last_run_status, last_run_error, created_at
               FROM smart_home_automations
              WHERE user_id = ?
              ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id], |r| {
            let id: i64 = r.get(0)?;
            let name: String = r.get(1)?;
            let description: Option<String> = r.get(2)?;
            let source: String = r.get(3)?;
            let spec_json: String = r.get(4)?;
            let enabled: i64 = r.get(5)?;
            let last_run_at: Option<i64> = r.get(6)?;
            let last_run_status: Option<String> = r.get(7)?;
            let last_run_error: Option<String> = r.get(8)?;
            let created_at: i64 = r.get(9)?;
            let spec: serde_json::Value =
                serde_json::from_str(&spec_json).unwrap_or_else(|_| serde_json::json!({}));
            Ok(json!({
                "id": id,
                "name": name,
                "description": description,
                "source": source,
                "spec": spec,
                "enabled": enabled == 1,
                "last_run_at": last_run_at,
                "last_run_status": last_run_status,
                "last_run_error": last_run_error,
                "created_at": created_at,
            }))
        })?;
        let all: Vec<serde_json::Value> = rows.filter_map(Result::ok).collect();
        Ok(serde_json::Value::Array(all))
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "automations": rows })))
}

/// POST /api/smart-home/automation/compile — natural-language → AST.
/// Returns a preview (summary + spec + warnings) that the UI renders
/// in the builder; explicit POST /automations is still required to
/// persist.
pub async fn handle_compile_automation(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<super::nl_automation::CompileRequest>,
) -> Result<Json<super::nl_automation::CompilePreview>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    match super::nl_automation::compile(user_id, db, body).await {
        Ok(preview) => Ok(Json(preview)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct AutomationCreateBody {
    pub name: String,
    /// `visual` (builder), `nl` (NL-compile output), `imported`.
    /// Defaults to `visual` when empty.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub description: Option<String>,
    pub spec: AutomationSpec,
}

pub async fn handle_create_automation(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<AutomationCreateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    if body.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name must not be empty" })),
        ));
    }
    // Block rules the engine can never act on — it's better to surface
    // "invalid automation" at create time than to let it silently never
    // fire. At least one trigger + one action is the minimum shape.
    if body.spec.triggers.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "at least one trigger is required" })),
        ));
    }
    if body.spec.actions.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "at least one action is required" })),
        ));
    }

    let source = match body.source.trim() {
        "" => "visual".to_string(),
        s @ ("visual" | "nl" | "imported") => s.to_string(),
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("source must be visual|nl|imported, got `{other}`"),
                })),
            ));
        }
    };
    let spec_json = serde_json::to_string(&body.spec)
        .map_err(|e| err_500(format!("serialize spec: {e}")))?;
    let name = body.name;
    let description = body.description;

    let id = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO smart_home_automations
                (user_id, name, description, source, nl_prompt, spec_json,
                 enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, NULL, ?, 1, ?, ?)",
            rusqlite::params![
                user_id, name, description, source, spec_json, now, now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "id": id })))
}

pub async fn handle_delete_automation(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "DELETE FROM smart_home_automations WHERE user_id = ? AND id = ?",
            rusqlite::params![user_id, id],
        )
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    if n == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "automation not found" })),
        ));
    }
    Ok(Json(json!({ "deleted": n })))
}

#[derive(Debug, Deserialize)]
pub struct AutomationToggleBody {
    pub enabled: bool,
}

/// POST /api/smart-home/automations/{id}/toggle — flip the enabled bit.
/// Cheaper than a full UPDATE with the whole body, and the common case
/// (user toggling a tile from the builder list) doesn't need a PUT.
pub async fn handle_toggle_automation(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<AutomationToggleBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let enabled_int: i64 = if body.enabled { 1 } else { 0 };
    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE smart_home_automations SET enabled = ?, updated_at = ?
             WHERE user_id = ? AND id = ?",
            rusqlite::params![enabled_int, now, user_id, id],
        )
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    if n == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "automation not found" })),
        ));
    }
    Ok(Json(json!({ "updated": n, "enabled": body.enabled })))
}

/// PUT /api/smart-home/automations/{id} — edit-in-place. Updates name,
/// description, and spec together; the toggle endpoint still handles
/// enable/disable solo so the common-case tile click doesn't need a
/// full body. Same validation as create — no empty name, ≥1 trigger,
/// ≥1 action, source whitelist.
pub async fn handle_update_automation(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<AutomationCreateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    if body.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name must not be empty" })),
        ));
    }
    if body.spec.triggers.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "at least one trigger is required" })),
        ));
    }
    if body.spec.actions.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "at least one action is required" })),
        ));
    }

    let source = match body.source.trim() {
        "" => "visual".to_string(),
        s @ ("visual" | "nl" | "imported") => s.to_string(),
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("source must be visual|nl|imported, got `{other}`"),
                })),
            ));
        }
    };
    let spec_json = serde_json::to_string(&body.spec)
        .map_err(|e| err_500(format!("serialize spec: {e}")))?;
    let name = body.name;
    let description = body.description;

    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE smart_home_automations
                SET name = ?, description = ?, source = ?, spec_json = ?, updated_at = ?
              WHERE user_id = ? AND id = ?",
            rusqlite::params![name, description, source, spec_json, now, user_id, id],
        )
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    if n == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "automation not found" })),
        ));
    }
    Ok(Json(json!({ "updated": n })))
}

// ── scenes ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SceneCreateBody {
    pub name: String,
    pub icon: Option<String>,
    pub actions: Vec<Action>,
    pub room_id: Option<i64>,
}

pub async fn handle_list_scenes(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let scenes = tokio::task::spawn_blocking(move || -> rusqlite::Result<serde_json::Value> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, icon, actions_json, room_id, created_at
               FROM smart_home_scenes
              WHERE user_id = ? ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id], |row| {
            let id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let icon: Option<String> = row.get(2)?;
            let actions_json: String = row.get(3)?;
            let room_id: Option<i64> = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let actions: serde_json::Value =
                serde_json::from_str(&actions_json).unwrap_or_else(|_| serde_json::json!([]));
            Ok(serde_json::json!({
                "id": id,
                "name": name,
                "icon": icon,
                "actions": actions,
                "room_id": room_id,
                "created_at": created_at,
            }))
        })?;
        let all: Vec<serde_json::Value> = rows.filter_map(Result::ok).collect();
        Ok(serde_json::Value::Array(all))
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "scenes": scenes })))
}

pub async fn handle_create_scene(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<SceneCreateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let actions_json = serde_json::to_string(&body.actions)
        .map_err(|e| err_500(format!("serialize actions: {e}")))?;
    let id = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = rusqlite::Connection::open(&db)?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO smart_home_scenes
                (user_id, name, icon, actions_json, room_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                user_id,
                body.name,
                body.icon,
                actions_json,
                body.room_id,
                now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "id": id })))
}

pub async fn handle_delete_scene(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let n = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute(
            "DELETE FROM smart_home_scenes WHERE user_id = ? AND id = ?",
            rusqlite::params![user_id, id],
        )
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!({ "deleted": n })))
}

/// POST /api/smart-home/scenes/{id}/activate — execute the scene's
/// actions through the normal control dispatch. Returns per-action
/// outcomes so the UI can flag partial failures.
pub async fn handle_activate_scene(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    // Load the scene's action list.
    let actions: Vec<Action> = {
        let db = db.clone();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<String> {
            let conn = rusqlite::Connection::open(&db)?;
            conn.query_row(
                "SELECT actions_json FROM smart_home_scenes WHERE user_id = ? AND id = ?",
                rusqlite::params![user_id, id],
                |row| row.get::<_, String>(0),
            )
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "scene not found" })),
            ),
            other => err_500(format!("db error: {other}")),
        })
        .and_then(|s| {
            serde_json::from_str::<Vec<Action>>(&s).map_err(|e| err_500(format!("parse actions: {e}")))
        })?
    };

    // Dispatch each action. `SetDevice` actions route through the same
    // /api/smart-home/control path so Matter-backed devices actually
    // change state; non-Matter drivers return a "not wired yet" note
    // but the scene still reports per-action success/failure.
    let mut outcomes: Vec<serde_json::Value> = Vec::new();
    let mut failed = 0usize;
    for action in &actions {
        let result = activate_action(&state, user_id, action).await;
        if result.is_err() {
            failed += 1;
        }
        outcomes.push(match result {
            Ok(note) => json!({ "ok": true, "note": note }),
            Err(e) => json!({ "ok": false, "error": e }),
        });
    }
    crate::smart_home::events::publish(
        crate::smart_home::events::SmartHomeEvent::SceneActivated {
            user_id,
            scene_id: id,
            failed,
        },
    );
    Ok(Json(json!({ "scene_id": id, "outcomes": outcomes })))
}

async fn activate_action(
    state: &std::sync::Arc<AppState>,
    user_id: i64,
    action: &Action,
) -> Result<String, String> {
    match action {
        Action::SetDevice { device_id, state: desired } => {
            // Look up driver + external_id.
            let db = state.db_path.clone();
            let device_id = *device_id;
            let info = tokio::task::spawn_blocking(move || -> rusqlite::Result<(String, String)> {
                let conn = rusqlite::Connection::open(&db)?;
                conn.query_row(
                    "SELECT driver, external_id FROM smart_home_devices
                      WHERE user_id = ? AND id = ?",
                    rusqlite::params![user_id, device_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .map_err(|e| format!("join: {e}"))?
            .map_err(|e| format!("db: {e}"))?;

            match info.0.as_str() {
                "matter" => {
                    let node_id: u64 = info
                        .1
                        .strip_prefix("node:")
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| "bad matter external_id".to_string())?;
                    if let Some(on) = desired.get("on").and_then(|v| v.as_bool()) {
                        crate::tools::matter::set_onoff(node_id, on).await?;
                    }
                    if let Some(level) = desired.get("level").and_then(|v| v.as_f64()) {
                        crate::tools::matter::set_level(node_id, level).await?;
                    }
                    if let Some(kelvin) =
                        desired.get("color_temp_kelvin").and_then(|v| v.as_u64())
                    {
                        crate::tools::matter::set_color_temp_kelvin(node_id, kelvin as u32)
                            .await?;
                    }
                    Ok(format!("matter device {} updated", device_id))
                }
                other => Err(format!(
                    "driver '{}' not wired yet — device {} skipped",
                    other, device_id
                )),
            }
        }
        Action::Scene { scene_id } => Err(format!(
            "nested scenes not supported (would activate {})",
            scene_id
        )),
        Action::Notify { target, text } => {
            log::info!(
                "[smart_home::scene] notify target={} text={} user_id={}",
                target,
                text,
                user_id
            );
            Ok("notify logged".into())
        }
        Action::Delay { seconds } => {
            tokio::time::sleep(std::time::Duration::from_secs(*seconds as u64)).await;
            Ok(format!("delayed {}s", seconds))
        }
    }
}

// ── diagnostics ─────────────────────────────────────────────────────────

/// GET /api/smart-home/diagnostics/summary — total + online + offline
/// device counts plus the top active-issue list for the dashboard.
pub async fn handle_diagnostics_summary(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let summary = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = rusqlite::Connection::open(&db)?;
        diagnostics::summary_for_user(&conn, user_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!(summary)))
}

/// GET /api/smart-home/diagnostics/mqtt — MQTT supervisor observability.
/// Per-session counters (reconnects, messages in/out, per-dialect
/// histogram) + aggregate StateCache stats (updates_received,
/// diffs_emitted, availability transitions, bridge events). Returns an
/// empty snapshot when the supervisor isn't installed — the caller
/// UI should render "MQTT driver not enabled" rather than error.
pub async fn handle_diagnostics_mqtt(
    State(_state): State<std::sync::Arc<AppState>>,
) -> Json<serde_json::Value> {
    let snap = crate::smart_home::drivers::mqtt::stats_snapshot().await;
    Json(json!(snap))
}

/// GET /api/smart-home/cameras/events — proxy Frigate's recent
/// detections into the smart_home surface. Query params: ?camera=<name>
/// &limit=<n>. All events across all cameras when `camera` omitted.
#[derive(Debug, Deserialize)]
pub struct CameraEventsQuery {
    pub camera: Option<String>,
    pub limit: Option<u32>,
}

pub async fn handle_camera_events(
    State(_state): State<std::sync::Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<CameraEventsQuery>,
) -> Json<serde_json::Value> {
    let events =
        crate::smart_home::drivers::camera::recent_events(q.camera.as_deref(), q.limit.unwrap_or(50))
            .await;
    Json(json!({ "events": events }))
}

/// GET /api/smart-home/energy/summary — today's kWh + per-device
/// breakdown + cost/carbon if a rate is configured.
pub async fn handle_energy_summary(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let summary = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = rusqlite::Connection::open(&db)?;
        energy::summary_for_user(&conn, user_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;
    Ok(Json(json!(summary)))
}

/// POST /api/smart-home/energy/ingest — force an immediate ingest pass.
/// Useful for the "Refresh" button on the energy dashboard.
pub async fn handle_energy_ingest(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine =
        std::sync::Arc::new(energy::EnergyEngine::new(state.db_path.clone()));
    let stored = engine
        .ingest_once()
        .await
        .map_err(|e| err_500(format!("ingest: {e}")))?;
    Ok(Json(json!({ "stored": stored })))
}

/// GET /api/smart-home/events/stream — Server-Sent Events feed.
///
/// Every smart-home event published on the internal bus becomes an SSE
/// message with `event: <kind>` + `data: <json>`. Dashboards use
/// EventSource to subscribe and refresh affected sections reactively
/// rather than polling `/api/smart-home/*/summary` on a timer.
///
/// Lagged receivers (slow consumers exceeding the 256-slot buffer)
/// surface as `broadcast::error::RecvError::Lagged`; we map that to a
/// synthetic `event: lagged` so the client can reload its state
/// snapshot and resume.
pub async fn handle_events_stream(
    State(_state): State<std::sync::Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    use tokio::sync::broadcast::error::RecvError;
    let mut rx = crate::smart_home::events::bus().subscribe();
    let stream = async_stream::stream! {
        // Emit a 'ready' frame first so the client knows the stream is live.
        yield Ok::<_, Infallible>(SseEvent::default().event("ready").data("{}"));
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    // `tag = "kind"` on the enum → serde writes the
                    // kebab-case kind into the JSON. Parse it back out
                    // so we can set the SSE `event:` header properly;
                    // falls back to "event" on any oddity.
                    let json = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                    let kind = serde_json::from_str::<serde_json::Value>(&json)
                        .ok()
                        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string))
                        .unwrap_or_else(|| "event".to_string());
                    yield Ok(SseEvent::default().event(kind).data(json));
                }
                Err(RecvError::Lagged(n)) => {
                    let payload = serde_json::json!({ "dropped": n }).to_string();
                    yield Ok(SseEvent::default().event("lagged").data(payload));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// POST /api/smart-home/diagnostics/sweep — run one sweep synchronously
/// (useful from the dashboard's "Check now" button). Returns the
/// SweepReport (numbers) plus the refreshed summary.
pub async fn handle_diagnostics_sweep(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = std::sync::Arc::new(diagnostics::DiagnosticsEngine::new(
        state.db_path.clone(),
    ));
    let report = engine
        .sweep_once()
        .await
        .map_err(|e| err_500(format!("sweep: {e}")))?;

    // Hand back the numbers + a fresh summary so the UI can update in one round-trip.
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();
    let summary = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = rusqlite::Connection::open(&db)?;
        diagnostics::summary_for_user(&conn, user_id)
    })
    .await
    .map_err(|e| err_500(format!("join error: {e}")))?
    .map_err(|e| err_500(format!("db error: {e}")))?;

    Ok(Json(json!({
        "sweep": report,
        "summary": summary,
    })))
}

// ── BLE anchor config (Week 7 follow-up) ────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BleAnchorBody {
    pub anchor_device_id: i64,
    pub room_id: i64,
    #[serde(default = "default_rssi_at_1m")]
    pub rssi_at_1m: i16,
}

fn default_rssi_at_1m() -> i16 {
    -50
}

#[derive(Debug, Deserialize)]
pub struct BleAnchorsReplaceBody {
    pub anchors: Vec<BleAnchorBody>,
}

/// GET /api/smart-home/ble/anchors — list current anchor config.
/// Populated from runtime map that `ble::BleDriver::hydrate_from_db`
/// loaded at startup, or that a previous PUT supplied.
pub async fn handle_list_ble_anchors(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let Some(driver) = super::drivers::ble::installed() else {
        return Ok(Json(json!({
            "anchors": [],
            "note": "BLE driver not installed in this build",
        })));
    };
    let _ = current_user_id(&state);
    let snapshot = driver.anchors_snapshot().await;
    let mut rows: Vec<serde_json::Value> = snapshot
        .into_values()
        .map(|a| {
            json!({
                "anchor_device_id": a.anchor_device_id,
                "anchor_label": a.anchor_label,
                "room_id": a.room_id,
                "rssi_at_1m": a.rssi_at_1m,
            })
        })
        .collect();
    rows.sort_by_key(|v| v["anchor_device_id"].as_i64().unwrap_or(0));
    Ok(Json(json!({ "anchors": rows })))
}

/// PUT /api/smart-home/ble/anchors — replace the anchor set. Writes
/// through to `smart_home_devices.state_json->ble_anchor` per row so
/// the config survives gateway restart. Strips `ble_anchor` from any
/// device that WAS an anchor but isn't in the new set.
pub async fn handle_put_ble_anchors(
    State(state): State<std::sync::Arc<AppState>>,
    Json(body): Json<BleAnchorsReplaceBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let Some(driver) = super::drivers::ble::installed() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "BLE driver not installed in this build" })),
        ));
    };
    let user_id = current_user_id(&state);
    let db = state.db_path.clone();

    // Validate every referenced device + room exists before writing —
    // reject the whole body if any id is dangling so the caller never
    // gets a half-applied update.
    let validation = {
        let body_anchors = body.anchors.iter()
            .map(|a| (a.anchor_device_id, a.room_id))
            .collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<String>> {
            let conn = rusqlite::Connection::open(&db)?;
            let mut errors = Vec::new();
            for (dev_id, room_id) in body_anchors {
                let dev_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM smart_home_devices WHERE user_id = ? AND id = ?",
                    rusqlite::params![user_id, dev_id],
                    |r| r.get(0),
                ).unwrap_or(0);
                if dev_count == 0 {
                    errors.push(format!("anchor_device_id={dev_id} not found"));
                }
                let room_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM smart_home_rooms WHERE user_id = ? AND id = ?",
                    rusqlite::params![user_id, room_id],
                    |r| r.get(0),
                ).unwrap_or(0);
                if room_count == 0 {
                    errors.push(format!("room_id={room_id} not found"));
                }
            }
            Ok(errors)
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .map_err(|e| err_500(format!("db error: {e}")))?
    };
    if !validation.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid references", "details": validation })),
        ));
    }

    // Build the new map. anchor_label is pulled fresh from the device
    // row — the caller doesn't supply it (prevents stale labels).
    let mut new_anchors: std::collections::HashMap<i64, super::drivers::ble::AnchorConfig> =
        std::collections::HashMap::new();
    for a in body.anchors {
        // Label lookup: query the device name. If the query fails we
        // still accept the anchor with a placeholder label — the
        // driver doesn't need the label for correctness, only for logs.
        let db = state.db_path.clone();
        let dev_id = a.anchor_device_id;
        let label = tokio::task::spawn_blocking(move || -> rusqlite::Result<String> {
            let conn = rusqlite::Connection::open(&db)?;
            conn.query_row(
                "SELECT name FROM smart_home_devices WHERE user_id = ? AND id = ?",
                rusqlite::params![user_id, dev_id],
                |r| r.get(0),
            )
        })
        .await
        .map_err(|e| err_500(format!("join error: {e}")))?
        .unwrap_or_else(|_| format!("device-{dev_id}"));

        new_anchors.insert(
            a.anchor_device_id,
            super::drivers::ble::AnchorConfig {
                anchor_device_id: a.anchor_device_id,
                anchor_label: label,
                room_id: a.room_id,
                rssi_at_1m: a.rssi_at_1m,
            },
        );
    }

    let written = driver
        .persist_anchors(new_anchors)
        .await
        .map_err(|e| err_500(format!("persist: {e}")))?;
    Ok(Json(json!({ "written": written })))
}
