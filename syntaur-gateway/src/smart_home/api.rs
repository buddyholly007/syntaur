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
    Ok(Json(json!({ "device": device })))
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
        other => Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": format!("driver '{}' not wired yet", other),
                "hint": "Non-Matter driver control lands per the plan calendar (Zigbee week 5, BLE week 7, MQTT week 8, cameras week 9, cloud adapters week 11, Z-Wave week 13)."
            })),
        )),
    }
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
        },
    );
    Ok(Json(json!({ "device": updated })))
}

// ── automation (stub) ───────────────────────────────────────────────────

pub async fn handle_list_automations() -> Json<serde_json::Value> {
    Json(json!({ "automations": [] }))
}

pub async fn handle_compile_automation(
    Json(_body): Json<super::nl_automation::CompileRequest>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "nl_automation::compile not yet implemented (week 7 milestone)"
        })),
    )
}

#[derive(Debug, Deserialize)]
pub struct AutomationCreateBody {
    pub name: String,
    pub source: String,
    pub spec: AutomationSpec,
}

pub async fn handle_create_automation(
    Json(_body): Json<AutomationCreateBody>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "automation persistence not yet wired (week 10 milestone)"
        })),
    )
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
