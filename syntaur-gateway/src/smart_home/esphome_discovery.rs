//! ESPHome network discovery + capability classifier.
//!
//! Powers the `/smart-home/esphome` quick-setup wizard. Browses
//! `_esphomelib._tcp.local.` via mDNS, parses each device's TXT record
//! (esphome_version, board, project_name, friendly_name, mac), and
//! returns a rich `DiscoveredEsphomeDevice` plus a recommended role.
//!
//! ## Why a focused browse instead of leaning on `wifi_lan::mdns_sweep`
//!
//! The unified sweep already lists `_esphomelib._tcp.local.` for adoption
//! into `smart_home_devices`, but it normalizes every service into a
//! generic `ScanCandidate` shape — fields like `board` and the
//! bluetooth-proxy / voice-assistant capability hints are dropped.
//! The wizard's whole point is "tell the user what each device IS", so
//! it needs the unflattened TXT.
//!
//! ## Role classifier
//!
//! Best-effort heuristic from the device's own self-description:
//!   - Name or friendly_name contains "bt"/"ble"/"proxy"  → BtProxyActive
//!   - project_name contains "satellite" or "voice"        → VoiceSatellite
//!   - project_name contains "presence" or "mmwave"        → PresenceMmwave
//!   - everything else (default for fresh ESP32-class hw)  → BtProxyActive
//!
//! Default is BtProxyActive because every ESP32/S2/S3/C3/C6 has a BT
//! radio capable of acting as a proxy, and that's the role that
//! maximises data-collected (advert firehose + GATT relay). The user
//! sees the recommendation alongside the rationale and can pick a
//! different role from the dropdown.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One ESPHome device found via mDNS, with TXT-record fields surfaced.
/// `host` is the device's IPv4 (or v6) address; `port` is its native
/// API port (always 6053 in stock ESPHome but read from the SRV record
/// to be safe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredEsphomeDevice {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub esphome_version: Option<String>,
    pub board: Option<String>,
    pub project_name: Option<String>,
    pub project_version: Option<String>,
    pub friendly_name: Option<String>,
    pub mac: Option<String>,
    pub network: Option<String>,
    /// Currently-installed role hints from project_name + friendly_name.
    /// Distinct from `recommended_role` which is what the wizard wants
    /// the device to BECOME.
    pub current_role_hints: Vec<String>,
    pub recommended_role: SuggestedRole,
    pub recommendation_reason: String,
}

/// Firmware role categories the wizard can install. Each maps to a
/// pre-compiled `.bin` artifact (board × role) shipped with the
/// gateway, applied via the ESPHome native-API OTA stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SuggestedRole {
    /// Active scanning + bluetooth_proxy + BTHome reception. Maximum
    /// data, recommended default for any ESP32-class radio.
    BtProxyActive,
    /// Passive scanning only — lower power, no GATT relay. Useful for
    /// battery proxies (rare) or when the user explicitly wants a
    /// scan-only role.
    BtProxyPassive,
    /// Voice-assistant satellite (mic + speaker). Sat1, ESP32-S3 boxes.
    VoiceSatellite,
    /// mmWave + BLE presence. Common bedroom / bathroom role.
    PresenceMmwave,
    /// We can't tell. UI should show the rationale + an "advanced"
    /// dropdown to pick manually.
    Unknown,
}

impl SuggestedRole {
    pub fn label(self) -> &'static str {
        match self {
            SuggestedRole::BtProxyActive => "BLE proxy (active)",
            SuggestedRole::BtProxyPassive => "BLE proxy (passive)",
            SuggestedRole::VoiceSatellite => "Voice satellite",
            SuggestedRole::PresenceMmwave => "Presence (mmWave + BLE)",
            SuggestedRole::Unknown => "Unknown",
        }
    }
}

/// Scan for ESPHome devices for `duration`. Returns each unique device
/// with its TXT record parsed and a role recommendation pre-computed.
/// Wraps the blocking `mdns_sd` daemon in `spawn_blocking` so the async
/// caller stays responsive.
pub async fn discover(duration: Duration) -> Vec<DiscoveredEsphomeDevice> {
    tokio::task::spawn_blocking(move || discover_blocking(duration))
        .await
        .unwrap_or_default()
}

fn discover_blocking(duration: Duration) -> Vec<DiscoveredEsphomeDevice> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("[smart_home::esphome_discovery] daemon: {e}");
            return Vec::new();
        }
    };
    let receiver = match daemon.browse("_esphomelib._tcp.local.") {
        Ok(rx) => rx,
        Err(e) => {
            log::warn!("[smart_home::esphome_discovery] browse: {e}");
            let _ = daemon.shutdown();
            return Vec::new();
        }
    };

    let deadline = Instant::now() + duration;
    // Key by fullname (mDNS-unique instance name) so two devices that
    // happen to share a default hostname like "esphome-web" both
    // surface in the wizard. MAC would also work but isn't always in
    // the TXT record.
    let mut found: HashMap<String, DiscoveredEsphomeDevice> = HashMap::new();
    while Instant::now() < deadline {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(200));
        match receiver.recv_timeout(wait) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let key = info.get_fullname().to_string();
                if let Some(d) = device_from_info(&info) {
                    found.insert(key, d);
                }
            }
            Ok(_) => {}
            Err(_) => {
                // timeout on this poll — keep looping until deadline
            }
        }
    }
    let _ = daemon.shutdown();
    found.into_values().collect()
}

