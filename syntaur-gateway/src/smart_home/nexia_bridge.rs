//! HTTP endpoints for the Nexia (Trane / Asair) cloud driver.
//!
//! Credentials are stored encrypted at
//! `~/.syntaur/nexia_creds.json` (XChaCha20-Poly1305 via the same
//! master.key envelope used by `syntaur-matter`). Re-login on every
//! request is acceptable — the Nexia cloud tolerates it and the extra
//! 300 ms is fine for a control-plane endpoint.
//!
//! Routes:
//!   POST /api/smart-home/nexia/creds     { email, password, brand? }
//!   GET  /api/smart-home/nexia/thermostats
//!   POST /api/smart-home/nexia/setpoint  { zone_id, heat?, cool? }
//!   POST /api/smart-home/nexia/mode      { zone_id, mode }       // HEAT|COOL|AUTO|OFF
//!   POST /api/smart-home/nexia/fan       { zone_id, fan_mode }   // auto|on|circulate
//!   POST /api/smart-home/nexia/run_mode  { zone_id, run_mode }   // run_schedule|permanent_hold
//!   POST /api/smart-home/nexia/em_heat   { zone_id, on }
//!
//! After we add the rest-of-Syntaur nexia crate wiring, callers don't
//! care that the backend is vendor cloud — the endpoint shape matches
//! our other smart-home drivers.

use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rust_nexia::{Brand, FanMode, HvacMode, NexiaClient, RunMode};

