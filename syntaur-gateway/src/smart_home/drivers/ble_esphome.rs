//! ESPHome native-API BLE-advertisement ingest (third RSSI source for
//! the BLE driver, alongside MQTT and the local btleplug host scanner).
//!
//! ## Status: feature-gated SCAFFOLD, not wired
//!
//! The dependency (`esphome-native-api = "2.0.7"`) is in Cargo.toml
//! behind the same `ble-host` feature flag as btleplug. The connection
//! lifecycle + protobuf wiring (auth handshake, `subscribe_bluetooth_le_
//! advertisements`, pull MAC+RSSI off `BluetoothLERawAdvertisementsResponse`,
//! reconnect on drop) is a multi-day implementation that doesn't fit
//! the "polish" scope of the session that introduced this file.
//!
//! ## Why this is deferred (not abandoned)
//!
//! The MQTT path already ingests RSSI from the same ESPHome proxies
//! through the bluetooth_proxy → MQTT bridge HA installs by default.
//! Native-API ingest is preferable in two cases:
//!   1. Deployments without HA / Mosquitto (pure-Syntaur installs).
//!   2. Latency-sensitive tracking — native API delivers raw frames
//!      ~100–500 ms ahead of the MQTT bridge.
//! For the v1 launch profile (HA-broker present, person-level presence
//! granularity), MQTT is sufficient.
//!
//! ## What's left to wire
//!
//! 1. `start_esphome_ingest(driver)` should read each user's
//!    `smart_home_devices` rows of kind=`esphome_proxy` (a new device
//!    kind), open one connection per proxy, subscribe to BLE adverts,
//!    map each frame's MAC+RSSI into `RssiObservation` attributed to
//!    the proxy's anchor row (via `state_json.ble_anchor`), and call
//!    `driver.push_observation`.
//! 2. Reconnect with exponential backoff. The ESPHome native API drops
//!    on Wi-Fi blips; v1 should tolerate those without restarting the
//!    gateway.
//! 3. Per-proxy auth via `smart_home_credentials` (the same encrypted
//!    store MQTT uses), so passwords never live in env vars.
//! 4. Lifecycle hookup in `smart_home::init` next to the host scanner.
//!
//! Until those four pieces land, `start_esphome_ingest` is a no-op and
//! a compile-time-feature-gated check; production behavior is
//! unchanged.

#![cfg_attr(not(feature = "ble-host"), allow(dead_code))]

use std::sync::Arc;

use super::ble::BleDriver;

/// Spawn the ESPHome native-API ingest task. Returns immediately with
/// a JoinHandle that completes the moment the task is started — the
/// task itself is a no-op until the implementation in the doc-block
/// "What's left to wire" lands.
pub fn start_esphome_ingest(_driver: Arc<BleDriver>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        log::info!(
            "[smart_home::ble_esphome] ESPHome native-API ingest scaffolded; \
             relying on MQTT path until per-proxy connection lifecycle is wired"
        );
    })
}
