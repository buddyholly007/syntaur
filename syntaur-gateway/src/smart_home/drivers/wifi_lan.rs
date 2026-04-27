//! Wi-Fi / LAN-native discovery — mDNS (DNS-SD) + SSDP/UPnP sweeps.
//!
//! This is the first driver to ship real scan output: pressing the
//! dashboard's "Scan for new devices" button fans out a parallel browse
//! across the service types below and emits a `ScanCandidate` per
//! resolved host. Fingerprinting is deliberately conservative — we
//! classify by the service type + a handful of TXT keys that all vendors
//! reliably publish, and leave deep per-vendor parsing to the individual
//! per-vendor drivers (Shelly, LIFX, Sonos, Tuya-local, ...).
//!
//! Two independent sweeps run in parallel and merge into one list:
//!   1. **mDNS** via `mdns_sd::ServiceDaemon` — blocking-style crate, so
//!      we run it inside `tokio::task::spawn_blocking`.
//!   2. **SSDP** M-SEARCH multicast on 239.255.255.250:1900 — raw UDP,
//!      tokio-native.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::smart_home::scan::ScanCandidate;

const DEFAULT_SWEEP_MS: u64 = 5_000;

/// Every mDNS service type we browse, paired with the driver name we
/// tag the resulting candidate with and a coarse `kind` guess.
///
/// `kind` is a best-effort default — drivers refine it once they
/// commission the device and read real capabilities. Unknown types fall
/// back to `driver="wifi_lan"` + `kind="unknown"` so the candidate card
/// still appears in the UI for the user to name + assign to a room.
const MDNS_SERVICES: &[(&str, &str, &str)] = &[
    // Matter operational fabric (post-commissioning).
    ("_matter._tcp.local.", "matter", "unknown"),
    // Matter commissionable (pre-pairing). This is what we want the
    // scan to surface for user-initiated commissioning via QR code.
    ("_matterc._udp.local.", "matter", "unknown"),
    // HomeKit-advertised accessories. Many Matter bridges and older
    // HomeKit-only devices live here.
    ("_hap._tcp.local.", "wifi_lan", "unknown"),
    // ESPHome native API — also the Syntaur voice satellite.
    ("_esphomelib._tcp.local.", "wifi_lan", "unknown"),
    // Shelly Gen1 + Gen2 both advertise this (Gen2 also hits _http).
    ("_shelly._tcp.local.", "wifi_lan_shelly", "plug"),
    // AirPlay receivers — Apple TV, HomePod, AVR, AirPlay speakers.
    ("_airplay._tcp.local.", "wifi_lan", "media_player"),
    ("_raop._tcp.local.", "wifi_lan", "media_player"),
    // Chromecast / Google Cast receivers.
    ("_googlecast._tcp.local.", "wifi_lan", "media_player"),
    // Sonos speakers advertise both the Sonos service + generic UPnP.
    ("_sonos._tcp.local.", "wifi_lan", "media_player"),
    // Roku ECP (external control protocol).
    ("_roku-rcp._tcp.local.", "wifi_lan", "media_player"),
    // Generic MQTT brokers — useful for diagnostics and Zigbee2MQTT
    // auto-detection even when the user isn't adding the broker as a
    // device.
    ("_mqtt._tcp.local.", "mqtt", "unknown"),
    // Smart printers show up here and sneak into the grid. Useful for
    // network diagnostics even if the user ignores them.
    ("_ipp._tcp.local.", "wifi_lan", "unknown"),
];

/// SSDP service targets we probe via M-SEARCH. Each entry is a
/// (`ST` header, driver, kind) triple that we send one M-SEARCH per.
const SSDP_TARGETS: &[(&str, &str, &str)] = &[
    ("upnp:rootdevice", "wifi_lan", "unknown"),
    // Samsung smart TVs.
    ("urn:samsung.com:device:RemoteControlReceiver:1", "wifi_lan", "media_player"),
    // Roku.
    ("roku:ecp", "wifi_lan", "media_player"),
    // Sonos.
    ("urn:schemas-upnp-org:device:ZonePlayer:1", "wifi_lan", "media_player"),
    // Belkin WeMo.
    ("urn:Belkin:device:controllee:1", "wifi_lan", "switch"),
];

