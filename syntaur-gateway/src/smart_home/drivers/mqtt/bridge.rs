//! Upstream MQTT bridge — one-direction mirror from the gateway's
//! primary broker (usually the embedded `rumqttd` on :1884) into the
//! user's existing broker (Mosquitto / HA-OS / etc.).
//!
//! The goal: a household running HA at `mosquitto.lan` sees every
//! Syntaur-native device without configuring HA against our embedded
//! broker directly. HA Discovery configs + per-device state publishes
//! land on their existing broker verbatim.
//!
//! Phase F-2 Piece A ships one-direction mirroring only. Control from
//! the upstream side back to the device (HA → Mosquitto → Syntaur)
//! still works either via HA connecting directly to :1884 (the
//! embedded broker) or through the user's own Mosquitto bridge config
//! pointing at :1884. Bidirectional relay with loop prevention is a
//! Phase F-3 follow-on.
//!
//! Topic filter: we mirror exactly the topics Syntaur writes:
//!   - `homeassistant/+/+/config` and `homeassistant/+/+/+/config`
//!     (HA Discovery configs emitted by `HADiscoveryPublisher`)
//!   - `syntaur/u/+/device/+/state` (per-device state republish)
//! Every other topic on the downstream broker is ignored, so the
//! bridge doesn't leak internal Syntaur traffic the user didn't ask
//! for.
//!
//! Triggered by the credential's secret blob carrying
//!   `"bridge_to": "mqtt://[user:pass@]host[:port]"`
//! Absent, the session runs without a bridge.

use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use tokio::sync::oneshot;

/// Topics Syntaur publishes that the bridge mirrors upstream. Keeping
/// this narrow stops the bridge from flooding the user's upstream
/// with `zigbee2mqtt/bridge/devices` retained inventories etc. that
/// are already available on their source of truth.
const MIRROR_FILTERS: &[&str] = &[
    "homeassistant/+/+/config",
    "homeassistant/+/+/+/config",
    "syntaur/u/+/device/+/state",
];

/// Configuration for one bridge.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub label: String,
    /// Primary-broker URL (the one the session + HADiscoveryPublisher
    /// are already talking to). Usually `mqtt://127.0.0.1:1884`.
    pub downstream_url: String,
    /// User's upstream broker URL.
    pub upstream_url: String,
}

pub struct Bridge {
    cfg: BridgeConfig,
    shutdown_rx: Option<oneshot::Receiver<()>>,
}

impl Bridge {
    pub fn new(cfg: BridgeConfig, shutdown_rx: oneshot::Receiver<()>) -> Self {
        Self {
            cfg,
            shutdown_rx: Some(shutdown_rx),
        }
    }

    /// Run forever (or until shutdown fires). Each direction runs its
    /// own reconnect loop; if one side drops, the other keeps going.
    pub async fn run(mut self) {
        let mut shutdown = self
            .shutdown_rx
            .take()
            .expect("Bridge::run called twice");

        tokio::select! {
            _ = &mut shutdown => {
                log::info!(
                    "[smart_home::mqtt::bridge] {} shutting down",
                    self.cfg.label
                );
            }
            _ = self.mirror_loop() => {
                log::info!(
                    "[smart_home::mqtt::bridge] {} mirror loop exited",
                    self.cfg.label
                );
            }
        }
    }

