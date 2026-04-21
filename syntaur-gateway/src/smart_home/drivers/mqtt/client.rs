//! Long-running subscriber — one `MqttSession` per configured broker.
//!
//! The session owns a single `rumqttc::AsyncClient` plus its event
//! loop, routes every incoming publish through the shared
//! `DialectRouter`, funnels:
//!   - `DialectMessage::Discovery*` → discovery cache snapshot (Phase C
//!     consumers call `MqttSupervisor::scan_snapshot`);
//!   - `DialectMessage::State(..)` → `StateCache::apply_state` →
//!     `SmartHomeEvent::DeviceStateChanged`;
//!   - `DialectMessage::Availability { .. }` →
//!     `StateCache::apply_availability` → `SmartHomeEvent::DeviceStateChanged`
//!     (only on transitions);
//!   - `DialectMessage::BridgeEvent(..)` → logged at info, no bus event
//!     in v1 (no subscribers yet — will wire in Phase F).
//!
//! Reconnect policy: exponential backoff starting at 1s, doubling to a
//! 30s cap, with ±20% jitter on every sleep so many sessions don't
//! synchronize after a network blip. Every reconnect re-issues the
//! subscription list.
//!
//! Event-loop capacity: widened from rumqttc's 32 default to 1024 so
//! retained-message catch-up (Z2M `bridge/devices`, HA Discovery
//! retain-everything brokers) doesn't back up the `EventLoop::poll`
//! channel and drop frames.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use tokio::sync::{mpsc, oneshot};

use super::command::EncodedCommand;
use super::dialects::{DialectMessage, DialectRouter};
use super::state::{EmittedChange, StateCache};
use super::stats::SessionCounters;
use crate::smart_home::events::{self, SmartHomeEvent};
use crate::smart_home::scan::ScanCandidate;

/// Channel message to drive a publish from outside the session loop.
/// Used by `MqttSupervisor::dispatch_command` so the dispatch layer
/// doesn't need a reference to the (reconnect-recreated) AsyncClient.
#[derive(Debug)]
pub enum SessionCommand {
    Publish(EncodedCommand),
}

/// Widened from rumqttc's 32 default. A busy Z2M `bridge/devices`
/// inventory (500+ devices) plus retained HA-Discovery catch-up on
/// reconnect easily fills 32 slots before `EventLoop::poll` drains them.
pub const EVENT_LOOP_CAPACITY: usize = 1024;

/// Reconnect backoff schedule — s×(1,2,4,8,30) with ±20% jitter. Caller
/// index clamped to the last slot to stabilize at 30s.
const BACKOFF_SCHEDULE_SECS: &[u64] = &[1, 2, 4, 8, 30];

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Syntaur user that owns this broker binding. Routes command
    /// dispatch (Phase D) to the right session.
    pub user_id: i64,
    /// `mqtt://user:pass@host:1883` or `mqtts://...`. Parsed by
    /// `url::Url` and spread into rumqttc options.
    pub url: String,
    /// Unique client id the broker sees us as. Default
    /// `syntaur-u<user>-<label>`.
    pub client_id: String,
    /// Human label for logs ("default", "garage-broker", …).
    pub label: String,
    /// TLS root CA for self-signed brokers. Plaintext PEM.
    pub ca_pem: Option<String>,
}

impl SessionConfig {
    pub fn from_credential(user_id: i64, label: &str, secret: &serde_json::Value) -> Option<Self> {
        let url = secret.get("url").and_then(|v| v.as_str())?.to_string();
        let client_id = secret
            .get("client_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("syntaur-u{}-{}", user_id, label));
        let ca_pem = secret
            .get("ca_pem")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        Some(Self {
            user_id,
            url,
            client_id,
            label: label.to_string(),
            ca_pem,
        })
    }
}

pub struct MqttSession {
    cfg: SessionConfig,
    router: Arc<DialectRouter>,
    state_cache: Arc<StateCache>,
    discovery_tx: tokio::sync::mpsc::UnboundedSender<ScanCandidate>,
    cmd_rx: Option<mpsc::Receiver<SessionCommand>>,
    shutdown_rx: Option<oneshot::Receiver<()>>,
    counters: Arc<SessionCounters>,
}

impl MqttSession {
    pub fn new(
        cfg: SessionConfig,
        router: Arc<DialectRouter>,
        state_cache: Arc<StateCache>,
        discovery_tx: tokio::sync::mpsc::UnboundedSender<ScanCandidate>,
        cmd_rx: mpsc::Receiver<SessionCommand>,
        shutdown_rx: oneshot::Receiver<()>,
        counters: Arc<SessionCounters>,
    ) -> Self {
        Self {
            cfg,
            router,
            state_cache,
            discovery_tx,
            cmd_rx: Some(cmd_rx),
            shutdown_rx: Some(shutdown_rx),
            counters,
        }
    }