#[derive(Deserialize)]
pub struct CredsReq {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub brand: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredCreds {
    email: String,
    password: String,
    brand: String,
}

/// `POST /api/smart-home/nexia/creds`.
pub async fn handle_save_creds(
    Json(req): Json<CredsReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let brand = req.brand.clone().unwrap_or_else(|| "trane".into());
    // Validate by logging in once.
    let mut client = NexiaClient::new(brand_from_str(&brand)?);
    client.login(&req.email, &req.password).await.map_err(|e| {
        (StatusCode::UNAUTHORIZED, format!("nexia login failed: {e}"))
    })?;

    let creds = StoredCreds {
        email: req.email,
        password: req.password,
        brand,
    };
    write_creds(&creds)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("persist: {e}")))?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/smart-home/nexia/thermostats`.
pub async fn handle_list_thermostats() -> Result<Json<Value>, (StatusCode, String)> {
    let creds = read_creds()
        .map_err(|e| (StatusCode::FAILED_DEPENDENCY, format!("no creds stored: {e}")))?;
    let mut client = NexiaClient::new(brand_from_str(&creds.brand)?);
    client
        .login(&creds.email, &creds.password)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("login: {e}")))?;
    let therms = client
        .list_thermostats()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("list: {e}")))?;

    // Project into a JSON-friendly shape. Keep the shape similar to
    // the vendor_bridge pattern — an array with `vendor: "nexia"`.
    let items: Vec<Value> = therms
        .into_iter()
        .flat_map(|t| {
            let therm_meta = json!({
                "vendor": "nexia",
                "id": t.id,
                "name": t.name,
                "model": t.model,
                "manufacturer": t.manufacturer,
                "firmware": t.firmware_version,
                "system_status": t.system_status,
                "indoor_humidity": t.indoor_humidity,
                "outdoor_temperature": t.outdoor_temperature,
                "compressor_speed": t.compressor_speed,
                "mode": t.mode,
                "fan_mode": t.fan_mode,
            });
            t.zones
                .into_iter()
                .map(move |z| {
                    let mut obj = therm_meta.clone();
                    if let Value::Object(m) = &mut obj {
                        m.insert(
                            "zone".into(),
                            json!({
                                "id": z.id,
                                "temperature": z.temperature,
                                "heat_setpoint": z.heat_setpoint,
                                "cool_setpoint": z.cool_setpoint,
                                "operating_state": z.operating_state,
                                "setpoint_heat_min": z.setpoint_heat_min,
                                "setpoint_heat_max": z.setpoint_heat_max,
                                "setpoint_cool_min": z.setpoint_cool_min,
                                "setpoint_cool_max": z.setpoint_cool_max,
                                "setpoint_delta": z.setpoint_delta,
                                "scale": z.scale,
                            }),
                        );
                    }
                    obj
                })
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(Json(json!({ "thermostats": items })))
}

#[derive(Deserialize)]
pub struct SetpointReq {
    pub zone_id: u64,
    #[serde(default)]
    pub heat: Option<f32>,
    #[serde(default)]
    pub cool: Option<f32>,
}
#[derive(Deserialize)]
pub struct ModeReq {
    pub zone_id: u64,
    pub mode: String,
}
#[derive(Deserialize)]
pub struct FanReq {
    pub zone_id: u64,
    pub fan_mode: String,
}
#[derive(Deserialize)]
pub struct RunModeReq {
    pub zone_id: u64,
    pub run_mode: String,
}
#[derive(Deserialize)]
pub struct EmHeatReq {
    pub zone_id: u64,
    pub on: bool,
}

pub async fn handle_setpoint(
    Json(r): Json<SetpointReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (client, therms) = connect_and_list().await?;
    let z = find_zone(&therms, r.zone_id)?;
    client.set_setpoint(z, r.heat, r.cool).await.map_err(apierr)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn handle_mode(Json(r): Json<ModeReq>) -> Result<Json<Value>, (StatusCode, String)> {
    let (client, therms) = connect_and_list().await?;
    let z = find_zone(&therms, r.zone_id)?;
    let mode = match r.mode.to_ascii_uppercase().as_str() {
        "HEAT" => HvacMode::Heat,
        "COOL" => HvacMode::Cool,
        "AUTO" => HvacMode::Auto,
        "OFF" => HvacMode::Off,
        other => {
            return Err((StatusCode::BAD_REQUEST, format!("unknown mode: {other}")))
        }
    };
    client.set_mode(z, mode).await.map_err(apierr)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn handle_fan(Json(r): Json<FanReq>) -> Result<Json<Value>, (StatusCode, String)> {
    let (client, therms) = connect_and_list().await?;
    let z = find_zone(&therms, r.zone_id)?;
    let fm = match r.fan_mode.to_ascii_lowercase().as_str() {
        "auto" => FanMode::Auto,
        "on" => FanMode::On,
        "circulate" | "circ" => FanMode::Circulate,
        other => {
            return Err((StatusCode::BAD_REQUEST, format!("unknown fan_mode: {other}")))
        }
    };
    client.set_fan_mode(z, fm).await.map_err(apierr)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn handle_run_mode(
    Json(r): Json<RunModeReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (client, therms) = connect_and_list().await?;
    let z = find_zone(&therms, r.zone_id)?;
    let rm = match r.run_mode.to_ascii_lowercase().as_str() {
        "run_schedule" | "schedule" | "resume" => RunMode::Schedule,
        "permanent_hold" | "hold" => RunMode::PermanentHold,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown run_mode: {other}"),
            ))
        }
    };
    client.set_run_mode(z, rm).await.map_err(apierr)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn handle_em_heat(
    Json(r): Json<EmHeatReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (client, therms) = connect_and_list().await?;
    let z = find_zone(&therms, r.zone_id)?;
    client.set_emergency_heat(z, r.on).await.map_err(apierr)?;
    Ok(Json(json!({ "ok": true })))
}

// ── helpers ───────────────────────────────────────────────────────

fn apierr(e: rust_nexia::NexiaError) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, format!("nexia: {e}"))
}

async fn connect_and_list(
) -> Result<(NexiaClient, Vec<rust_nexia::Thermostat>), (StatusCode, String)> {
    let creds = read_creds()
        .map_err(|e| (StatusCode::FAILED_DEPENDENCY, format!("no creds: {e}")))?;
    let mut client = NexiaClient::new(brand_from_str(&creds.brand)?);
    client
        .login(&creds.email, &creds.password)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("login: {e}")))?;
    let therms = client
        .list_thermostats()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("list: {e}")))?;
    Ok((client, therms))
}

fn find_zone<'a>(
    therms: &'a [rust_nexia::Thermostat],
    zid: u64,
) -> Result<&'a rust_nexia::Zone, (StatusCode, String)> {
    for t in therms {
        for z in &t.zones {
            if z.id == zid {
                return Ok(z);
            }
        }
    }
    Err((StatusCode::NOT_FOUND, format!("zone {zid} not found")))
}

fn brand_from_str(s: &str) -> Result<Brand, (StatusCode, String)> {
    match s.to_ascii_lowercase().as_str() {
        "trane" => Ok(Brand::Trane),
        "nexia" => Ok(Brand::Nexia),
        "asair" => Ok(Brand::Asair),
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("brand must be trane|nexia|asair (got {other:?})"),
        )),
    }
}

// ── credential persistence (plain JSON 0600 for now; encrypted at
//    rest is a follow-up that plugs into syntaur's master.key envelope
//    — same pattern syntaur-matter uses) ───────────────────────────

fn creds_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home).join(".syntaur").join("nexia_creds.json")
}

fn read_creds() -> std::io::Result<StoredCreds> {
    let bytes = std::fs::read(creds_path())?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn write_creds(c: &StoredCreds) -> std::io::Result<()> {
    let path = creds_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(c)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
