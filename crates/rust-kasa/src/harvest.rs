//! **One-time** inventory builder for Tapo / Kasa KLAP devices.
//!
//! Unlike aidot, TP-Link's cloud API does NOT return a device list —
//! it just authenticates the user. Device discovery is LAN-local:
//! either UDP broadcast on port 20002 (newer Tapo) or HTTP probe of a
//! user-supplied IP list. We take the IP-list route because it's
//! VLAN-portable (no broadcasts needed) and fits the "user provides
//! credentials, Syntaur provides the rest" UX.
//!
//! Under the hood `harvest_from_ips` does a fresh KLAP handshake +
//! `get_device_info` call per IP, pulls the alias / model / MAC /
//! device_id, and folds into an [`Inventory`]. Credentials are
//! validated at the first IP (handshake1 verifies `auth_hash`).

use serde::Deserialize;

use crate::{Device, Inventory, InventoryDevice, KasaError};

#[derive(Debug, Deserialize)]
struct DeviceInfoRaw {
    // Tapo's get_device_info returns all of these; we only surface a
    // few into the canonical Inventory shape. Fields are optional
    // because other TP-Link SKUs (bulbs, plugs, cameras) use subset.
    #[serde(default)]
    mac: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
    /// Tapo returns the friendly name base64-encoded under `nickname`,
    /// plus a plain-text copy in certain responses. We accept either.
    #[serde(default)]
    nickname: Option<String>,
}

/// Build an inventory by connecting to each IP, doing KLAP handshake,
/// reading `get_device_info`. If any one IP errors, skipped (logged on
/// stderr when `KASA_VERBOSE` is set) — partial inventories are
/// preferred over bailing completely.
pub async fn harvest_from_ips(
    username: &str,
    password: &str,
    ips: &[String],
) -> Result<Inventory, KasaError> {
    let verbose = std::env::var("KASA_VERBOSE").is_ok();
    let mut devices: Vec<InventoryDevice> = Vec::new();
    for ip in ips {
        match probe_one(ip, username, password).await {
            Ok(d) => {
                if verbose {
                    eprintln!("[kasa] harvested {ip} -> {} ({})", d.alias, d.model);
                }
                devices.push(d);
            }
            Err(e) => {
                if verbose {
                    eprintln!("[kasa] skip {ip}: {e}");
                }
            }
        }
    }
    Ok(Inventory {
        username: username.into(),
        password: password.into(),
        devices,
    })
}

async fn probe_one(
    ip: &str,
    username: &str,
    password: &str,
) -> Result<InventoryDevice, KasaError> {
    let mut client = Device::connect(ip, username, password).await?;
    let v = client.query("get_device_info", None).await?;
    let raw: DeviceInfoRaw = serde_json::from_value(v)?;
    // Tapo's nickname field is base64-encoded UTF-8; try both shapes.
    let alias = raw
        .nickname
        .as_deref()
        .and_then(decode_alias)
        .unwrap_or_else(|| ip.to_string());
    Ok(InventoryDevice {
        ip: ip.to_string(),
        mac: raw.mac.unwrap_or_default(),
        alias,
        model: raw.model.unwrap_or_default(),
        device_id: raw.device_id.unwrap_or_default(),
    })
}

fn decode_alias(s: &str) -> Option<String> {
    // Try base64 first (the TP-Link newer encoding); fall back to plain.
    if let Ok(bytes) = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        s.as_bytes(),
    ) {
        if let Ok(decoded) = String::from_utf8(bytes) {
            if decoded.chars().all(|c| !c.is_control()) && !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
