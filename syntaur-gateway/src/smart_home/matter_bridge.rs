//! HTTP endpoints for Path C — Matter fabric + commissioning.
//!
//! Gateway integration (Phase 6). Routes:
//!   POST /api/smart-home/matter/fabric/init         { label }
//!   GET  /api/smart-home/matter/fabrics              → list of summaries
//!   POST /api/smart-home/matter/pair/decode          { qr } | { code }  → decoded PairingPayload
//!   POST /api/smart-home/matter/commission           { fabric_label, qr?, code?, wifi_ssid?, wifi_psk?, assigned_node_id }
//!   GET  /api/smart-home/matter/auto_recommission    → bool
//!   POST /api/smart-home/matter/auto_recommission    { enabled: bool }
//!
//! The `commission` endpoint currently 501s with a clear pointer to
//! Phase 4 (BLE central transport) — the state machine + fabric +
//! QR parsing are all ready; only the transport layer remains. When
//! `syntaur-matter-ble` ships BTP, this handler becomes a ~10-line
//! wrapper that pipes through the existing Commissioner.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use syntaur_matter::{
    list_fabrics, parse_manual_code, parse_qr, save_fabric, FabricHandle, PairingPayload,
};

/// `POST /api/smart-home/matter/fabric/init`.
pub async fn handle_init_fabric(
    Json(body): Json<InitFabricReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let handle = FabricHandle::new(body.label.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("fabric new: {e}")))?;
    let path = save_fabric(&handle)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("persist: {e}")))?;
    Ok(Json(json!({
        "ok": true,
        "label": handle.label,
        "fabric_id": format!("{:#018x}", handle.fabric_id),
        "path": path.display().to_string(),
    })))
}

/// `GET /api/smart-home/matter/fabrics`.
pub async fn handle_list_fabrics() -> Result<Json<Value>, (StatusCode, String)> {
    let all = list_fabrics()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("list: {e}")))?;
    Ok(Json(json!({ "fabrics": all })))
}

#[derive(Debug, Deserialize)]
pub struct InitFabricReq {
    pub label: String,
}

#[derive(Debug, Deserialize)]
pub struct PairDecodeReq {
    #[serde(default)]
    pub qr: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

/// `POST /api/smart-home/matter/pair/decode`.
pub async fn handle_decode_pairing(
    Json(req): Json<PairDecodeReq>,
) -> Result<Json<PairingPayload>, (StatusCode, String)> {
    let payload = match (req.qr.as_deref(), req.code.as_deref()) {
        (Some(q), _) => parse_qr(q),
        (_, Some(c)) => parse_manual_code(c),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "supply either `qr` or `code`".into(),
            ))
        }
    }
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse: {e}")))?;
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
pub struct CommissionReq {
    pub fabric_label: String,
    #[serde(default)]
    pub qr: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub wifi_ssid: Option<String>,
    #[serde(default)]
    pub wifi_psk: Option<String>,
    /// Node ID to assign the device on our fabric.
    pub assigned_node_id: u64,
}

/// `POST /api/smart-home/matter/commission`.
///
/// **Currently returns 501** — the commissioner state machine + fabric
/// + QR parsing are all ready server-side. The missing piece is
/// the BLE central transport in `syntaur-matter-ble::btp`. When
/// Phase 4 ships, this handler's body shrinks to ~10 lines: load
/// fabric → parse pairing code → `BleCommissionExchange::connect(...)`
/// → `Commissioner::new(&fabric).commission(&mut ex, node_id, wifi)`.
pub async fn handle_commission(
    Json(req): Json<CommissionReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Validate inputs up-front so Phase 4 bring-up doesn't have to.
    let _fabric = syntaur_matter::load_fabric(&req.fabric_label)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("fabric: {e}")))?;
    let _payload = match (req.qr.as_deref(), req.code.as_deref()) {
        (Some(q), _) => parse_qr(q),
        (_, Some(c)) => parse_manual_code(c),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "supply either `qr` or `code`".into(),
            ))
        }
    }
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse: {e}")))?;

    Err((
        StatusCode::NOT_IMPLEMENTED,
        format!(
            "Path C Phase 4 (BLE central + BTP session) not shipped yet. \
             Inputs validated: fabric={:?}, assigned_node_id={}, has_wifi={}. \
             See vault/projects/path_c_plan.md for remaining work.",
            req.fabric_label,
            req.assigned_node_id,
            req.wifi_ssid.is_some()
        ),
    ))
}

// ── Auto-recommission daemon — Phase 7 scaffold ─────────────────────

/// `GET /api/smart-home/matter/auto_recommission`.
pub async fn handle_get_auto_recommission() -> Json<Value> {
    let enabled = auto_recommission_enabled();
    Json(json!({ "enabled": enabled }))
}

#[derive(Debug, Deserialize)]
pub struct AutoRecommissionReq {
    pub enabled: bool,
}

/// `POST /api/smart-home/matter/auto_recommission`.
///
/// Single global opt-in per Sean's spec: "I don't want it per-device."
/// Flag is persisted in `~/.syntaur/auto_recommission.flag` (empty
/// file = enabled; missing = disabled).
pub async fn handle_set_auto_recommission(
    Json(req): Json<AutoRecommissionReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let path = auto_recommission_flag_path();
    if req.enabled {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir: {e}")))?;
        }
        std::fs::write(&path, b"")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}")))?;
    } else {
        let _ = std::fs::remove_file(&path);
    }
    Ok(Json(json!({ "ok": true, "enabled": req.enabled })))
}

fn auto_recommission_flag_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    std::path::PathBuf::from(home)
        .join(".syntaur")
        .join("auto_recommission.flag")
}

fn auto_recommission_enabled() -> bool {
    auto_recommission_flag_path().exists()
}

// We intentionally DON'T wire the mDNS watcher task here — it belongs
// to a background service that watches `_matterc._udp` and triggers
// re-commissioning when a saved pairing record matches an advertised
// device. That service needs the BLE transport (Phase 4) to actually
// re-pair. Skeleton + plan live in vault/projects/path_c_plan.md §
// Phase 7. The file-flag + GET/POST pair above ships today so the
// dashboard can toggle the user preference now; the watcher hooks up
// to the same flag when it lands.

// Surface Phase 6 helper fn for smoke tests + docs.
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn decode_route_roundtrips_known_qr() {
        let resp = super::handle_decode_pairing(axum::Json(super::PairDecodeReq {
            qr: Some("MT:Y.K9042C00KA0648G00".into()),
            code: None,
        }))
        .await
        .unwrap();
        assert_eq!(resp.0.vendor_id, Some(0x2ECE));
    }

    #[tokio::test]
    async fn decode_route_roundtrips_known_code() {
        let resp = super::handle_decode_pairing(axum::Json(super::PairDecodeReq {
            qr: None,
            code: Some("00876800071".into()),
        }))
        .await
        .unwrap();
        assert_eq!(resp.0.passcode, 123456);
    }
}

// `Serialize` is manually required because `PairingPayload` defaults
// to plain serde_json::to_value via axum Json — make sure it's
// derivable. (It already is; this import just makes the compiler
// happy for the Serialize bound axum needs.)
#[allow(dead_code)]
fn _assert_serializable(_: &impl Serialize) {}