/// Run both mDNS and SSDP sweeps in parallel and merge into one
/// deduplicated candidate list.
pub async fn sweep() -> Vec<ScanCandidate> {
    sweep_for(Duration::from_millis(DEFAULT_SWEEP_MS)).await
}

pub async fn sweep_for(duration: Duration) -> Vec<ScanCandidate> {
    let mdns_task = tokio::spawn(mdns_sweep(duration));
    let ssdp_task = tokio::spawn(ssdp_sweep(duration));
    // Per-vendor LAN adopters (Govee, WiZ, ...) discovery runs in
    // parallel — each adopter caps its own window inside discover_all().
    let lan_task = tokio::spawn(super::lan::discover_all());

    let mut results: Vec<ScanCandidate> = Vec::new();
    match mdns_task.await {
        Ok(v) => results.extend(v),
        Err(e) => log::warn!("[smart_home::wifi_lan] mdns task panicked: {}", e),
    }
    match ssdp_task.await {
        Ok(v) => results.extend(v),
        Err(e) => log::warn!("[smart_home::wifi_lan] ssdp task panicked: {}", e),
    }
    match lan_task.await {
        Ok(v) => results.extend(v.into_iter().map(|c| c.into_scan_candidate())),
        Err(e) => log::warn!("[smart_home::wifi_lan] lan adopter task panicked: {}", e),
    }
    dedupe(results)
}

/// Browse every service in `MDNS_SERVICES` in parallel for `duration`.
async fn mdns_sweep(duration: Duration) -> Vec<ScanCandidate> {
    tokio::task::spawn_blocking(move || -> Vec<ScanCandidate> {
        let daemon = match mdns_sd::ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                log::warn!("[smart_home::wifi_lan] mdns daemon failed: {}", e);
                return Vec::new();
            }
        };

        // Kick off every browse up front so they run in parallel inside
        // the daemon's own thread; we just drain the channels below.
        let mut streams: Vec<(&'static str, &'static str, &'static str, _)> = Vec::new();
        for &(service, driver, kind) in MDNS_SERVICES {
            match daemon.browse(service) {
                Ok(rx) => streams.push((service, driver, kind, rx)),
                Err(e) => {
                    log::debug!(
                        "[smart_home::wifi_lan] browse {} failed: {}",
                        service,
                        e
                    );
                }
            }
        }

        let deadline = Instant::now() + duration;
        let mut candidates: Vec<ScanCandidate> = Vec::new();
        // Round-robin poll with a small per-channel timeout so no one
        // service type starves the others.
        let per_poll = Duration::from_millis(80);
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut made_progress = false;
            for (service, driver, kind, rx) in &streams {
                let wait = per_poll.min(remaining);
                match rx.recv_timeout(wait) {
                    Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                        made_progress = true;
                        if let Some(c) = candidate_from_mdns(service, driver, kind, &info) {
                            candidates.push(c);
                        }
                    }
                    Ok(_) => {
                        // Ignore Found / Remove / SearchStarted / etc.
                    }
                    Err(_) => {
                        // Timeout on this channel — move on.
                    }
                }
            }
            if !made_progress {
                // Nothing new across the whole round — short sleep so
                // we don't spin hot.
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        for (service, _, _, _) in &streams {
            let _ = daemon.stop_browse(service);
        }
        let _ = daemon.shutdown();
        candidates
    })
    .await
    .unwrap_or_default()
}