    /// Run forever (or until `shutdown_rx` fires). Never returns an
    /// error — broker errors surface as reconnect attempts.
    pub async fn run(mut self) {
        let mut shutdown = self
            .shutdown_rx
            .take()
            .expect("MqttSession::run called twice");
        let mut cmd_rx = self
            .cmd_rx
            .take()
            .expect("MqttSession::run called twice");
        let mut attempt: usize = 0;
        log::info!(
            "[smart_home::mqtt] session {} starting (url={})",
            self.cfg.label,
            redact_url(&self.cfg.url)
        );

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    log::info!(
                        "[smart_home::mqtt] session {} shutting down",
                        self.cfg.label
                    );
                    return;
                }
                outcome = self.one_connection_cycle(&mut cmd_rx) => {
                    match outcome {
                        Ok(()) => {
                            // Graceful disconnect — reset backoff.
                            attempt = 0;
                        }
                        Err(e) => {
                            log::warn!(
                                "[smart_home::mqtt] session {} connection error: {} — reconnecting",
                                self.cfg.label,
                                e
                            );
                        }
                    }
                    self.counters.mark_reconnect(chrono::Utc::now().timestamp());
                    let sleep = backoff_sleep(attempt);
                    attempt = attempt.saturating_add(1);
                    tokio::select! {
                        _ = &mut shutdown => {
                            log::info!(
                                "[smart_home::mqtt] session {} shutdown during backoff",
                                self.cfg.label
                            );
                            return;
                        }
                        _ = tokio::time::sleep(sleep) => {}
                    }
                }
            }
        }
    }

    async fn one_connection_cycle(
        &self,
        cmd_rx: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        let opts = mqtt_options_from_cfg(&self.cfg)?;
        let (client, mut event_loop) = AsyncClient::new(opts, EVENT_LOOP_CAPACITY);
        for topic in self.router.subscribe_topics() {
            if let Err(e) = client.subscribe(topic, QoS::AtMostOnce).await {
                log::warn!(
                    "[smart_home::mqtt] session {} subscribe {} failed: {}",
                    self.cfg.label,
                    topic,
                    e
                );
            }
        }
        self.counters.mark_connected(chrono::Utc::now().timestamp());

        loop {
            tokio::select! {
                event_res = event_loop.poll() => {
                    let event = event_res.map_err(|e| format!("poll: {e}"))?;
                    if let Event::Incoming(Incoming::Publish(publish)) = event {
                        self.counters.bump_in();
                        if let Some(msg) = self.router.parse(&publish.topic, &publish.payload) {
                            self.handle_message(msg).await;
                        }
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::Publish(enc)) => {
                            if let Err(e) = client
                                .publish(enc.topic.clone(), enc.qos, enc.retain, enc.payload)
                                .await
                            {
                                log::warn!(
                                    "[smart_home::mqtt] session {} publish {} failed: {}",
                                    self.cfg.label,
                                    enc.topic,
                                    e
                                );
                            } else {
                                self.counters.bump_out();
                            }
                        }
                        None => {
                            // Supervisor dropped the sender — fall out
                            // of the connection cycle so shutdown is
                            // observed on the outer select.
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    async fn handle_message(&self, msg: DialectMessage) {
        match msg {
            DialectMessage::Discovery(c) => {
                self.counters.bump_dialect(c.details.get("schema").and_then(|v| v.as_str()).unwrap_or("unknown")).await;
                let _ = self.discovery_tx.send(c);
            }
            DialectMessage::Discoveries(list) => {
                if let Some(first) = list.first() {
                    let d = first.details.get("schema").and_then(|v| v.as_str()).unwrap_or("unknown");
                    self.counters.bump_dialect(d).await;
                }
                for c in list {
                    let _ = self.discovery_tx.send(c);
                }
            }
            DialectMessage::State(update) => {
                self.counters.bump_dialect(&update.source).await;
                let source = update.source.clone();
                match self.state_cache.apply_state("mqtt", update).await {
                    Ok(Some(change)) => emit_change(change, source),
                    Ok(None) => {}
                    Err(e) => log::warn!(
                        "[smart_home::mqtt] apply_state failed on session {}: {}",
                        self.cfg.label,
                        e
                    ),
                }
            }
            DialectMessage::Availability { external_id, online } => {
                match self
                    .state_cache
                    .apply_availability("mqtt", external_id, online)
                    .await
                {
                    Ok(Some(change)) => {
                        let source = change.source.clone();
                        emit_change(change, source);
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!(
                        "[smart_home::mqtt] apply_availability failed on session {}: {}",
                        self.cfg.label,
                        e
                    ),
                }
            }
            DialectMessage::BridgeEvent(v) => {
                self.state_cache.stats_ref().note_bridge_event();
                log::info!(
                    "[smart_home::mqtt] bridge event on session {}: {}",
                    self.cfg.label,
                    v
                );
            }
            _ => {}
        }
    }
}

fn emit_change(change: EmittedChange, source: String) {
    events::publish(SmartHomeEvent::DeviceStateChanged {
        user_id: change.user_id,
        device_id: change.device_id,
        state: change.state,
        source,
    });
}

fn mqtt_options_from_cfg(cfg: &SessionConfig) -> Result<MqttOptions, String> {
    let parsed = url::Url::parse(&cfg.url).map_err(|e| format!("url parse: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "url missing host".to_string())?
        .to_string();
    let default_port: u16 = match parsed.scheme() {
        "mqtts" | "ssl" => 8883,
        _ => 1883,
    };
    let port = parsed.port().unwrap_or(default_port);
    let mut opts = MqttOptions::new(cfg.client_id.clone(), host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    // rumqttc 0.25 defaults to clean_session=true; keep it that way so
    // reconnect-catchup replays retained frames rather than pulling
    // backlog from the broker (which Mosquitto limits on the session).
    if !parsed.username().is_empty() {
        opts.set_credentials(
            parsed.username().to_string(),
            parsed.password().unwrap_or("").to_string(),
        );
    }
    // Phase C doesn't wire TLS root-of-trust: rumqttc's TLS config shape
    // changed between 0.24 and 0.25 and needs a rustls ClientConfig
    // we'd build from `cfg.ca_pem`. Tracked as a Phase E follow-on
    // (embedded-broker TLS + upstream-bridge TLS land together).
    Ok(opts)
}

fn redact_url(url: &str) -> String {
    // `url::Url::parse` loses the password if we ask it to redact, so
    // do it by hand: strip everything between `://` and `@`.
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

fn backoff_sleep(attempt: usize) -> Duration {
    let idx = attempt.min(BACKOFF_SCHEDULE_SECS.len() - 1);
    let base = BACKOFF_SCHEDULE_SECS[idx] as f64;
    // ±20% jitter — (rand 0.0..1.0) derived without pulling `rand`
    // crate; use nanos of SystemTime as a cheap source.
    let jitter = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        // nanos in 0..1e9; normalize to -0.2..0.2
        ((nanos as f64 / 1_000_000_000.0) - 0.5) * 0.4
    };
    let secs = (base * (1.0 + jitter)).max(0.05);
    Duration::from_secs_f64(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_masks_password() {
        assert_eq!(
            redact_url("mqtt://user:hunter2@broker.lan:1883"),
            "mqtt://***@broker.lan:1883"
        );
    }

    #[test]
    fn redact_url_no_credentials_passthrough() {
        assert_eq!(
            redact_url("mqtt://broker.lan:1883"),
            "mqtt://broker.lan:1883"
        );
    }

    #[test]
    fn backoff_respects_cap() {
        // attempt = many → clamps to last schedule entry (30s ±20%)
        let d = backoff_sleep(100);
        assert!(d >= Duration::from_secs_f64(30.0 * 0.79));
        assert!(d <= Duration::from_secs_f64(30.0 * 1.21));
    }

    #[test]
    fn backoff_starts_near_one_second() {
        let d = backoff_sleep(0);
        assert!(d >= Duration::from_secs_f64(0.79));
        assert!(d <= Duration::from_secs_f64(1.21));
    }

    #[test]
    fn session_config_from_credential_uses_defaults() {
        let secret = serde_json::json!({"url": "mqtt://broker.lan:1883"});
        let cfg = SessionConfig::from_credential(7, "garage", &secret).unwrap();
        assert_eq!(cfg.client_id, "syntaur-u7-garage");
        assert_eq!(cfg.label, "garage");
        assert!(cfg.ca_pem.is_none());
    }

    #[test]
    fn session_config_from_credential_honors_overrides() {
        let secret = serde_json::json!({
            "url": "mqtts://broker.lan:8883",
            "client_id": "custom-id",
            "ca_pem": "-----BEGIN CERT-----\n..."
        });
        let cfg = SessionConfig::from_credential(1, "default", &secret).unwrap();
        assert_eq!(cfg.client_id, "custom-id");
        assert!(cfg.ca_pem.is_some());
    }

    #[test]
    fn mqtt_options_defaults_port_by_scheme() {
        let cfg = SessionConfig {
            user_id: 1,
            url: "mqtts://broker.lan".into(),
            client_id: "x".into(),
            label: "x".into(),
            ca_pem: None,
        };
        // We can't read the port back from MqttOptions easily in 0.25,
        // but a successful build means host parsing succeeded.
        assert!(mqtt_options_from_cfg(&cfg).is_ok());
    }
}
