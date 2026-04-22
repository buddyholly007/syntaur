//! HTTP endpoints that front the pure-Rust vendor LAN drivers
//! ([`rust_aidot`] + [`rust_kasa`]) for the Smart Home module.
//!
//! Design rules (match the rest of the project):
//! - Cloud is touched ONLY during `harvest`. After that the runtime is
//!   LAN + Syntaur's own disk.
//! - Inventories live at `~/.syntaur/{aidot,kasa}_inventory.json` (0600).
//!   The files are the source of truth; no DB yet. Future: encrypt at
//!   rest via `~/.syntaur/master.key`.
//! - Endpoints are additive to the existing `/api/smart-home/*` surface;
//!   they do not replace `handle_control` / `handle_list_devices`. When
//!   the main driver framework absorbs these, we delete this file.
//!
//! Routes (mounted in `main.rs`):
//!   POST /api/smart-home/vendor/harvest/aidot  body: { email, password, country? }
//!   POST /api/smart-home/vendor/harvest/kasa   body: { email, password, ips: [...] }
//!   GET  /api/smart-home/vendor/devices        → merged list { aidot: [...], kasa: [...] }
//!   POST /api/smart-home/vendor/action         body: { vendor, alias, action, value? }
//!
//! The `action` endpoint accepts `on | off | dim | rgbw` with a vendor-
//! appropriate `value`. Per-vendor endpoints would be cleaner for types
//! but one merged endpoint keeps the surface small.

use std::path::PathBuf;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `POST /api/smart-home/vendor/harvest/{vendor}`.
pub async fn handle_harvest(
    Path(vendor): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match vendor.as_str() {
        "aidot" => harvest_aidot(body).await,
        "kasa" => harvest_kasa(body).await,
        other => Err((
            StatusCode::NOT_FOUND,
            format!("unknown vendor {other:?}; known: aidot, kasa"),
        )),
    }
}

/// `GET /api/smart-home/vendor/devices` — combined list from both inventories.
pub async fn handle_list_devices() -> Json<Value> {
    let aidot = match load_aidot_inventory() {
        Ok(inv) => inv
            .devices
            .iter()
            .map(|d| {
                json!({
                    "vendor": "aidot",
                    "id": d.id,
                    "alias": d.name,
                    "mac": d.mac,
                    "model": d.model_id,
                    "online": d.online,
                    "ip": d.last_known_ip(),
                })
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    let kasa = match load_kasa_inventory() {
        Ok(inv) => inv
            .devices
            .iter()
            .map(|d| {
                json!({
                    "vendor": "kasa",
                    "id": d.device_id,
                    "alias": d.alias.trim(),
                    "mac": d.mac,
                    "model": d.model,
                    "ip": d.ip,
                })
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    Json(json!({
        "aidot": aidot,
        "kasa": kasa,
        "total": aidot.len() + kasa.len(),
    }))
}

/// `POST /api/smart-home/vendor/action`.
pub async fn handle_action(
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match req.vendor.as_str() {
        "aidot" => aidot_action(req).await,
        "kasa" => kasa_action(req).await,
        other => Err((
            StatusCode::NOT_FOUND,
            format!("unknown vendor {other:?}"),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub vendor: String,
    pub alias: String,
    pub action: String,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AidotHarvestReq {
    email: String,
    password: String,
    #[serde(default)]
    country: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KasaHarvestReq {
    email: String,
    password: String,
    ips: Vec<String>,
}

// ── impls ──────────────────────────────────────────────────────────────

async fn harvest_aidot(body: Value) -> Result<Json<Value>, (StatusCode, String)> {
    let req: AidotHarvestReq = serde_json::from_value(body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad body: {e}")))?;
    let country = req.country.as_deref().unwrap_or("United States");
    let inv = rust_aidot::harvest(&req.email, &req.password, country)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("harvest: {e}")))?;
    write_json_0600(&aidot_inventory_path(), &inv)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("persist: {e}")))?;
    Ok(Json(json!({
        "ok": true,
        "vendor": "aidot",
        "count": inv.devices.len(),
        "path": aidot_inventory_path().display().to_string(),
    })))
}

async fn harvest_kasa(body: Value) -> Result<Json<Value>, (StatusCode, String)> {
    let req: KasaHarvestReq = serde_json::from_value(body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad body: {e}")))?;
    let inv = rust_kasa::harvest_from_ips(&req.email, &req.password, &req.ips)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("harvest: {e}")))?;
    write_json_0600(&kasa_inventory_path(), &inv)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("persist: {e}")))?;
    Ok(Json(json!({
        "ok": true,
        "vendor": "kasa",
        "count": inv.devices.len(),
        "path": kasa_inventory_path().display().to_string(),
    })))
}

async fn aidot_action(req: ActionRequest) -> Result<Json<Value>, (StatusCode, String)> {
    let inv = load_aidot_inventory()
        .map_err(|e| (StatusCode::FAILED_DEPENDENCY, format!("inventory: {e}")))?;
    let dev = inv
        .devices
        .iter()
        .find(|d| d.name == req.alias)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no aidot device named {:?}", req.alias),
            )
        })?;
    let ip = dev.last_known_ip().ok_or((
        StatusCode::FAILED_DEPENDENCY,
        "device has no cached IP; re-harvest".into(),
    ))?;
    let mut client = rust_aidot::DeviceClient::connect(dev.clone(), inv.user_id.clone(), &ip)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("connect: {e}")))?;

    match req.action.as_str() {
        "on" => client
            .turn_on()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn_on: {e}")))?,
        "off" => client
            .turn_off()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn_off: {e}")))?,
        "dim" => {
            let pct = req
                .value
                .as_ref()
                .and_then(|v| v.as_u64())
                .ok_or((StatusCode::BAD_REQUEST, "dim requires numeric value".into()))?
                .min(100) as u8;
            client
                .set_dimming(pct)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, format!("dim: {e}")))?
        }
        "rgbw" => {
            let arr = req
                .value
                .as_ref()
                .and_then(|v| v.as_array())
                .ok_or((StatusCode::BAD_REQUEST, "rgbw requires [r,g,b,w]".into()))?;
            if arr.len() != 4 {
                return Err((StatusCode::BAD_REQUEST, "rgbw must be length-4".into()));
            }
            let comp = |i: usize| arr[i].as_u64().unwrap_or(0).min(255) as u8;
            client
                .set_rgbw(comp(0), comp(1), comp(2), comp(3))
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, format!("rgbw: {e}")))?
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown aidot action {other:?}"),
            ))
        }
    }
    Ok(Json(json!({ "ok": true })))
}