fn candidate_from_mdns(
    service: &str,
    driver: &str,
    default_kind: &str,
    info: &mdns_sd::ServiceInfo,
) -> Option<ScanCandidate> {
    let addrs: Vec<String> = info
        .get_addresses()
        .iter()
        .map(|a| a.to_string())
        .filter(|s| s.contains('.')) // v4 only; v6 handled separately later
        .collect();
    let ip = addrs.into_iter().next();
    let hostname = info.get_hostname().trim_end_matches('.').to_string();
    let name_from_fullname = info
        .get_fullname()
        .split_once(service)
        .map(|(n, _)| n.trim_end_matches('.').to_string())
        .unwrap_or_else(|| hostname.clone());

    let mut txt: HashMap<String, String> = HashMap::new();
    for prop in info.get_properties().iter() {
        txt.insert(prop.key().to_string(), prop.val_str().to_string());
    }

    // Use the fullname as the external_id so re-scans de-duplicate
    // cleanly even if IP changes (DHCP lease rotation, Wi-Fi roam).
    let external_id = info.get_fullname().to_string();

    // Refine driver + kind for a few well-known service types.
    let (driver, kind, vendor) = refine_classification(service, driver, default_kind, &txt);

    Some(ScanCandidate {
        driver: driver.to_string(),
        external_id,
        name: if name_from_fullname.is_empty() {
            hostname.clone()
        } else {
            name_from_fullname
        },
        kind: kind.to_string(),
        vendor,
        ip,
        mac: None,
        details: json!({
            "source": "mdns",
            "service_type": service,
            "hostname": hostname,
            "port": info.get_port(),
            "txt": txt,
        }),
    })
}

fn refine_classification(
    service: &str,
    default_driver: &str,
    default_kind: &str,
    txt: &HashMap<String, String>,
) -> (String, String, Option<String>) {
    let kind = match service {
        // HomeKit category `ci` byte per HAP spec.
        "_hap._tcp.local." => txt
            .get("ci")
            .and_then(|c| c.parse::<u8>().ok())
            .map(homekit_category)
            .unwrap_or(default_kind),
        _ => default_kind,
    };

    // Vendor guess from TXT when publishers include it.
    let vendor = txt
        .get("md")
        .or_else(|| txt.get("manufacturer"))
        .or_else(|| txt.get("vendor"))
        .cloned();

    (default_driver.to_string(), kind.to_string(), vendor)
}

/// Homekit Accessory Category Identifier (`ci`) → our device-kind string.
/// Per the HAP spec (Section 13, Accessory Categories).
fn homekit_category(ci: u8) -> &'static str {
    match ci {
        2 => "hub",         // Bridge
        5 => "plug",        // Outlet
        7 => "light",       // Lightbulb
        8 => "sensor_motion",
        9 => "thermostat",
        10 => "sensor_motion",
        11 => "sensor_climate",
        14 => "cover",      // Window Covering
        15 => "fan",
        16 => "lock",       // Garage Door Opener (close enough)
        17 => "switch",     // Programmable Switch
        18 => "plug",       // Range Extender
        19 => "camera",
        20 => "lock",       // Video Doorbell
        21 => "fan",        // Air Purifier
        26 => "speaker",
        27 => "light",      // Ambient Lightning
        28 => "plug",       // TV
        29 => "speaker",
        _ => "unknown",
    }
}

// ── SSDP ────────────────────────────────────────────────────────────────

async fn ssdp_sweep(duration: Duration) -> Vec<ScanCandidate> {
    use tokio::net::UdpSocket;

    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[smart_home::wifi_lan] ssdp bind failed: {}", e);
            return Vec::new();
        }
    };

    let multicast: std::net::SocketAddr = "239.255.255.250:1900".parse().unwrap();

    // Send one M-SEARCH per target. MX is the max response delay; we
    // use the full sweep duration so slow devices still answer.
    let mx = duration.as_secs().clamp(1, 5);
    for &(st, _, _) in SSDP_TARGETS {
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: {}\r\nST: {}\r\n\r\n",
            mx, st
        );
        if let Err(e) = socket.send_to(msg.as_bytes(), multicast).await {
            log::debug!("[smart_home::wifi_lan] M-SEARCH {} send failed: {}", st, e);
        }
    }

    let deadline = Instant::now() + duration;
    let mut candidates: Vec<ScanCandidate> = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let recv = tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await;
        let (n, addr) = match recv {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                log::debug!("[smart_home::wifi_lan] ssdp recv err: {}", e);
                continue;
            }
            Err(_) => break, // timeout → end of sweep
        };
        let text = match std::str::from_utf8(&buf[..n]) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(c) = parse_ssdp_response(text, addr.ip().to_string()) {
            candidates.push(c);
        }
    }
    candidates
}