fn device_from_info(info: &mdns_sd::ServiceInfo) -> Option<DiscoveredEsphomeDevice> {
    let name = info.get_fullname().to_string();
    // Strip the trailing service type so the name is just the host
    // label (e.g. "proxy-kids._esphomelib._tcp.local." → "proxy-kids").
    let short_name = info
        .get_hostname()
        .trim_end_matches('.')
        .trim_end_matches(".local")
        .to_string();
    let port = info.get_port();
    // Prefer IPv4: ESPHome native API is reliably reachable over v4 on
    // every home network, and link-local v6 addresses (fe80::) often
    // surface from mdns_sd but aren't routable from the gateway. Fall
    // back to whatever the daemon gave us if no v4 address came back.
    let addresses: Vec<_> = info.get_addresses().iter().collect();
    let host = addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addresses.first())
        .map(|a| a.to_string())
        .unwrap_or_default();
    if host.is_empty() {
        return None;
    }

    let txt = info.get_properties();
    let get = |k: &str| {
        txt.get_property_val_str(k)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    let esphome_version = get("version");
    let board = get("board");
    let project_name = get("project_name");
    let project_version = get("project_version");
    let friendly_name = get("friendly_name");
    let mac = get("mac");
    let network = get("network");

    let mut current_role_hints = Vec::new();
    let inferred = classify(
        &short_name,
        friendly_name.as_deref(),
        project_name.as_deref(),
        &mut current_role_hints,
    );

    Some(DiscoveredEsphomeDevice {
        name: short_name,
        host,
        port,
        esphome_version,
        board,
        project_name,
        project_version,
        friendly_name,
        mac,
        network,
        current_role_hints,
        recommended_role: inferred.0,
        recommendation_reason: inferred.1,
    })
}

/// Pick a recommended role + rationale string. Free function so tests
/// can exercise it directly. `current_role_hints` is appended into,
/// not replaced — ESPHome devices commonly self-identify with multiple
/// labels.
pub fn classify(
    name: &str,
    friendly_name: Option<&str>,
    project_name: Option<&str>,
    current_role_hints: &mut Vec<String>,
) -> (SuggestedRole, String) {
    let blob = format!(
        "{} {} {}",
        name.to_ascii_lowercase(),
        friendly_name.unwrap_or("").to_ascii_lowercase(),
        project_name.unwrap_or("").to_ascii_lowercase(),
    );

    let says_bt_proxy = blob.contains("bt proxy")
        || blob.contains("ble proxy")
        || blob.contains("bt-proxy")
        || blob.contains("ble-proxy")
        || blob.contains("bluetooth_proxy")
        || blob.contains("bluetoothproxy");
    let says_voice = blob.contains("satellite")
        || blob.contains("voice")
        || blob.contains("respeaker")
        || blob.contains("box-3");
    let says_presence = blob.contains("presence") || blob.contains("mmwave") || blob.contains("ld2410");

    if says_bt_proxy {
        current_role_hints.push("bt-proxy".into());
    }
    if says_voice {
        current_role_hints.push("voice".into());
    }
    if says_presence {
        current_role_hints.push("presence".into());
    }

    if says_voice && says_bt_proxy {
        return (
            SuggestedRole::VoiceSatellite,
            "device self-identifies as a voice satellite; voice + BT-proxy combined firmware exists, but voice is the higher-value role".into(),
        );
    }
    if says_voice {
        return (
            SuggestedRole::VoiceSatellite,
            "device self-identifies as a voice satellite".into(),
        );
    }
    if says_presence {
        return (
            SuggestedRole::PresenceMmwave,
            "device self-identifies as a presence/mmWave sensor".into(),
        );
    }
    if says_bt_proxy {
        return (
            SuggestedRole::BtProxyActive,
            "device self-identifies as a BT proxy; recommending the active firmware to maximise data collection".into(),
        );
    }
    // No self-identification: every ESP32-class chip ships with BT, so
    // the highest-data-yield default is the active BT proxy. Users can
    // override.
    (
        SuggestedRole::BtProxyActive,
        "no role hints in mDNS metadata; defaulting to active BT proxy because every ESP32-class radio supports it and it maximises data collected for the household".into(),
    )
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bt_proxy_self_identification() {
        let mut hints = Vec::new();
        let (role, _why) = classify("proxy-kids", Some("Kids Bath BT Proxy"), None, &mut hints);
        assert_eq!(role, SuggestedRole::BtProxyActive);
        assert!(hints.contains(&"bt-proxy".to_string()));
    }

    #[test]
    fn classify_voice_self_identification() {
        let mut hints = Vec::new();
        let (role, _why) = classify(
            "satellite1-918358",
            Some("Satellite 1"),
            Some("futureproof.satellite1"),
            &mut hints,
        );
        assert_eq!(role, SuggestedRole::VoiceSatellite);
        assert!(hints.contains(&"voice".to_string()));
    }

    #[test]
    fn classify_default_recommends_bt_proxy() {
        let mut hints = Vec::new();
        let (role, why) = classify("kitchen-temp", Some("Kitchen Temp"), None, &mut hints);
        assert_eq!(role, SuggestedRole::BtProxyActive);
        assert!(why.contains("maximises") || why.contains("default"));
        assert!(hints.is_empty());
    }

    #[test]
    fn classify_presence_self_identification() {
        let mut hints = Vec::new();
        let (role, _why) = classify("bedroom-mmwave", None, Some("ld2410-presence"), &mut hints);
        assert_eq!(role, SuggestedRole::PresenceMmwave);
        assert!(hints.contains(&"presence".to_string()));
    }

    #[test]
    fn role_label_strings() {
        // Sanity: labels are non-empty so the wizard UI never renders blanks.
        for r in [
            SuggestedRole::BtProxyActive,
            SuggestedRole::BtProxyPassive,
            SuggestedRole::VoiceSatellite,
            SuggestedRole::PresenceMmwave,
            SuggestedRole::Unknown,
        ] {
            assert!(!r.label().is_empty());
        }
    }
}
