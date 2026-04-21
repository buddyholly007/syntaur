//! Per-protocol driver registry. Each submodule owns one
//! protocol's device lifecycle + state cache + event feed into the
//! module-wide broadcast channel.
//!
//! Stubs-only at Track A week 1 — real drivers land per the plan
//! calendar: wifi_lan (week 2), matter (weeks 3–4), zigbee (weeks 5–6),
//! ble (week 7), mqtt (week 8), camera (week 9), zwave (week 13),
//! cloud adapters (week 11).

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;

use crate::smart_home::scan::ScanCandidate;

pub mod ble;
pub mod camera;
pub mod cloud;
pub mod matter;
pub mod mqtt;
pub mod wifi_lan;
pub mod zigbee;
pub mod zwave;

/// Uniform lifecycle surface implemented by any driver that wants to
/// participate in the module's fan-out scan + supervised runtime.
///
/// Scan-only drivers (`camera`, `wifi_lan` — one-shot LAN sweep) only
/// need `scan()`. Supervised drivers (MQTT long-running subscription,
/// Matter v1.1 Controller event loop) also implement `start()` to hand
/// back a `DriverHandle` whose `shutdown` is signalled during
/// `smart_home::shutdown`.
///
/// Adding this trait is additive — existing drivers keep their free-fn
/// `scan()` until they opt in. The `scan.rs` `tokio::join!` fan-out
/// continues to work unchanged.
#[async_trait]
pub trait SmartHomeDriver: Send + Sync {
    /// Stable identifier, matches `smart_home_devices.driver` column
    /// values ("mqtt", "matter", "wifi_lan", "zwave", "ble", "camera",
    /// "zigbee", or a per-vendor cloud label).
    fn name(&self) -> &'static str;

    /// One-shot discovery. Called during `smart_home::scan::run()`.
    /// Must not block for longer than the scan window (see
    /// `scan::DEFAULT_SCAN_SECONDS` — currently 5s). Returns the set of
    /// devices visible at the moment of the call.
    async fn scan(&self) -> Vec<ScanCandidate>;

    /// Start the supervised runtime (long-running subscription, event
    /// loop, etc.) and return a handle whose `shutdown` channel is used
    /// by `smart_home::shutdown` for orderly teardown. Drivers without
    /// a runtime component can return immediately.
    async fn start(self: Arc<Self>) -> Result<DriverHandle, String>;
}

/// Opaque handle returned by `SmartHomeDriver::start`. The holder
/// keeps the driver alive; dropping it without sending on `shutdown`
/// leaves the spawned task running until process exit, which is
/// acceptable — `tokio::sync::oneshot` is "signal, not required".
pub struct DriverHandle {
    pub shutdown: oneshot::Sender<()>,
}
