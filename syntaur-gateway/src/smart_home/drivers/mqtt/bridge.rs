//! Upstream MQTT bridge — bidirectional mirror between the gateway's
//! primary broker (usually the embedded `rumqttd` on :1884) and the
//! user's existing broker (Mosquitto / HA-OS / etc.).
//!
//! The goal: a household running HA at `mosquitto.lan` sees every
//! Syntaur-native device without configuring HA against our embedded
//! broker directly, AND can control Syntaur devices from their HA
//! instance by publishing to the familiar `syntaur/u/<user>/device/<id>/cmd`
//! topic. The bridge relays both directions:
//!
//! ```text
//!   downstream (embedded)                    upstream (user's Mosquitto)
//!   └── SH_DOWN_FILTERS  ──────────────────→ republish verbatim
//!         (HA Discovery configs, state)
//!
//!   downstream (embedded) ←────────────────── SH_UP_FILTERS
//!                                             (Syntaur command topics)
//! ```
//!
//! Loop prevention: the two direction filters are **disjoint by
//! construction**. Nothing we forward downstream→upstream overlaps
//! with what we forward upstream→downstream:
//!   - Downstream→upstream carries `homeassistant/*/config` (HA
//!     Discovery writes from `HADiscoveryPublisher`) + Syntaur's
//!     own `syntaur/u/+/device/+/state` state feeds.
//!   - Upstream→downstream carries `syntaur/u/+/device/+/cmd` — the
//!     HA command topic HA publishes to drive devices.
//! Because the topic namespaces don't overlap, a mirrored message
//! can't re-enter the bridge from the opposite side. No MQTT-5
//! user-property tag needed; no topic-prefix dance. If a user later
//! wants a wider bidirectional mirror (not the case for our HA-control
//! use case), that's when the tagging mitigation lands.
//!
//! Triggered by the credential's secret blob carrying
//!   `"bridge_to": "mqtt://[user:pass@]host[:port]"`
//! Absent, the session runs without a bridge.

use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use tokio::sync::oneshot;

/// Topics Syntaur publishes that the bridge mirrors downstream →
/// upstream. Keeping this narrow stops the bridge from flooding the
/// user's upstream with `zigbee2mqtt/bridge/devices` retained
/// inventories etc. that are already available on their source of
/// truth.
const SH_DOWN_FILTERS: &[&str] = &[
    "homeassistant/+/+/config",
    "homeassistant/+/+/+/config",
    "syntaur/u/+/device/+/state",
];

/// Topics the bridge mirrors upstream → downstream. Scoped to HA's
/// command topic pattern so a user's HA instance running against
/// their upstream broker can drive Syntaur devices. Keeping this
/// narrower than SH_DOWN_FILTERS guarantees no overlap → no loops.
const SH_UP_FILTERS: &[&str] = &["syntaur/u/+/device/+/cmd"];

