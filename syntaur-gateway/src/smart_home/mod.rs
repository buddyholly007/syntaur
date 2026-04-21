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
pub mod devices;
pub mod diagnostics;
pub mod drivers;
pub mod energy;
pub mod events;
pub mod nl_automation;
pub mod presence;
pub mod rooms;
pub mod scan;

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

    let energy_engine = std::sync::Arc::new(energy::EnergyEngine::new(db_path));
    let _energy_handle = energy_engine.spawn();

    log::info!(
        "[smart_home] module initialized — event bus + automation + diagnostics + energy engines spawned"
    );
    Ok(())
}
