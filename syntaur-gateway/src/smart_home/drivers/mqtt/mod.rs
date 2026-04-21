//! MQTT driver — pure-Rust client for smart-home device discovery and
//! (from Phase D) control, via a user-supplied broker or Syntaur's own
//! in-process `rumqttd` (Phase E, embedded-by-default per plan §8 Q1).
//!
//! Runtime path (Phase C): the [`MqttSupervisor`] hooks off
//! `smart_home::init`, reads every `smart_home_credentials` row with
//! `provider='mqtt'`, and spawns one long-running [`MqttSession`] per
//! row. Dialect parses produce [`dialects::DialectMessage`] variants
//! that land in the [`StateCache`] hash-diff layer and fire
//! `SmartHomeEvent::DeviceStateChanged` on the module-wide bus.
//!
//! Scan path: when the supervisor is running, `scan()` returns the
//! in-memory discovery snapshot the long-running session has already
//! populated — no second broker connection. If no supervisor is set
//! (tests, minimal deployments), `scan()` falls back to the legacy
//! one-shot `SMART_HOME_MQTT_URL` env-var path.
//!
//! Supported dialects (see `dialects/` for each):
//!   - `tasmota`      — discovery + STATE/SENSOR/POWER/LWT runtime
//!   - `shelly_gen1`  — `shellies/<id>/announce`
//!   - `shelly_gen2`  — `shellyplus<id>/online` + RPC (Phase D commands)
//!   - `esphome`      — `esphome/discover/<host>`
//!   - `zigbee2mqtt`  — `bridge/devices` inventory (per-device runtime in Phase B follow-on)
//!   - `openmqttgateway` — BLE/RF/IR bridge output
//!   - `ha_discovery` — Home Assistant MQTT discovery (fallback)
//!
//! Configuration: `smart_home_credentials` (provider='mqtt') is the
//! canonical source. `SMART_HOME_MQTT_URL` is honored as a deprecated
//! fallback and logged at warn level.

use std::sync::OnceLock;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};

use crate::smart_home::scan::ScanCandidate;

pub mod broker;
pub mod client;
pub mod command;
pub mod dialects;
pub mod publisher;
pub mod state;
pub mod stats;
pub mod supervisor;

use dialects::{DialectMessage, DialectRouter};
pub use supervisor::MqttSupervisor;

const DEFAULT_SCAN_SECONDS: u64 = 5;
/// Widened from rumqttc's 32 default so retained-message catch-up on
/// reconnect doesn't back up the event loop. See
/// [`client::EVENT_LOOP_CAPACITY`] — same rationale for the one-shot
/// scan path, where bridge/devices arrays + HA Discovery retention can
/// exceed 32 in a single cycle.
const SCAN_EVENT_LOOP_CAPACITY: usize = 1024;

/// Global supervisor handle, set by `smart_home::init` → `MqttSupervisor::spawn`.
/// `scan()` prefers this when present.
static SUPERVISOR: OnceLock<std::sync::Arc<MqttSupervisor>> = OnceLock::new();

/// Install the supervisor handle. Called once from `smart_home::init`.
/// Subsequent calls are no-ops.
pub fn install_supervisor(sup: std::sync::Arc<MqttSupervisor>) {
    let _ = SUPERVISOR.set(sup);
}

/// Dispatch a state-patch control call through the installed MQTT
/// supervisor. `Err(..)` when no supervisor is installed; the
/// automation engine + `/api/smart-home/control` callers interpret
/// that as "mqtt not wired yet." Returns the number of command
/// publishes enqueued on success.
pub async fn dispatch_command(
    user_id: i64,
    device_id: i64,
    state: &serde_json::Value,
) -> Result<usize, String> {
    let Some(sup) = SUPERVISOR.get() else {
        return Err("mqtt supervisor not installed".into());
    };
    sup.dispatch_command(user_id, device_id, state).await
}

/// Top-level scan entry. If a `MqttSupervisor` is running, returns its
/// in-memory discovery snapshot (no second connection). Otherwise falls
/// back to a legacy one-shot scan against `SMART_HOME_MQTT_URL`.
pub async fn scan() -> Vec<ScanCandidate> {
    if let Some(sup) = SUPERVISOR.get() {
        let snap = sup.scan_snapshot().await;
        if !snap.is_empty() {
            return snap;
        }
        // Empty snapshot usually means the subscriber hasn't received
        // retained discovery yet (cold boot). Fall through to the
        // one-shot scan — it's a fixed 5s stall but gives the user
        // something when they press "Scan" before the subscriber has
        // warmed up.
    }
    let Some(opts) = mqtt_options_from_env() else {
        log::debug!("[smart_home::mqtt] SMART_HOME_MQTT_URL not set, skipping scan");
        return Vec::new();
    };
    scan_with_options(opts, Duration::from_secs(DEFAULT_SCAN_SECONDS)).await
}

/// Public so integration tests and a future UI "test connection" path
/// can exercise a specific broker without going through env.
pub async fn scan_with_options(opts: MqttOptions, window: Duration) -> Vec<ScanCandidate> {
    let router = DialectRouter::v1();
    let (client, mut event_loop) = AsyncClient::new(opts, SCAN_EVENT_LOOP_CAPACITY);

    for topic in router.subscribe_topics() {
        if let Err(e) = client.subscribe(topic, QoS::AtMostOnce).await {
            log::warn!("[smart_home::mqtt] subscribe {} failed: {}", topic, e);
        }
    }

    let mut candidates: Vec<ScanCandidate> = Vec::new();
    let deadline = tokio::time::Instant::now() + window;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let next = tokio::time::timeout(remaining, event_loop.poll()).await;
        let event = match next {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                log::info!("[smart_home::mqtt] event loop error, ending scan: {}", e);
                break;
            }
            Err(_) => break, // scan window elapsed
        };
        if let Event::Incoming(Incoming::Publish(publish)) = event {
            if let Some(msg) = router.parse(&publish.topic, &publish.payload) {
                match msg {
                    DialectMessage::Discovery(c) => candidates.push(c),
                    DialectMessage::Discoveries(list) => candidates.extend(list),
                    // Phase C variants (State / Availability / BridgeEvent)
                    // will add arms here.
                    _ => {}
                }
            }
        }
    }

    let _ = client.disconnect().await;
    dedupe(candidates)
}

fn mqtt_options_from_env() -> Option<MqttOptions> {
    let url = std::env::var("SMART_HOME_MQTT_URL").ok()?;
    let parsed = url::Url::parse(&url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port().unwrap_or(1883);
    let mut opts = MqttOptions::new("syntaur-smart-home-scan", host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !parsed.username().is_empty() {
        opts.set_credentials(
            parsed.username().to_string(),
            parsed.password().unwrap_or("").to_string(),
        );
    }
    Some(opts)
}

fn dedupe(mut v: Vec<ScanCandidate>) -> Vec<ScanCandidate> {
    v.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    v.dedup_by(|a, b| a.external_id == b.external_id);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_collapses_duplicates_by_external_id() {
        let c = |id: &str| ScanCandidate {
            driver: "mqtt".into(),
            external_id: id.into(),
            name: id.into(),
            kind: "unknown".into(),
            vendor: None,
            ip: None,
            mac: None,
            details: serde_json::Value::Null,
        };
        let out = dedupe(vec![c("a"), c("b"), c("a")]);
        assert_eq!(out.len(), 2);
    }

    // Per-dialect parse tests live with their dialects under dialects/.
    // Router-level tests (dialect coverage, unknown-topic rejection)
    // live in dialects/mod.rs.
}