    async fn mirror_loop(&self) {
        loop {
            if let Err(e) = self.one_cycle().await {
                log::warn!(
                    "[smart_home::mqtt::bridge] {} cycle error: {} — reconnecting in 5s",
                    self.cfg.label,
                    e
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn one_cycle(&self) -> Result<(), String> {
        let down = build_client(&self.cfg.downstream_url, "syntaur-bridge-sub")?;
        let (down_client, mut down_loop) = down;
        let up = build_client(&self.cfg.upstream_url, "syntaur-bridge-pub")?;
        let (up_client, mut up_loop) = up;

        for filter in MIRROR_FILTERS {
            if let Err(e) = down_client.subscribe(*filter, QoS::AtLeastOnce).await {
                log::warn!(
                    "[smart_home::mqtt::bridge] {} subscribe {} failed: {}",
                    self.cfg.label,
                    filter,
                    e
                );
            }
        }
        log::info!(
            "[smart_home::mqtt::bridge] {} mirroring {} → {}",
            self.cfg.label,
            redact(&self.cfg.downstream_url),
            redact(&self.cfg.upstream_url)
        );

        // Service the upstream event loop passively — we don't
        // subscribe upstream (one-way relay), but rumqttc still needs
        // its event loop polled to flush publishes.
        let up_poll = tokio::spawn(async move {
            loop {
                match up_loop.poll().await {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!(
                            "[smart_home::mqtt::bridge] upstream event loop error: {}",
                            e
                        );
                        break;
                    }
                }
            }
        });

        let outcome = async move {
            loop {
                let ev = down_loop
                    .poll()
                    .await
                    .map_err(|e| format!("downstream poll: {e}"))?;
                if let Event::Incoming(Incoming::Publish(p)) = ev {
                    // Defensive: rumqttc can deliver frames outside
                    // our subscribed filters if the broker's own
                    // bridge config cross-publishes. Re-check.
                    if !topic_matches_any(&p.topic, MIRROR_FILTERS) {
                        continue;
                    }
                    if let Err(e) = up_client
                        .publish(p.topic.clone(), QoS::AtLeastOnce, p.retain, p.payload)
                        .await
                    {
                        log::warn!(
                            "[smart_home::mqtt::bridge] {} upstream publish {} failed: {}",
                            self.cfg.label,
                            p.topic,
                            e
                        );
                    }
                }
            }
        }
        .await;

        up_poll.abort();
        outcome
    }
}

fn build_client(
    url: &str,
    client_id_prefix: &str,
) -> Result<(AsyncClient, rumqttc::EventLoop), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("url: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "url missing host".to_string())?
        .to_string();
    let default_port = match parsed.scheme() {
        "mqtts" | "ssl" => 8883,
        _ => 1883,
    };
    let port = parsed.port().unwrap_or(default_port);
    let client_id = format!(
        "{}-{}",
        client_id_prefix,
        std::process::id().to_string().chars().take(6).collect::<String>()
    );
    let mut opts = MqttOptions::new(client_id, host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    if !parsed.username().is_empty() {
        opts.set_credentials(
            parsed.username().to_string(),
            parsed.password().unwrap_or("").to_string(),
        );
    }
    Ok(AsyncClient::new(opts, 256))
}

fn redact(url: &str) -> String {
    if let (Some(scheme_end), Some(at)) = (url.find("://"), url.find('@')) {
        if at > scheme_end + 3 {
            let mut s = String::new();
            s.push_str(&url[..scheme_end + 3]);
            s.push_str("***@");
            s.push_str(&url[at + 1..]);
            return s;
        }
    }
    url.to_string()
}

/// True if `topic` matches any of `filters` using MQTT wildcards
/// (`+` = single level, `#` = multi). No shared-subscription syntax.
pub fn topic_matches_any(topic: &str, filters: &[&str]) -> bool {
    filters.iter().any(|f| topic_matches(topic, f))
}

fn topic_matches(topic: &str, filter: &str) -> bool {
    let t: Vec<&str> = topic.split('/').collect();
    let f: Vec<&str> = filter.split('/').collect();
    let mut ti = 0usize;
    let mut fi = 0usize;
    while ti < t.len() && fi < f.len() {
        match f[fi] {
            "#" => return true,
            "+" => {
                ti += 1;
                fi += 1;
            }
            s if s == t[ti] => {
                ti += 1;
                fi += 1;
            }
            _ => return false,
        }
    }
    // MQTT: `a/#` matches `a` as well as `a/anything`. If we ran out of
    // topic segments and the next filter segment is a lone `#`, the
    // parent matches.
    if ti == t.len() && fi + 1 == f.len() && f[fi] == "#" {
        return true;
    }
    ti == t.len() && fi == f.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_plus_matches_single_level() {
        assert!(topic_matches("a/b/c", "a/+/c"));
        assert!(topic_matches("a/b/c", "a/+/+"));
        assert!(!topic_matches("a/b/c/d", "a/+/+"));
        assert!(!topic_matches("a/b", "a/+/+"));
    }

    #[test]
    fn wildcard_hash_matches_tail() {
        assert!(topic_matches("a/b/c/d", "a/#"));
        assert!(topic_matches("a", "a/#"));
        assert!(!topic_matches("b", "a/#"));
    }

    #[test]
    fn literal_topic_must_match_exactly() {
        assert!(topic_matches("a/b", "a/b"));
        assert!(!topic_matches("a/b", "a/c"));
        assert!(!topic_matches("a/b/c", "a/b"));
    }

    #[test]
    fn matches_ha_discovery_shapes() {
        let filters = MIRROR_FILTERS;
        // With node_id: 5 parts → `homeassistant/+/+/+/config`.
        assert!(topic_matches_any(
            "homeassistant/switch/syntaur/7/config",
            filters
        ));
        // Without node_id: 4 parts → `homeassistant/+/+/config`. This
        // shape isn't emitted by our HADiscoveryPublisher today but the
        // filter matches it for forward compatibility.
        assert!(topic_matches_any("homeassistant/light/7/config", filters));
    }

    #[test]
    fn matches_syntaur_state_topic() {
        let filters = MIRROR_FILTERS;
        assert!(topic_matches_any(
            "syntaur/u/1/device/7/state",
            filters
        ));
    }

    #[test]
    fn rejects_foreign_topics() {
        let filters = MIRROR_FILTERS;
        assert!(!topic_matches_any(
            "zigbee2mqtt/bridge/devices",
            filters
        ));
        assert!(!topic_matches_any(
            "syntaur/u/1/device/7/cmd",
            filters
        ));
    }

    #[test]
    fn redact_masks_credentials() {
        assert_eq!(
            redact("mqtt://u:p@host:1883"),
            "mqtt://***@host:1883"
        );
        assert_eq!(
            redact("mqtt://host:1883"),
            "mqtt://host:1883"
        );
    }
}
