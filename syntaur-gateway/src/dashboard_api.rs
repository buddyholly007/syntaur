//! `/api/appearance` + `/api/dashboard/layout` — per-user storage for the
//! dashboard theme engine and widget grid.
//!
//! Schema in `index/schema.rs` v64: `user_appearance` + `dashboard_layout`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::AppState;

// ─── /api/appearance ──────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserAppearance {
    pub accent: String,
    pub theme_mode: String,
    pub hue_shift: u8,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub light_start_min: u32,
    pub dark_start_min: u32,
    #[serde(default)]
    pub ambient_mode: u8,
}

impl Default for UserAppearance {
    fn default() -> Self {
        // Dark is the calm-neutral default. Light mode is opt-in via
        // Settings → Appearance; `auto` only flips to light when the
        // user has set a latitude/longitude (see theme.rs::compute).
        Self {
            accent: "sage".into(),
            theme_mode: "dark".into(),
            hue_shift: 0,
            latitude: None,
            longitude: None,
            light_start_min: 420,
            dark_start_min: 1140,
            ambient_mode: 0,
        }
    }
}

fn validate_appearance(a: &UserAppearance) -> Result<(), &'static str> {
    if !matches!(a.accent.as_str(), "sage" | "indigo" | "ochre" | "gray") {
        return Err("accent must be sage|indigo|ochre|gray");
    }
    if !matches!(a.theme_mode.as_str(), "auto" | "light" | "dark" | "schedule") {
        return Err("theme_mode must be auto|light|dark|schedule");
    }
    if a.light_start_min >= 1440 || a.dark_start_min >= 1440 {
        return Err("start_min must be < 1440");
    }
    if a.hue_shift > 1 {
        return Err("hue_shift must be 0 or 1");
    }
    if a.ambient_mode > 1 {
        return Err("ambient_mode must be 0 or 1");
    }
    if let Some(lat) = a.latitude { if !(-90.0..=90.0).contains(&lat) { return Err("latitude out of range"); } }
    if let Some(lon) = a.longitude { if !(-180.0..=180.0).contains(&lon) { return Err("longitude out of range"); } }
    Ok(())
}

