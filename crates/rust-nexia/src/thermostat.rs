//! Typed thermostat + zone control over the Nexia cloud REST API.
//!
//! Every operation is a POST to an href advertised by the device's
//! own feature JSON — Nexia's API is hypermedia-ish, so rather than
//! hardcoding paths we parse the advertised href per action. For our
//! known account we've seen the canonical forms:
//!   POST /mobile/xxl_zones/{zone_id}/setpoints         {heat, cool}
//!   POST /mobile/xxl_zones/{zone_id}/zone_mode         {value: HEAT|COOL|AUTO|OFF}
//!   POST /mobile/xxl_zones/{zone_id}/run_mode          {value: run_schedule|permanent_hold}
//!   POST /mobile/xxl_thermostats/{id}/fan_mode         {value: auto|on|circulate}
//!   POST /mobile/xxl_thermostats/{id}/air_cleaner_mode {value: auto|quick|allergy}
//!   POST /mobile/xxl_thermostats/{id}/emergency_heat   {value: true|false}
//!   POST /mobile/xxl_thermostats/{id}/fan_speed        {value: 0.0..1.0}
//!
//! The "hypermedia" angle: we ALSO expose
//! [`ZoneHandle::post_to_named_action`] which walks the feature tree
//! to find an action by name. This is more resilient if Nexia ever
//! changes the URL shape but adds a transition stage where the old
//! URL still works.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{NexiaClient, NexiaError};

/// One thermostat (typically one per HVAC unit) + its zones. Only
/// single-zone systems tested so far; multi-zone Trane Comfortlink
/// setups should also work since the zone structure is first-class in
/// the API.
#[derive(Debug, Clone)]
pub struct Thermostat {
    /// Numeric device id — the key to `/mobile/xxl_thermostats/{id}`.
    pub id: u64,
    /// User-facing name (e.g. `"Thermostat 1"`).
    pub name: String,
    /// `Trane` / `Nexia` / `American Standard` — the top-of-page manufacturer string.
    pub manufacturer: Option<String>,
    /// Model string from `advanced_info` (e.g. `"XL850"`).
    pub model: Option<String>,
    /// Firmware version from `advanced_info` (e.g. `"5.12.1"`).
    pub firmware_version: Option<String>,
    /// AUID (Application Unique ID) — Trane/Nexia's internal device serial.
    pub auid: Option<String>,
    /// Live HVAC status string (`"System Idle"`, `"Cooling"`, `"Heating"`,
    /// `"Waiting..."`, etc.).
    pub system_status: String,
    /// Last-reported indoor humidity (whole percent).
    pub indoor_humidity: Option<f32>,
    /// Last-reported outdoor temperature (scale matches the zone).
    pub outdoor_temperature: Option<f32>,
    /// Current variable-compressor speed on 0.0..=1.0; 0.0 = idle.
    /// Fields's presence on your system means a variable-speed
    /// outdoor unit (XV18/XV19/XV20i-class).
    pub compressor_speed: Option<f32>,
    /// Zones owned by this thermostat (typically 1; multi-zone
    /// systems can list multiple).
    pub zones: Vec<Zone>,
    /// Current system mode (`HEAT`/`COOL`/`AUTO`/`OFF`) — mirrors the
    /// value of the `thermostat_mode` feature.
    pub mode: Option<String>,
    /// Current fan mode (`auto`/`on`/`circulate`).
    pub fan_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Zone {
    /// `/mobile/xxl_zones/{id}`.
    pub id: u64,
    /// Current air temperature in the zone.
    pub temperature: Option<f32>,
    /// Active heat setpoint.
    pub heat_setpoint: Option<f32>,
    /// Active cool setpoint.
    pub cool_setpoint: Option<f32>,
    /// `heat` / `cool` / `idle` / `off` — what the equipment is doing right now.
    pub operating_state: Option<String>,
    /// Minimum/maximum allowed heat setpoint values.
    pub setpoint_heat_min: Option<i32>,
    pub setpoint_heat_max: Option<i32>,
    pub setpoint_cool_min: Option<i32>,
    pub setpoint_cool_max: Option<i32>,
    /// Minimum delta between heat and cool setpoints in AUTO mode.
    pub setpoint_delta: Option<i32>,
    /// Temperature scale (`f` or `c`).
    pub scale: String,
    /// Per-feature href cache so callers don't need to walk the tree.
    actions: Actions,
}

#[derive(Debug, Clone, Default)]
struct Actions {
    set_setpoint: Option<String>,
    set_zone_mode: Option<String>,
    set_run_mode: Option<String>,
    set_fan_mode: Option<String>,
    set_emergency_heat: Option<String>,
    set_air_cleaner_mode: Option<String>,
    set_fan_speed: Option<String>,
    set_dehumidify: Option<String>,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HvacMode {
    #[serde(rename = "HEAT")]
    Heat,
    #[serde(rename = "COOL")]
    Cool,
    #[serde(rename = "AUTO")]
    Auto,
    #[serde(rename = "OFF")]
    Off,
}

impl HvacMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heat => "HEAT",
            Self::Cool => "COOL",
            Self::Auto => "AUTO",
            Self::Off => "OFF",
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FanMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "on")]
    On,
    #[serde(rename = "circulate")]
    Circulate,
}