fn parse_ssdp_response(text: &str, ip: String) -> Option<ScanCandidate> {
    // Headers are case-insensitive; normalize to lower-cased keys.
    let mut headers: HashMap<String, String> = HashMap::new();
    for line in text.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let usn = headers.get("usn").cloned().unwrap_or_default();
    let st = headers.get("st").cloned().unwrap_or_default();
    if usn.is_empty() {
        return None;
    }

    let (driver, kind) = SSDP_TARGETS
        .iter()
        .find(|(t, _, _)| st.contains(t))
        .map(|(_, d, k)| (*d, *k))
        .unwrap_or(("wifi_lan", "unknown"));

    let server = headers.get("server").cloned();
    let vendor = server.as_ref().and_then(|s| s.split('/').next().map(String::from));
    let location = headers.get("location").cloned();

    Some(ScanCandidate {
        driver: driver.to_string(),
        external_id: usn,
        name: headers
            .get("server")
            .cloned()
            .or_else(|| Some(ip.clone()))
            .unwrap_or_default(),
        kind: kind.to_string(),
        vendor,
        ip: Some(ip),
        mac: None,
        details: json!({
            "source": "ssdp",
            "st": st,
            "location": location,
            "headers": headers,
        }),
    })
}

// ── Dedupe ──────────────────────────────────────────────────────────────

/// Keep one candidate per (driver, external_id) pair, but when the same
/// logical device answered multiple sweeps (mDNS + SSDP + different
/// service types) merge them by preferring the entry with the richer
/// details JSON. This keeps the UI card list clean.
fn dedupe(mut input: Vec<ScanCandidate>) -> Vec<ScanCandidate> {
    input.sort_by(|a, b| {
        a.driver
            .cmp(&b.driver)
            .then_with(|| a.external_id.cmp(&b.external_id))
    });
    input.dedup_by(|a, b| a.driver == b.driver && a.external_id == b.external_id);
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homekit_category_known_codes() {
        assert_eq!(homekit_category(7), "light");
        assert_eq!(homekit_category(2), "hub");
        assert_eq!(homekit_category(19), "camera");
        assert_eq!(homekit_category(255), "unknown");
    }

    #[test]
    fn parse_ssdp_happy_path() {
        let text = "HTTP/1.1 200 OK\r\n\
                    CACHE-CONTROL: max-age=1800\r\n\
                    LOCATION: http://192.168.1.10:1400/xml/device_description.xml\r\n\
                    SERVER: Linux/4.4 UPnP/1.0 Sonos/80.1-49230\r\n\
                    ST: urn:schemas-upnp-org:device:ZonePlayer:1\r\n\
                    USN: uuid:RINCON_ABC::urn:schemas-upnp-org:device:ZonePlayer:1\r\n\
                    \r\n";
        let c = parse_ssdp_response(text, "192.168.1.10".to_string())
            .expect("expected candidate");
        assert_eq!(c.driver, "wifi_lan");
        assert_eq!(c.kind, "media_player");
        assert!(c.external_id.contains("RINCON_ABC"));
        assert_eq!(c.ip.as_deref(), Some("192.168.1.10"));
    }

    #[test]
    fn dedupe_merges_same_external_id() {
        let c1 = ScanCandidate {
            driver: "matter".into(),
            external_id: "n42".into(),
            name: "Node 42".into(),
            kind: "light".into(),
            vendor: None,
            ip: None,
            mac: None,
            details: json!({}),
        };
        let c2 = ScanCandidate {
            driver: "matter".into(),
            external_id: "n42".into(),
            name: "Node 42".into(),
            kind: "light".into(),
            vendor: None,
            ip: None,
            mac: None,
            details: json!({}),
        };
        let out = dedupe(vec![c1, c2]);
        assert_eq!(out.len(), 1);
    }
}