pub async fn handle_get_appearance(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<Json<UserAppearance>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db_path = state.db_path.clone();
    let row = tokio::task::spawn_blocking(move || -> Result<Option<UserAppearance>, rusqlite::Error> {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.query_row(
            "SELECT accent, theme_mode, hue_shift, latitude, longitude, light_start_min, dark_start_min, ambient_mode
             FROM user_appearance WHERE user_id = ?",
            [uid],
            |r| Ok(UserAppearance {
                accent: r.get(0)?,
                theme_mode: r.get(1)?,
                hue_shift: r.get::<_, i64>(2)? as u8,
                latitude: r.get(3)?,
                longitude: r.get(4)?,
                light_start_min: r.get::<_, i64>(5)? as u32,
                dark_start_min: r.get::<_, i64>(6)? as u32,
                ambient_mode: r.get::<_, i64>(7)? as u8,
            }),
        ).map(Some).or_else(|e| if let rusqlite::Error::QueryReturnedNoRows = e { Ok(None) } else { Err(e) })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(row.unwrap_or_default()))
}

pub async fn handle_post_appearance(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UserAppearance>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    if let Err(e) = validate_appearance(&body) {
        return Ok(Json(serde_json::json!({ "ok": false, "error": e })));
    }
    let db_path = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "INSERT INTO user_appearance (user_id, accent, theme_mode, hue_shift, latitude, longitude, light_start_min, dark_start_min, ambient_mode, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
               accent=excluded.accent, theme_mode=excluded.theme_mode, hue_shift=excluded.hue_shift,
               latitude=excluded.latitude, longitude=excluded.longitude,
               light_start_min=excluded.light_start_min, dark_start_min=excluded.dark_start_min,
               ambient_mode=excluded.ambient_mode,
               updated_at=excluded.updated_at",
            rusqlite::params![
                uid, body.accent, body.theme_mode, body.hue_shift as i64,
                body.latitude, body.longitude,
                body.light_start_min as i64, body.dark_start_min as i64,
                body.ambient_mode as i64, now
            ],
        )?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ─── /api/dashboard/layout ────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DashboardLayoutItem {
    pub id: i64,
    pub kind: String,
    pub size: String,     // "S" | "M" | "L" | "XL"
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct DashboardLayoutBody {
    pub items: Vec<DashboardLayoutItem>,
}

fn validate_layout(items: &[DashboardLayoutItem]) -> Result<(), &'static str> {
    if items.len() > 48 {
        return Err("too many widgets (max 48)");
    }
    for it in items {
        if !matches!(it.size.as_str(), "S" | "M" | "L" | "XL") {
            return Err("size must be S|M|L|XL");
        }
        if it.kind.len() > 64 || it.kind.is_empty() {
            return Err("kind length out of range");
        }
        if crate::pages::dashboard_widgets::find(&it.kind).is_none() {
            return Err("unknown widget kind");
        }
    }
    Ok(())
}

pub async fn handle_get_layout(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<Json<DashboardLayoutBody>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db_path = state.db_path.clone();
    let json: Option<String> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<String>> {
        let conn = rusqlite::Connection::open(&db_path)?;
        let r: rusqlite::Result<String> = conn.query_row(
            "SELECT layout_json FROM dashboard_layout WHERE user_id = ?",
            [uid],
            |r| r.get(0),
        );
        match r {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let items: Vec<DashboardLayoutItem> = match json {
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
        None => default_layout(),
    };
    Ok(Json(DashboardLayoutBody { items }))
}

pub async fn handle_post_layout(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(body): Json<DashboardLayoutBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    if let Err(e) = validate_layout(&body.items) {
        return Ok(Json(serde_json::json!({ "ok": false, "error": e })));
    }
    let payload = serde_json::to_string(&body.items)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let db_path = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "INSERT INTO dashboard_layout (user_id, layout_json, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET layout_json=excluded.layout_json, updated_at=excluded.updated_at",
            rusqlite::params![uid, payload, now],
        )?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn default_layout() -> Vec<DashboardLayoutItem> {
    // First-run widgets: the handful a new user will touch within 30
    // seconds of landing — Peter chat (Dashboard persona, hands off),
    // todo, calendar, today's events, quick module launchers, and the
    // now-playing strip. Keep in sync with `defaultLayout()` in
    // pages/dashboard.rs DASHBOARD_SCRIPT.
    vec![
        DashboardLayoutItem { id: 1, kind: "chat".into(),          size: "L".into(), config: serde_json::Value::Null },
        DashboardLayoutItem { id: 2, kind: "todo".into(),          size: "M".into(), config: serde_json::Value::Null },
        DashboardLayoutItem { id: 3, kind: "calendar".into(),      size: "M".into(), config: serde_json::Value::Null },
        DashboardLayoutItem { id: 4, kind: "today".into(),         size: "M".into(), config: serde_json::Value::Null },
        DashboardLayoutItem { id: 5, kind: "quick_actions".into(), size: "M".into(), config: serde_json::Value::Null },
        DashboardLayoutItem { id: 6, kind: "now_playing".into(),   size: "M".into(), config: serde_json::Value::Null },
    ]
}

// ─── /api/dashboard/system ────────────────────────────────────────────

pub async fn handle_get_system(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let _principal = crate::resolve_principal(&state, token).await?;

    let uptime_secs = state.start_time.elapsed().as_secs();
    let llm_provider = state.config.models.mode.clone();
    let modules_on = state.config.modules.entries.values().filter(|e| e.enabled).count() as i64;
    let modules_total = state.config.modules.entries.len() as i64;
    let version = env!("CARGO_PKG_VERSION");

    Ok(Json(serde_json::json!({
        "uptime_secs": uptime_secs,
        "llm_provider": llm_provider,
        "modules_on": modules_on,
        "modules_total": modules_total,
        "version": version,
    })))
}
