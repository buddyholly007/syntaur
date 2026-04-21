//! Unified discovery pipeline — the single "Scan for new devices" button
//! fans out to every driver's discovery surface and aggregates candidates.
//!
//! Week 2 brings the Wi-Fi / LAN path online (mDNS + SSDP via
//! `drivers::wifi_lan`). Matter commissioning (week 3), Zigbee
//! permit-join (week 5), BLE (week 7), MQTT auto-discovery (week 8),
//! and Z-Wave AddNode via `syntaur-zwave` (week 13) plug in below as
//! their drivers come online. Each driver fails independently — one
//! coordinator error doesn't stop the others from returning.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::drivers;

/// A candidate device surfaced by one of the scanners. `driver` names
/// which driver will own the device if the user confirms the card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCandidate {
    pub driver: String,
    pub external_id: String,
    pub name: String,
    pub kind: String,
    pub vendor: Option<String>,
    pub ip: Option<String>,
    pub mac: Option<String>,
    /// Freeform driver-specific detail for the confirmation card.
    pub details: serde_json::Value,
}

/// Aggregated scan result. Drivers fail independently.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub candidates: Vec<ScanCandidate>,
    pub errors: Vec<ScanError>,
    /// How long the scan ran, milliseconds.
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanError {
    pub source: String,
    pub message: String,
}

/// Run a single coordinated scan across all enabled drivers.
pub async fn run(_user_id: i64) -> ScanReport {
    let start = Instant::now();
    let mut report = ScanReport::default();

    // Run every live driver in parallel; each surfaces its own errors
    // on its own source so a flaky coordinator never poisons the
    // overall report.
    let (wifi, matter, mqtt, camera) = tokio::join!(
        drivers::wifi_lan::sweep(),
        drivers::matter::scan(),
        drivers::mqtt::scan(),
        drivers::camera::scan(),
    );
    log::info!(
        "[smart_home::scan] wifi_lan={} matter={} mqtt={} camera={} candidates",
        wifi.len(),
        matter.len(),
        mqtt.len(),
        camera.len()
    );
    report.candidates.extend(wifi);
    report.candidates.extend(matter);
    report.candidates.extend(mqtt);
    report.candidates.extend(camera);

    // Zigbee deferred to v1.x (task #17). BLE driver (week 7 stub) +
    // Z-Wave inclusion (week 13) plug in here as their tracks land.

    report.duration_ms = start.elapsed().as_millis() as u64;
    log::info!(
        "[smart_home::scan] total {} candidates in {} ms",
        report.candidates.len(),
        report.duration_ms
    );
    report
}