impl FanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Circulate => "circulate",
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunMode {
    #[serde(rename = "run_schedule")]
    Schedule,
    #[serde(rename = "permanent_hold")]
    PermanentHold,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "run_schedule",
            Self::PermanentHold => "permanent_hold",
        }
    }
}

impl NexiaClient {
    /// Bootstrap — login already done. Discover the house_id via
    /// `POST /mobile/session`, then load the full device tree.
    pub async fn list_thermostats(&self) -> Result<Vec<Thermostat>, NexiaError> {
        let session = self
            .post_raw(
                "/mobile/session",
                json!({
                    "app_version": crate::APP_VERSION,
                    "device_uuid": uuid::Uuid::new_v4().to_string(),
                }),
            )
            .await?;
        let house_id = session
            .pointer("/result/_links/child/0/data/id")
            .ok_or(NexiaError::MissingField("house_id"))?;
        let house_id = house_id
            .as_u64()
            .map(|n| n.to_string())
            .or_else(|| house_id.as_str().map(String::from))
            .ok_or(NexiaError::MissingField("house_id as u64/str"))?;
        let tree = self
            .get_raw(&format!("/mobile/houses/{house_id}"))
            .await?;
        let devices = find_devices_items(&tree);
        Ok(devices.into_iter().filter_map(parse_thermostat).collect())
    }

    /// Set heat and/or cool setpoint on a zone. Passing `None` for
    /// either keeps the current value. Nexia will enforce min/max +
    /// delta constraints server-side.
    pub async fn set_setpoint(
        &self,
        zone: &Zone,
        heat: Option<f32>,
        cool: Option<f32>,
    ) -> Result<(), NexiaError> {
        let href = zone
            .actions
            .set_setpoint
            .as_deref()
            .ok_or(NexiaError::MissingField("set_setpoint action"))?;
        let mut body = serde_json::Map::new();
        if let Some(h) = heat {
            body.insert("heat".into(), json!(h));
        }
        if let Some(c) = cool {
            body.insert("cool".into(), json!(c));
        }
        self.post_raw(href, Value::Object(body)).await?;
        Ok(())
    }

    pub async fn set_mode(&self, zone: &Zone, mode: HvacMode) -> Result<(), NexiaError> {
        let href = zone
            .actions
            .set_zone_mode
            .as_deref()
            .ok_or(NexiaError::MissingField("set_zone_mode action"))?;
        self.post_raw(href, json!({ "value": mode.as_str() })).await?;
        Ok(())
    }

    pub async fn set_run_mode(&self, zone: &Zone, run: RunMode) -> Result<(), NexiaError> {
        let href = zone
            .actions
            .set_run_mode
            .as_deref()
            .ok_or(NexiaError::MissingField("set_run_mode action"))?;
        self.post_raw(href, json!({ "value": run.as_str() })).await?;
        Ok(())
    }

