//! Non-Matter LAN-only adopter framework.
//!
//! Each `LanAdopter` is a per-vendor module that knows how to discover,
//! onboard, and control devices that speak a vendor-specific LAN
//! protocol (Govee, WiZ, Meross, etc.). Mirrors the design of
//! [`super::mqtt::dialects::Dialect`] but runs against UDP/HTTP instead
//! of an MQTT broker.
//!
//! ## When to add a `LanAdopter` vs. another driver
//!
//! - **Matter** — device is Matter-commissioned. Use the matter driver.
//! - **MQTT-published** (Tasmota, Z2M, ESPHome, Frigate) — use the
//!   MQTT subsystem. The MQTT broker is the bus.
//! - **Vendor-specific LAN protocol** — drop a new adopter here. The
//!   adopter owns the wire format end-to-end.
//!
//! ## Recipe families covered
//!
//! Per the [matter migration + LAN adoption plan][plan], adopters fall
//! into five recipes:
//!
//! - **R1** — cloud-bootstrapped LAN (Tuya, Tapo, AiDot, Govee w/account,
//!   Meross). One-time cloud login harvests a per-device key, all
//!   subsequent traffic is LAN-only.
//! - **R2** — fully open LAN (LIFX, WiZ, Yeelight, Magic Home, Govee
//!   "LAN mode toggle"). No auth or static factory key.
//! - **R3** — custom-firmware reflash (Tasmota, ESPHome). Handled by the
//!   MQTT subsystem post-flash.
//! - **R4** — MQTT broker as adapter. Handled by the MQTT subsystem.
//! - **R5** — cloud-only, no LAN at all. Refused.
//!
//! [plan]: ../../../../../../vault/research/matter_migration_and_lan_adoption.md

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::smart_home::scan::ScanCandidate;

pub mod dyson;
pub mod govee;
pub mod wiz;

/// One device the adopter has located on the LAN. Adopters fill what
/// they can — the rest stays `None` until the user confirms onboarding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanCandidate {
    /// Adopter slug ("govee", "wiz", ...). Becomes the
    /// `smart_home_devices.driver` value.
    pub adopter: String,
    /// Adopter-specific stable identifier (MAC, serial, mesh address).
    pub external_id: String,
    pub name: String,
    pub kind: String,
    pub vendor: String,
    pub ip: String,
    pub mac: Option<String>,
    pub model: Option<String>,
    /// Adopter-specific freeform detail surfaced in the confirmation
    /// card (capability hints, firmware version, color modes, etc.).
    pub details: serde_json::Value,
}

impl LanCandidate {
    /// Convert into the unified `ScanCandidate` shape the rest of the
    /// scan pipeline already speaks.
    pub fn into_scan_candidate(self) -> ScanCandidate {
        ScanCandidate {
            driver: format!("lan_{}", self.adopter),
            external_id: self.external_id,
            name: self.name,
            kind: self.kind,
            vendor: Some(self.vendor),
            ip: Some(self.ip),
            mac: self.mac,
            details: self.details,
        }
    }
}

/// Result of a write/control attempt against a LAN device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanCommandOutcome {
    pub adopter: String,
    pub external_id: String,
    pub ok: bool,
    pub message: Option<String>,
}

/// One-shot control intent. Rich types stay in the adopter — at this
/// layer everything is a structured patch the adopter knows how to
/// translate to its native protocol.
#[derive(Debug, Clone)]
pub enum LanCommand {
    SetOn(bool),
    /// Brightness 0..=100.
    SetBrightness(u8),
    /// Color temperature in Kelvin (e.g. 2700–6500).
    SetColorTempK(u16),
    /// 8-bit per channel.
    SetColorRgb { r: u8, g: u8, b: u8 },
    /// Vendor-specific raw command — adopter interprets the JSON.
    Raw(serde_json::Value),
}

/// Lifecycle surface a vendor LAN adopter implements.
///
/// Adopters are stateless functor structs (held by `Arc`) so they're
/// cheap to register and call concurrently. Long-lived state (sockets,
/// session keys) lives behind locks the adopter owns.
#[async_trait]
pub trait LanAdopter: Send + Sync + 'static {
    /// Stable slug — becomes part of the driver column value
    /// (`lan_<slug>`) and the candidate's `external_id` namespace.
    fn slug(&self) -> &'static str;

    /// Recipe family per the LAN adoption plan. Drives onboarding
    /// UX (R1 needs a cloud-bootstrap step; R2 is one-tap).
    fn recipe(&self) -> Recipe;

    /// Probe the LAN and return candidates this adopter recognizes.
    /// Must complete within `scan::DEFAULT_SCAN_SECONDS` (~5s). Errors
    /// are swallowed and logged so one adopter never poisons the
    /// aggregate scan report.
    async fn discover(&self) -> Vec<LanCandidate>;

    /// Send a command to a previously-onboarded device. The default
    /// implementation returns `not implemented` — adopters override as
    /// they grow control surface.
    async fn dispatch(
        &self,
        external_id: &str,
        cmd: &LanCommand,
    ) -> LanCommandOutcome {
        let _ = (external_id, cmd);
        LanCommandOutcome {
            adopter: self.slug().to_string(),
            external_id: external_id.to_string(),
            ok: false,
            message: Some("dispatch not implemented for adopter".to_string()),
        }
    }
}

/// Recipe families per the plan. Surfaced in the UI so the user knows
/// whether onboarding will require a cloud login (R1) or is one-tap (R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recipe {
    /// Cloud-bootstrapped LAN. Needs a vendor cloud account once.
    R1CloudBootstrap,
    /// Open LAN — no auth or shared factory key.
    R2OpenLan,
}

/// All adopters registered for the running gateway, in no particular
/// order. New adopters land here.
pub fn registry() -> Vec<Arc<dyn LanAdopter>> {
    vec![
        Arc::new(dyson::DysonAdopter::default()),
        Arc::new(govee::GoveeAdopter::default()),
        Arc::new(wiz::WizAdopter::default()),
    ]
}

/// Run every registered adopter's discovery in parallel and merge the
/// results into one list. Per-adopter failures are logged and dropped —
/// scan reports never get poisoned by one adopter going sideways.
pub async fn discover_all() -> Vec<LanCandidate> {
    let adopters = registry();
    let mut tasks = Vec::with_capacity(adopters.len());
    for a in adopters {
        let slug = a.slug();
        tasks.push(tokio::spawn(async move {
            let started = std::time::Instant::now();
            let result = a.discover().await;
            log::info!(
                "[lan_adopter::{}] discovered {} candidates in {} ms",
                slug,
                result.len(),
                started.elapsed().as_millis()
            );
            result
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        match t.await {
            Ok(v) => all.extend(v),
            Err(e) => log::warn!("[lan_adopter] task join failed: {e}"),
        }
    }
    all
}

/// Find the adopter whose `slug()` matches and dispatch a command. Used
/// by the smart-home control path when `device.driver == "lan_<slug>"`.
pub async fn dispatch_command(
    adopter_slug: &str,
    external_id: &str,
    cmd: &LanCommand,
) -> LanCommandOutcome {
    for a in registry() {
        if a.slug() == adopter_slug {
            return a.dispatch(external_id, cmd).await;
        }
    }
    LanCommandOutcome {
        adopter: adopter_slug.to_string(),
        external_id: external_id.to_string(),
        ok: false,
        message: Some(format!("no adopter registered for slug '{}'", adopter_slug)),
    }
}