// Preserve the old public name for callers/tests that imported it.
#[allow(dead_code)]
pub const MIRROR_FILTERS: &[&str] = SH_DOWN_FILTERS;

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
        // Four clients, one pair per direction. Each pair has a
        // "reader" subscribed on the source broker and a "writer" that
        // publishes to the destination broker. Neither pair overlaps
        // the other's topic filters so there's no self-echo risk.
        let (down_read_client, mut down_read_loop) =
            build_client(&self.cfg.downstream_url, "syntaur-bridge-down-read")?;
        let (up_write_client, mut up_write_loop) =
            build_client(&self.cfg.upstream_url, "syntaur-bridge-up-write")?;
        let (up_read_client, mut up_read_loop) =
            build_client(&self.cfg.upstream_url, "syntaur-bridge-up-read")?;
        let (down_write_client, mut down_write_loop) =
            build_client(&self.cfg.downstream_url, "syntaur-bridge-down-write")?;

        for filter in SH_DOWN_FILTERS {
            if let Err(e) = down_read_client
                .subscribe(*filter, QoS::AtLeastOnce)
                .await
            {
                log::warn!(
                    "[smart_home::mqtt::bridge] {} down sub {} failed: {}",
                    self.cfg.label,
                    filter,
                    e
                );
            }
        }
        for filter in SH_UP_FILTERS {
            if let Err(e) = up_read_client.subscribe(*filter, QoS::AtLeastOnce).await {
                log::warn!(
                    "[smart_home::mqtt::bridge] {} up sub {} failed: {}",
                    self.cfg.label,
                    filter,
                    e
                );
            }
        }
        log::info!(
            "[smart_home::mqtt::bridge] {} mirroring {} ⇄ {}",
            self.cfg.label,
            redact(&self.cfg.downstream_url),
            redact(&self.cfg.upstream_url)
        );

        // Keep the write-only clients' event loops serviced so their
        // publish queues drain (rumqttc requires polling even when
        // nothing is subscribed).
        let up_write_poll = tokio::spawn(async move {
            loop {
                if up_write_loop.poll().await.is_err() {
                    break;
                }
            }
        });
        let down_write_poll = tokio::spawn(async move {
            loop {
                if down_write_loop.poll().await.is_err() {
                    break;
                }
            }
        });

        // Downstream → upstream task.
        let down_up_label = self.cfg.label.clone();
        let down_up = tokio::spawn(async move {
            loop {
                let ev = match down_read_loop.poll().await {
                    Ok(e) => e,
                    Err(e) => {
                        return Err::<(), String>(format!("down poll: {e}"));
                    }
                };
                if let Event::Incoming(Incoming::Publish(p)) = ev {
                    if !topic_matches_any(&p.topic, SH_DOWN_FILTERS) {
                        continue;
                    }
                    if let Err(e) = up_write_client
                        .publish(
                            p.topic.clone(),
                            QoS::AtLeastOnce,
                            p.retain,
                            p.payload,
                        )
                        .await
                    {
                        log::warn!(
                            "[smart_home::mqtt::bridge] {} down→up publish {} failed: {}",
                            down_up_label,
                            p.topic,
                            e
                        );
                    }
                }
            }
        });

        // Upstream → downstream task.
        let up_down_label = self.cfg.label.clone();
        let up_down = tokio::spawn(async move {
            loop {
                let ev = match up_read_loop.poll().await {
                    Ok(e) => e,
                    Err(e) => {
                        return Err::<(), String>(format!("up poll: {e}"));
                    }
                };
                if let Event::Incoming(Incoming::Publish(p)) = ev {
                    if !topic_matches_any(&p.topic, SH_UP_FILTERS) {
                        continue;
                    }
                    if let Err(e) = down_write_client
                        .publish(
                            p.topic.clone(),
                            QoS::AtLeastOnce,
                            p.retain,
                            p.payload,
                        )
                        .await
                    {
                        log::warn!(
                            "[smart_home::mqtt::bridge] {} up→down publish {} failed: {}",
                            up_down_label,
                            p.topic,
                            e
                        );
                    }
                }
            }
        });

        // If either mirror task exits we tear the whole cycle down —
        // reconnect policy lives in the outer loop.
        let outcome: Result<(), String> = tokio::select! {
            r = down_up => r.unwrap_or_else(|e| Err(format!("down→up join: {e}"))),
            r = up_down => r.unwrap_or_else(|e| Err(format!("up→down join: {e}"))),
        };

        up_write_poll.abort();
        down_write_poll.abort();
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
    fn up_filters_cover_command_topic() {
        assert!(topic_matches_any(
            "syntaur/u/1/device/7/cmd",
            SH_UP_FILTERS
        ));
    }

    #[test]
    fn down_and_up_filters_are_disjoint() {
        // The bridge's no-loop guarantee hinges on these being
        // disjoint. A mirror-eligible frame on one side must NEVER
        // match the opposite-side filter. If someone adds a shared
        // topic, this test fails loud.
        let down_cases = &[
            "homeassistant/switch/syntaur/7/config",
            "homeassistant/light/7/config",
            "syntaur/u/1/device/7/state",
        ];
        for t in down_cases {
            assert!(topic_matches_any(t, SH_DOWN_FILTERS));
            assert!(
                !topic_matches_any(t, SH_UP_FILTERS),
                "DOWN filter topic {} also matches UP filter — loop risk",
                t
            );
        }
        let up_cases = &["syntaur/u/1/device/7/cmd"];
        for t in up_cases {
            assert!(topic_matches_any(t, SH_UP_FILTERS));
            assert!(
                !topic_matches_any(t, SH_DOWN_FILTERS),
                "UP filter topic {} also matches DOWN filter — loop risk",
                t
            );
        }
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