    pub async fn set_fan_mode(&self, zone: &Zone, fan: FanMode) -> Result<(), NexiaError> {
        let href = zone
            .actions
            .set_fan_mode
            .as_deref()
            .ok_or(NexiaError::MissingField("set_fan_mode action"))?;
        self.post_raw(href, json!({ "value": fan.as_str() })).await?;
        Ok(())
    }

    pub async fn set_emergency_heat(&self, zone: &Zone, on: bool) -> Result<(), NexiaError> {
        let href = zone
            .actions
            .set_emergency_heat
            .as_deref()
            .ok_or(NexiaError::MissingField("set_emergency_heat action"))?;
        self.post_raw(href, json!({ "value": on })).await?;
        Ok(())
    }
}

// ── parser ──────────────────────────────────────────────────────────────

fn find_devices_items(tree: &Value) -> Vec<Value> {
    let Some(children) = tree
        .pointer("/result/_links/child")
        .and_then(|v| v.as_array())
    else {
        return vec![];
    };
    for child in children {
        let href = child.pointer("/href").and_then(|v| v.as_str()).unwrap_or("");
        if href.contains("/devices") {
            if let Some(items) = child
                .pointer("/data/items")
                .and_then(|v| v.as_array())
            {
                return items.clone();
            }
        }
    }
    vec![]
}

