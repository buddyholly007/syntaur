//! Capability discovery for commissioned Matter devices.
//!
//! Walks the Descriptor cluster on every endpoint and captures the
//! device-type label + supported controls in a form that's safe to
//! persist (serde-friendly) and safe to render to humans (uses
//! percentages and warm/cool labels, not raw `1..254` levels or
//! mireds).
//!
//! Two consumers:
//! - `matter-op caps` CLI — thin wrapper around `discover_capabilities`
//!   + [`DeviceCapabilities::render_human`].
//! - `syntaur-gateway` smart-home driver — calls `discover_capabilities`
//!   on commission success, persists the resulting `DeviceCapabilities`
//!   as JSON, and uses it to scope the per-device tile UI + the agent
//!   tool surface (no fake color picker on a CT-only bulb; no
//!   `set_color` tool offered to the agent for that device).

use rs_matter::im::client::ImClient;
use rs_matter::im::AttrResp;
use rs_matter::transport::exchange::Exchange;

use serde::{Deserialize, Serialize};

// ---------------- cluster + attribute IDs ----------------
//
// Re-defined here (instead of importing from matter-op) so the library
// is the canonical home — matter-op now imports these.

pub const CLUSTER_ON_OFF: u32 = 0x0006;
pub const CLUSTER_LEVEL_CONTROL: u32 = 0x0008;
pub const CLUSTER_DESCRIPTOR: u32 = 0x001D;
pub const CLUSTER_BASIC_INFO: u32 = 0x0028;
pub const CLUSTER_DOOR_LOCK: u32 = 0x0101;
pub const CLUSTER_WINDOW_COVERING: u32 = 0x0102;
pub const CLUSTER_THERMOSTAT: u32 = 0x0201;
pub const CLUSTER_FAN_CONTROL: u32 = 0x0202;
pub const CLUSTER_COLOR_CONTROL: u32 = 0x0300;
pub const CLUSTER_ILLUMINANCE_MEASUREMENT: u32 = 0x0400;
pub const CLUSTER_TEMPERATURE_MEASUREMENT: u32 = 0x0402;
pub const CLUSTER_RELATIVE_HUMIDITY_MEASUREMENT: u32 = 0x0405;
pub const CLUSTER_OCCUPANCY_SENSING: u32 = 0x0406;
pub const CLUSTER_ELEC_POWER: u32 = 0x0090;
pub const CLUSTER_ELEC_ENERGY: u32 = 0x0091;

const ATTR_DESCRIPTOR_DEVICE_TYPE_LIST: u32 = 0x0000;
const ATTR_DESCRIPTOR_SERVER_LIST: u32 = 0x0001;
const ATTR_DESCRIPTOR_PARTS_LIST: u32 = 0x0003;
const ATTR_FEATURE_MAP: u32 = 0xFFFC;

const ATTR_BASIC_VENDOR_NAME: u32 = 0x0001;
const ATTR_BASIC_VENDOR_ID: u32 = 0x0002;
const ATTR_BASIC_PRODUCT_NAME: u32 = 0x0003;
const ATTR_BASIC_PRODUCT_ID: u32 = 0x0004;
const ATTR_BASIC_SOFTWARE_VERSION_STRING: u32 = 0x000A;

const ATTR_LEVEL_MIN: u32 = 0x0002;
const ATTR_LEVEL_MAX: u32 = 0x0003;

const ATTR_COLOR_TEMP_PHYS_MIN_MIREDS: u32 = 0x400B;
const ATTR_COLOR_TEMP_PHYS_MAX_MIREDS: u32 = 0x400C;

// ColorControl FeatureMap bits.
const FEATURE_COLORCTRL_HS: u32 = 1 << 0;
const FEATURE_COLORCTRL_EHUE: u32 = 1 << 1;
const FEATURE_COLORCTRL_CL: u32 = 1 << 2;
const FEATURE_COLORCTRL_XY: u32 = 1 << 3;
const FEATURE_COLORCTRL_CT: u32 = 1 << 4;

// ---------------- public types ----------------

/// Top-level capability profile for a commissioned Matter device.
/// Stored as JSON in `smart_home_devices.capabilities_json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub vendor_name: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_name: Option<String>,
    pub product_id: Option<u16>,
    pub software_version: Option<String>,
    pub endpoints: Vec<EndpointCaps>,
}

/// Per-endpoint capability summary. Most consumer devices have a single
/// actionable endpoint (the bulb / plug / lock); composed devices like
/// energy plugs split power-meter onto a sibling endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointCaps {
    pub endpoint: u16,
    pub device_type_label: String,
    /// All device-type IDs declared on this endpoint (most-specific first
    /// preserved by Matter spec ordering).
    pub device_type_ids: Vec<u32>,
    pub controls: Vec<Control>,
}

