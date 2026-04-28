// Track A week 1 scaffolding: several helpers + types are wired for
// future driver bring-up (weeks 2-13) and don't have call sites yet.
// Relax the dead_code lint at the module root so the scaffold doesn't
// spew warnings; each item gets its user once its track lands, and this
// attribute can come off before v1 ships.
#![allow(dead_code)]

//! Smart Home and Network module.
//!
//! Pure-Rust home-automation stack: Matter (rs-matter), Zigbee, Z-Wave
//! (via the workspace `syntaur-zwave` crate), Wi-Fi/LAN-native drivers,
//! BLE, MQTT, RTSP/ONVIF cameras, opt-in proprietary cloud adapters.
//! No sidecars — see `vault/feedback/pure_rust_no_sidecars.md`.
//!
//! Scaffolding only at Track A Week 1: schema migrations (v57–v63) are
//! in place, module tree is wired, handlers return 501/empty so the UI
//! can exercise the route surface while individual drivers land in
//! subsequent weeks.
//!
//! Layout (matches `plans/we-need-to-work-floofy-haven.md`):
//!
//! ```text
//! smart_home/
//! ├── mod.rs         ← this file: init() + type re-exports
//! ├── api.rs         ← /api/smart-home/* HTTP handlers
//! ├── devices.rs     ← Device model + CRUD + state cache
//! ├── rooms.rs       ← Room model + CRUD
//! ├── scan.rs        ← unified discovery pipeline
//! ├── automation.rs  ← automation engine (triggers/conditions/actions)
//! ├── nl_automation.rs ← LLM natural-language → AST compiler
//! ├── energy.rs      ← energy accounting roll-ups
//! ├── diagnostics.rs ← network-health worker
//! ├── presence.rs    ← BLE + phone + voice fusion
//! └── drivers/       ← per-protocol driver crate exports
//! ```

pub mod api;
pub mod automation;
pub mod credentials;
pub mod devices;
pub mod diagnostics;
pub mod drivers;
pub mod energy;
pub mod esphome_discovery;
pub mod events;
pub mod firmware_flash;
pub mod firmware_role;
pub mod nl_automation;
pub mod presence;
pub mod rooms;
pub mod scan;

// Path C + vendor LAN + Nexia — additive HTTP route modules. Wired
// as siblings to api.rs; each owns its /api/smart-home/<bucket>/*
// subtree. See vault/projects/path_c_plan.md for Matter;
// vault/projects/rust_aidot.md + rust_kasa.md for vendor LAN;
// vault/projects/trane_nexia_thermostat.md for Nexia.
pub mod matter_bridge;
pub mod nexia_bridge;
pub mod vendor_bridge;

/// Module-wide init hook. Called from `main.rs` once the `AppState`
/// and database pool are ready. Launches the automation engine
/// supervisor as a detached background task so enabled time-triggered
/// automations fire every minute without further wiring from `main.rs`.
///
/// Future drivers with their own background tasks (MQTT event-bus
/// subscriber, energy roll-up scheduler, diagnostics sweeper) hang off
/// this call so `main.rs` stays stable.
pub async fn init(db_path: std::path::PathBuf) -> Result<(), String> {
    // Touch the event bus so the OnceLock fires at a known time rather
    // than during first publish (important for tests that want to
    // subscribe before engines start).
    let _ = events::bus();

    let auto_engine =
        std::sync::Arc::new(automation::AutomationEngine::new(db_path.clone()));
    let _auto_handle = auto_engine.spawn();

    let diag_engine =
        std::sync::Arc::new(diagnostics::DiagnosticsEngine::new(db_path.clone()));
    let _diag_handle = diag_engine.spawn();

    let energy_engine = std::sync::Arc::new(energy::EnergyEngine::new(db_path.clone()));
    let _energy_handle = energy_engine.spawn();
    let ble_db_path = db_path.clone();

    // Phase E MQTT embedded broker — `rumqttd` on 127.0.0.1:1884 by
    // default. Honors `SMART_HOME_EMBEDDED_BROKER=off` to disable or
    // `=<host:port>` to rebind. Bind conflicts (Mosquitto already on
    // the port, permission denied, etc.) soft-fail into the supervisor
    // running bridge-only against an upstream broker.
    let _embedded_broker = drivers::mqtt::broker::EmbeddedBroker::from_env_or_default();

    // Phase C MQTT supervisor — one long-running rumqttc session per
    // `smart_home_credentials` row (provider='mqtt'), plus a legacy
    // `SMART_HOME_MQTT_URL` fallback. Never fails `init`: bad/missing
    // credentials log a warning and no sessions spawn.
    let mqtt_supervisor = drivers::mqtt::MqttSupervisor::spawn(db_path.clone()).await;
    drivers::mqtt::install_supervisor(mqtt_supervisor.clone());

    // Phase F-1 Home Assistant MQTT-Discovery publisher — retained
    // config frames so HA/openHAB auto-surface every Syntaur device,
    // plus state republish on every DeviceStateChanged. Detached task
    // with a ~2s warm-up so the supervisor's sessions have a chance
    // to connect before the first config publish lands.
    let _ha_discovery = std::sync::Arc::new(
        drivers::mqtt::publisher::HADiscoveryPublisher::new(mqtt_supervisor, db_path.clone()),
    )
    .spawn();

    // BLE driver — multi-tenant. Subscribes to the event bus, filters
    // MQTT events coming from any user's configured anchor devices,
    // and writes `smart_home_presence_signals` rows on a 15s tick
    // attributed per-tenant. Anchors live in
    // `smart_home_devices.state_json->ble_anchor`; a single driver
    // instance services every tenant by composite key
    // `(user_id, anchor_device_id)`.
    let ble = std::sync::Arc::new(drivers::ble::BleDriver::new(db_path));
    // Hydrate the anchor set from smart_home_devices.state_json->ble_anchor
    // before the ingest loop starts subscribing. If the DB has no
    // anchor rows (fresh install), this is a no-op — users seed anchors
    // via `PUT /api/smart-home/ble/anchors`, which also writes them
    // back to the DB so subsequent restarts recover the config.
    if let Err(e) = ble.hydrate_from_db().await {
        log::warn!("[smart_home] ble anchor hydrate failed (non-fatal): {e}");
    }
    let _ble_handle = ble.clone().spawn();
    // Local btleplug host scanner — third RSSI source. No-op at runtime
    // when the `ble-host` feature is off OR when the host has no BT
    // adapter, so this call is safe on every deployment target.
    let _host_handle = drivers::ble_host::start_host_scanner(ble.clone());
    // ESPHome native-API ingest — Phase 4. One TCP connection per
    // smart_home_devices row of kind=esphome_proxy whose
    // state_json.esphome.mode is "tracking". On builds without the
    // ble-host feature the supervisor is a logging no-op so the wiring
    // stays in place across deployment targets.
    let _esphome_handle =
        drivers::ble_esphome::start_esphome_ingest(ble.clone(), ble_db_path);
    drivers::ble::install(ble);

    log::info!(
        "[smart_home] module initialized — event bus + automation + diagnostics + energy + mqtt + ble engines spawned"
    );
    Ok(())
}