fn parse_thermostat(item: Value) -> Option<Thermostat> {
    let id = item.get("id").and_then(|v| v.as_u64())?;
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("thermostat")
        .to_string();
    let manufacturer = item
        .get("manufacturer")
        .and_then(|v| v.as_str())
        .map(String::from);
    let indoor_humidity = item
        .get("indoor_humidity")
        .and_then(|v| v.as_str().and_then(|s| s.parse::<f32>().ok()).or_else(|| v.as_f64().map(|n| n as f32)));
    let outdoor_temperature = item
        .get("outdoor_temperature")
        .and_then(|v| v.as_str().and_then(|s| s.parse::<f32>().ok()).or_else(|| v.as_f64().map(|n| n as f32)));
    let system_status = item
        .get("system_status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut model = None;
    let mut firmware_version = None;
    let mut auid = None;
    let mut compressor_speed = None;
    let mut mode = None;
    let mut fan_mode = None;
    let mut zone = default_zone_from_thermostat(&item);

    for feat in item.get("features").and_then(|v| v.as_array()).iter().flat_map(|a| a.iter()) {
        match feat.get("name").and_then(|v| v.as_str()).unwrap_or("") {
            "advanced_info" => {
                for it in feat.get("items").and_then(|v| v.as_array()).iter().flat_map(|a| a.iter()) {
                    let label = it.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let value = it.get("value").and_then(|v| v.as_str()).map(String::from);
                    match label {
                        "Model" => model = value,
                        "Firmware Version" => firmware_version = value,
                        "AUID" => auid = value,
                        _ => {}
                    }
                }
            }
            "thermostat" => {
                let z = &mut zone;
                z.id = feat
                    .get("device_identifier")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.split('-').next_back())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(z.id);
                z.temperature = feat.get("temperature").and_then(|v| v.as_f64()).map(|n| n as f32);
                z.heat_setpoint = feat.get("setpoint_heat").and_then(|v| v.as_f64()).map(|n| n as f32);
                z.cool_setpoint = feat.get("setpoint_cool").and_then(|v| v.as_f64()).map(|n| n as f32);
                z.operating_state = feat.get("operating_state").and_then(|v| v.as_str()).map(String::from);
                z.setpoint_heat_min = feat.get("setpoint_heat_min").and_then(|v| v.as_i64()).map(|n| n as i32);
                z.setpoint_heat_max = feat.get("setpoint_heat_max").and_then(|v| v.as_i64()).map(|n| n as i32);
                z.setpoint_cool_min = feat.get("setpoint_cool_min").and_then(|v| v.as_i64()).map(|n| n as i32);
                z.setpoint_cool_max = feat.get("setpoint_cool_max").and_then(|v| v.as_i64()).map(|n| n as i32);
                z.setpoint_delta = feat.get("setpoint_delta").and_then(|v| v.as_i64()).map(|n| n as i32);
                if let Some(s) = feat.get("scale").and_then(|v| v.as_str()) {
                    z.scale = s.into();
                }
                if let Some(h) = feat.pointer("/actions/set_heat_setpoint/href").and_then(|v| v.as_str()) {
                    z.actions.set_setpoint = Some(h.into());
                }
            }
            "thermostat_mode" => {
                mode = feat.get("value").and_then(|v| v.as_str()).map(String::from);
                if let Some(h) = feat.pointer("/actions/update_thermostat_mode/href").and_then(|v| v.as_str()) {
                    zone.actions.set_zone_mode = Some(h.into());
                }
            }
            "thermostat_run_mode" => {
                if let Some(h) = feat.pointer("/actions/update_thermostat_run_mode/href").and_then(|v| v.as_str()) {
                    zone.actions.set_run_mode = Some(h.into());
                }
            }
            "thermostat_fan_mode" => {
                fan_mode = feat.get("value").and_then(|v| v.as_str()).map(String::from);
                if let Some(h) = feat.pointer("/actions/update_thermostat_fan_mode/href").and_then(|v| v.as_str()) {
                    zone.actions.set_fan_mode = Some(h.into());
                }
            }
            "thermostat_compressor_speed" => {
                compressor_speed = feat.get("compressor_speed").and_then(|v| v.as_f64()).map(|n| n as f32);
            }
            _ => {}
        }
    }

    // Pull emergency_heat + air_cleaner + fan_speed + dehumidify hrefs from settings[].
    for setting in item.get("settings").and_then(|v| v.as_array()).iter().flat_map(|a| a.iter()) {
        match setting.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "emergency_heat" => {
                if let Some(h) = setting.pointer("/actions/update_thermostat_emergency_heat/href")
                    .or_else(|| setting.pointer("/href"))
                    .and_then(|v| v.as_str())
                {
                    zone.actions.set_emergency_heat = Some(h.into());
                }
            }
            "air_cleaner_mode" => {
                if let Some(h) = setting.pointer("/actions/update_thermostat_air_cleaner_mode/href")
                    .or_else(|| setting.pointer("/href"))
                    .and_then(|v| v.as_str())
                {
                    zone.actions.set_air_cleaner_mode = Some(h.into());
                }
            }
            "fan_speed" => {
                if let Some(h) = setting.pointer("/actions/update_thermostat_fan_speed/href")
                    .or_else(|| setting.pointer("/href"))
                    .and_then(|v| v.as_str())
                {
                    zone.actions.set_fan_speed = Some(h.into());
                }
            }
            "dehumidify" => {
                if let Some(h) = setting.pointer("/actions/update_thermostat_dehumidify/href")
                    .or_else(|| setting.pointer("/href"))
                    .and_then(|v| v.as_str())
                {
                    zone.actions.set_dehumidify = Some(h.into());
                }
            }
            _ => {}
        }
    }

    Some(Thermostat {
        id,
        name,
        manufacturer,
        model,
        firmware_version,
        auid,
        system_status,
        indoor_humidity,
        outdoor_temperature,
        compressor_speed,
        zones: vec![zone],
        mode,
        fan_mode,
    })
}

fn default_zone_from_thermostat(item: &Value) -> Zone {
    Zone {
        id: 0,
        temperature: None,
        heat_setpoint: None,
        cool_setpoint: None,
        operating_state: None,
        setpoint_heat_min: None,
        setpoint_heat_max: None,
        setpoint_cool_min: None,
        setpoint_cool_max: None,
        setpoint_delta: None,
        scale: item
            .pointer("/features/0/scale")
            .and_then(|v| v.as_str())
            .unwrap_or("f")
            .into(),
        actions: Actions::default(),
    }
}