/// One user-facing control surface on an endpoint. Each variant carries
/// the human-friendly bounds — `Brightness::max_pct = 100` always, no
/// raw 1-254 level leakage; `ColorTemp` carries Kelvin warm/cool, not
/// mireds. Raw fields are kept for diagnostic only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Control {
    Power,
    Brightness {
        min_pct: u8,
        max_pct: u8,
        raw_min: u8,
        raw_max: u8,
    },
    ColorTemp {
        warm_kelvin: u32,
        cool_kelvin: u32,
        raw_min_mireds: Option<u16>,
        raw_max_mireds: Option<u16>,
    },
    ColorPicker {
        color_spaces: Vec<String>,
    },
    Climate,
    Lock,
    WindowCover,
    Fan,
    Sensor(SensorKind),
    PowerMeter,
    EnergyMeter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Occupancy,
    Temperature,
    Humidity,
    Illuminance,
}

// ---------------- discovery ----------------

/// Walk Descriptor on each endpoint and read enough attributes to
/// summarize what a user can actually do with the device. Reuses the
/// caller's `Exchange` so a single CASE session handles every read.
pub async fn discover_capabilities(
    ex: &mut Exchange<'_>,
) -> DeviceCapabilities {
    // Nameplate (BasicInformation, ep 0).
    let vendor_name = read_str(ex, 0, CLUSTER_BASIC_INFO, ATTR_BASIC_VENDOR_NAME).await;
    let vendor_id = read_u16(ex, 0, CLUSTER_BASIC_INFO, ATTR_BASIC_VENDOR_ID).await;
    let product_name = read_str(ex, 0, CLUSTER_BASIC_INFO, ATTR_BASIC_PRODUCT_NAME).await;
    let product_id = read_u16(ex, 0, CLUSTER_BASIC_INFO, ATTR_BASIC_PRODUCT_ID).await;
    let software_version = read_str(ex, 0, CLUSTER_BASIC_INFO, ATTR_BASIC_SOFTWARE_VERSION_STRING).await;

    let mut endpoints: Vec<u16> = vec![0];
    endpoints.extend(read_u16_list(ex, 0, CLUSTER_DESCRIPTOR, ATTR_DESCRIPTOR_PARTS_LIST).await);

    let mut out = Vec::new();
    for ep in endpoints {
        let dev_types = read_device_type_ids(ex, ep).await;
        let servers = read_u32_list(ex, ep, CLUSTER_DESCRIPTOR, ATTR_DESCRIPTOR_SERVER_LIST).await;

        let mut controls: Vec<Control> = Vec::new();
        for &cid in INTERESTING_CLUSTERS {
            if !servers.contains(&cid) {
                continue;
            }
            if let Some(c) = render_cluster_control(ex, ep, cid).await {
                controls.push(c);
            }
        }
        if controls.is_empty() {
            continue;
        }

        let label = device_type_label(&dev_types, &servers).to_string();
        out.push(EndpointCaps {
            endpoint: ep,
            device_type_label: label,
            device_type_ids: dev_types,
            controls,
        });
    }

    DeviceCapabilities {
        vendor_name,
        vendor_id,
        product_name,
        product_id,
        software_version,
        endpoints: out,
    }
}

const INTERESTING_CLUSTERS: &[u32] = &[
    CLUSTER_ON_OFF,
    CLUSTER_LEVEL_CONTROL,
    CLUSTER_COLOR_CONTROL,
    CLUSTER_THERMOSTAT,
    CLUSTER_DOOR_LOCK,
    CLUSTER_WINDOW_COVERING,
    CLUSTER_FAN_CONTROL,
    CLUSTER_OCCUPANCY_SENSING,
    CLUSTER_TEMPERATURE_MEASUREMENT,
    CLUSTER_RELATIVE_HUMIDITY_MEASUREMENT,
    CLUSTER_ILLUMINANCE_MEASUREMENT,
    CLUSTER_ELEC_POWER,
    CLUSTER_ELEC_ENERGY,
];