async fn kasa_action(req: ActionRequest) -> Result<Json<Value>, (StatusCode, String)> {
    let inv = load_kasa_inventory()
        .map_err(|e| (StatusCode::FAILED_DEPENDENCY, format!("inventory: {e}")))?;
    let dev = inv
        .find_by_alias(&req.alias)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no kasa device named {:?}", req.alias),
            )
        })?;
    let mut client = rust_kasa::Device::connect(&dev.ip, &inv.username, &inv.password)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("connect: {e}")))?;
    match req.action.as_str() {
        "on" => client
            .turn_on()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn_on: {e}")))?,
        "off" => client
            .turn_off()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn_off: {e}")))?,
        "dim" | "brightness" => {
            let pct = req
                .value
                .as_ref()
                .and_then(|v| v.as_u64())
                .ok_or((StatusCode::BAD_REQUEST, "dim requires numeric value".into()))?
                .min(100) as u8;
            client
                .set_brightness(pct)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, format!("brightness: {e}")))?
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown kasa action {other:?}"),
            ))
        }
    }
    Ok(Json(json!({ "ok": true })))
}

// ── persistence helpers ───────────────────────────────────────────────

fn syntaur_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    PathBuf::from(home).join(".syntaur")
}

fn aidot_inventory_path() -> PathBuf {
    syntaur_dir().join("aidot_inventory.json")
}

fn kasa_inventory_path() -> PathBuf {
    syntaur_dir().join("kasa_inventory.json")
}

fn load_aidot_inventory() -> std::io::Result<rust_aidot::Inventory> {
    let bytes = std::fs::read(aidot_inventory_path())?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn load_kasa_inventory() -> std::io::Result<rust_kasa::Inventory> {
    rust_kasa::Inventory::load_default().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("{e}"))
    })
}

fn write_json_0600<T: Serialize>(path: &PathBuf, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write atomically via a temp file so an interrupted write doesn't
    // leave a torn-credential file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
