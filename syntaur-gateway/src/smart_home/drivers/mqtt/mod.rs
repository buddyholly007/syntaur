//! MQTT driver — pure-Rust client for smart-home device discovery and
//! (from Phase D) control, via a user-supplied broker or Syntaur's own
//! in-process `rumqttd` (Phase E, embedded-by-default per plan §8 Q1).
//!
//! v1 scan behavior: connect, subscribe to the topic set every
//! registered dialect asks for, parse retained frames through
//! [`dialects::DialectRouter`] into [`ScanCandidate`]s for a short
//! window, disconnect. If the broker is unreachable or no config is
//! present, return empty (graceful degrade — Matter / Wi-Fi / etc.
//! scans must not be poisoned by a flaky broker).
//!
//! Supported dialects (see `dialects/` for each):
//!   - `ha`           — Home Assistant MQTT discovery
//!   - `tasmota`      — Tasmota discovery (extends to STATE/SENSOR in Phase B4)
//!   - `shelly_gen1`  — `shellies/<id>/announce`
//!   - `esphome`      — `esphome/discover/<host>`
//!
//! Phase B additions (coming):
//!   - `shelly_gen2`       — RPC-over-MQTT for Plus/Pro/Gen3 devices
//!   - `zigbee2mqtt`       — bridge/devices inventory + per-device state
//!   - `openmqttgateway`   — BLE/RF/IR bridge output
//!
//! Configuration in v1: `SMART_HOME_MQTT_URL=mqtt://[user:pass@]host[:port]`.
//! Phase A's `smart_home_credentials` table supersedes this —
//! the supervisor (Phase C) reads encrypted rows from there and the
//! env var stays as a last-resort dev-mode fallback.

use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};

use crate::smart_home::scan::ScanCandidate;

pub mod dialects;

use dialects::{DialectMessage, DialectRouter};

const DEFAULT_SCAN_SECONDS: u64 = 5;

/// Top-level scan entry. Returns empty if no broker is configured or
/// the connection fails.
pub async fn scan() -> Vec<ScanCandidate> {
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
    let (client, mut event_loop) = AsyncClient::new(opts, 32);

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