async fn render_cluster_control(
    ex: &mut Exchange<'_>,
    ep: u16,
    cid: u32,
) -> Option<Control> {
    match cid {
        CLUSTER_ON_OFF => Some(Control::Power),
        CLUSTER_LEVEL_CONTROL => {
            let min = read_u8(ex, ep, CLUSTER_LEVEL_CONTROL, ATTR_LEVEL_MIN).await.unwrap_or(1);
            let max = read_u8(ex, ep, CLUSTER_LEVEL_CONTROL, ATTR_LEVEL_MAX).await.unwrap_or(254);
            Some(Control::Brightness {
                min_pct: level_to_pct(min, max),
                max_pct: level_to_pct(max, max),
                raw_min: min,
                raw_max: max,
            })
        }
        CLUSTER_COLOR_CONTROL => {
            let fmap = read_u32(ex, ep, CLUSTER_COLOR_CONTROL, ATTR_FEATURE_MAP).await.unwrap_or(0);
            let has_ct = fmap & FEATURE_COLORCTRL_CT != 0;
            let has_color = fmap
                & (FEATURE_COLORCTRL_HS
                    | FEATURE_COLORCTRL_XY
                    | FEATURE_COLORCTRL_EHUE
                    | FEATURE_COLORCTRL_CL)
                != 0;
            if has_color {
                let mut spaces = Vec::new();
                if fmap & FEATURE_COLORCTRL_HS != 0 {
                    spaces.push("hue_sat".to_string());
                }
                if fmap & FEATURE_COLORCTRL_EHUE != 0 {
                    spaces.push("enhanced_hue".to_string());
                }
                if fmap & FEATURE_COLORCTRL_XY != 0 {
                    spaces.push("xy".to_string());
                }
                if fmap & FEATURE_COLORCTRL_CL != 0 {
                    spaces.push("color_loop".to_string());
                }
                // ColorPicker is the dominant control if both are present —
                // most apps expose CT as a sub-mode of the picker.
                return Some(Control::ColorPicker { color_spaces: spaces });
            }
            if has_ct {
                let pmin = read_u16(ex, ep, CLUSTER_COLOR_CONTROL, ATTR_COLOR_TEMP_PHYS_MIN_MIREDS).await;
                let pmax = read_u16(ex, ep, CLUSTER_COLOR_CONTROL, ATTR_COLOR_TEMP_PHYS_MAX_MIREDS).await;
                let (warm_k, cool_k) = match (pmin, pmax) {
                    (Some(mn), Some(mx)) if mn > 0 && mx > 0 => (
                        1_000_000u32 / mx as u32,
                        1_000_000u32 / mn as u32,
                    ),
                    _ => (2700, 6500), // sensible defaults if attrs missing
                };
                return Some(Control::ColorTemp {
                    warm_kelvin: warm_k,
                    cool_kelvin: cool_k,
                    raw_min_mireds: pmin,
                    raw_max_mireds: pmax,
                });
            }
            None
        }
        CLUSTER_THERMOSTAT => Some(Control::Climate),
        CLUSTER_DOOR_LOCK => Some(Control::Lock),
        CLUSTER_WINDOW_COVERING => Some(Control::WindowCover),
        CLUSTER_FAN_CONTROL => Some(Control::Fan),
        CLUSTER_OCCUPANCY_SENSING => Some(Control::Sensor(SensorKind::Occupancy)),
        CLUSTER_TEMPERATURE_MEASUREMENT => Some(Control::Sensor(SensorKind::Temperature)),
        CLUSTER_RELATIVE_HUMIDITY_MEASUREMENT => Some(Control::Sensor(SensorKind::Humidity)),
        CLUSTER_ILLUMINANCE_MEASUREMENT => Some(Control::Sensor(SensorKind::Illuminance)),
        CLUSTER_ELEC_POWER => Some(Control::PowerMeter),
        CLUSTER_ELEC_ENERGY => Some(Control::EnergyMeter),
        _ => None,
    }
}

fn level_to_pct(level: u8, max: u8) -> u8 {
    let max = (max as u32).max(1);
    let l = level as u32;
    ((l * 100 + max / 2) / max).min(100) as u8
}

fn device_type_label(types: &[u32], servers: &[u32]) -> &'static str {
    // Most-specific recognized device type wins. Matter Application
    // Cluster Library §1.5.
    for &t in types {
        match t {
            0x010D => return "Extended Color Light",
            0x010C => return "Color Temperature Light",
            0x0101 => return "Dimmable Light",
            0x0100 => return "On/Off Light",
            0x010B => return "Dimmable Plug-in Unit",
            0x010A => return "On/Off Plug-in Unit",
            0x0202 => return "Window Covering",
            0x000A => return "Door Lock",
            0x0301 => return "Thermostat",
            0x002B => return "Fan",
            0x0107 => return "Occupancy Sensor",
            0x0302 => return "Temperature Sensor",
            0x0307 => return "Humidity Sensor",
            0x0106 => return "Light Sensor",
            0x0510 => return "Electrical Sensor",
            0x0850 => return "Smart Plug (Energy)",
            _ => {}
        }
    }
    // Heuristic fallback: Meross/Eve put energy clusters on a separate
    // unlabeled endpoint with no DeviceTypeList entries we recognize.
    if servers.contains(&CLUSTER_ELEC_POWER) || servers.contains(&CLUSTER_ELEC_ENERGY) {
        return "Power Meter";
    }
    "Device"
}

// ---------------- TLV helpers ----------------

async fn read_str(ex: &mut Exchange<'_>, ep: u16, cluster: u32, attr: u32) -> Option<String> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => d.data.utf8().ok().map(|s| s.to_string()),
        _ => None,
    }
}
async fn read_u8(ex: &mut Exchange<'_>, ep: u16, cluster: u32, attr: u32) -> Option<u8> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => d.data.u8().ok(),
        _ => None,
    }
}
async fn read_u16(ex: &mut Exchange<'_>, ep: u16, cluster: u32, attr: u32) -> Option<u16> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => d.data.u16().ok(),
        _ => None,
    }
}
async fn read_u32(ex: &mut Exchange<'_>, ep: u16, cluster: u32, attr: u32) -> Option<u32> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => d.data.u32().ok(),
        _ => None,
    }
}
async fn read_u16_list(
    ex: &mut Exchange<'_>,
    ep: u16,
    cluster: u32,
    attr: u32,
) -> Vec<u16> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => match d.data.array() {
            Ok(arr) => arr
                .iter()
                .filter_map(|e| e.ok().and_then(|x| x.u16().ok()))
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}
async fn read_u32_list(
    ex: &mut Exchange<'_>,
    ep: u16,
    cluster: u32,
    attr: u32,
) -> Vec<u32> {
    match ImClient::read_single_attr(ex, ep, cluster, attr, false).await {
        Ok(AttrResp::Data(d)) => match d.data.array() {
            Ok(arr) => arr
                .iter()
                .filter_map(|e| e.ok().and_then(|x| x.u32().ok()))
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}
async fn read_device_type_ids(ex: &mut Exchange<'_>, ep: u16) -> Vec<u32> {
    match ImClient::read_single_attr(
        ex,
        ep,
        CLUSTER_DESCRIPTOR,
        ATTR_DESCRIPTOR_DEVICE_TYPE_LIST,
        false,
    )
    .await
    {
        Ok(AttrResp::Data(d)) => match d.data.array() {
            Ok(arr) => arr
                .iter()
                .filter_map(|e| {
                    let elem = e.ok()?;
                    let s = elem.r#struct().ok()?;
                    let t = s.find_ctx(0).ok()?;
                    t.u32().or_else(|_| t.u16().map(|v| v as u32)).ok()
                })
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

// ---------------- human rendering ----------------

impl DeviceCapabilities {
    /// Multi-line summary formatted the way users actually think about
    /// devices — percentages, warm/cool labels, no raw protocol levels.
    /// Used by `matter-op caps` and as the body of any "see what this
    /// device can do" UI tooltip.
    pub fn render_human(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(
            &mut s,
            "\n{} {} (VID {} PID {}, sw {})",
            self.vendor_name.as_deref().unwrap_or("?"),
            self.product_name.as_deref().unwrap_or("?"),
            self.vendor_id
                .map(|v| format!("{:#06x}", v))
                .unwrap_or_else(|| "?".into()),
            self.product_id
                .map(|v| format!("{:#06x}", v))
                .unwrap_or_else(|| "?".into()),
            self.software_version.as_deref().unwrap_or("?"),
        );
        for ep in &self.endpoints {
            let _ = writeln!(&mut s, "\n  Endpoint {}: {}", ep.endpoint, ep.device_type_label);
            for c in &ep.controls {
                let _ = writeln!(&mut s, "    {}", render_control_human(c));
            }
        }
        s
    }
}

fn render_control_human(c: &Control) -> String {
    match c {
        Control::Power => "Power      ON / OFF".into(),
        Control::Brightness { min_pct, max_pct, raw_min, raw_max } => {
            format!("Brightness {}%–{}%  (raw level {}–{})", min_pct, max_pct, raw_min, raw_max)
        }
        Control::ColorTemp { warm_kelvin, cool_kelvin, .. } => {
            format!("Color temp Warm ←——→ Cool  ({} K – {} K)", warm_kelvin, cool_kelvin)
        }
        Control::ColorPicker { color_spaces } => {
            if color_spaces.is_empty() {
                "Color      Full color picker".into()
            } else {
                format!("Color      Full color picker  ({})", color_spaces.join(", "))
            }
        }
        Control::Climate => "Climate    Heat / Cool / Auto + setpoints".into(),
        Control::Lock => "Lock       Lock / Unlock".into(),
        Control::WindowCover => "Cover      Open / Close + position".into(),
        Control::Fan => "Fan        Speed control".into(),
        Control::Sensor(SensorKind::Occupancy) => "Sensor     Occupancy (motion)".into(),
        Control::Sensor(SensorKind::Temperature) => "Sensor     Temperature".into(),
        Control::Sensor(SensorKind::Humidity) => "Sensor     Humidity".into(),
        Control::Sensor(SensorKind::Illuminance) => "Sensor     Light level".into(),
        Control::PowerMeter => "Energy     Live power (W)".into(),
        Control::EnergyMeter => "Energy     Cumulative (Wh)".into(),
    }
}
