//! Embeddable MQTT broker — the "HA for normal people" default.
//!
//! Boots `rumqttd` 0.20 on 127.0.0.1:1884 so a Syntaur install with no
//! existing Mosquitto gives the user a working broker without them
//! knowing what MQTT is. Power users with an existing broker disable
//! via `SMART_HOME_EMBEDDED_BROKER=off` and point the supervisor at
//! their upstream via the `smart_home_credentials` table.
//!
//! Port choice: 1884, never 1883. Mosquitto's default is 1883; if the
//! user already has one listening we would collide on every init. The
//! standard "not-Mosquitto" convention is 1884, so that's what we
//! claim. TLS and auth are v1.1+ territory — v1 ships plaintext
//! localhost-only, which is safe because the listener is bound to
//! `127.0.0.1` (no LAN exposure).
//!
//! ## Bind-conflict handling
//!
//! `EmbeddedBroker::spawn` probes the address with `TcpListener::bind`
//! before handing rumqttd its config. On `EADDRINUSE` we log a
//! structured warning and return `None` — the supervisor and the rest
//! of `smart_home::init` continue. We never panic, never fail init.
//!
//! ## Shutdown
//!
//! `rumqttd::Broker::start()` is a blocking call that joins its own
//! internal OS threads. There is no graceful shutdown API in 0.20, so
//! the broker lives until the process exits. Acceptable for v1; if
//! integration tests need a fresh broker per run we'll bind to port 0
//! and accept leakage within the test process.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};

use rumqttd::{Broker, Config, ConnectionSettings, RouterConfig, ServerSettings};

/// Default bind for the embedded broker. Port 1884 keeps Mosquitto on
/// 1883 untouched, and binding to 127.0.0.1 keeps the broker off the
/// LAN until auth + TLS land in Phase E-2.
pub const DEFAULT_BIND: &str = "127.0.0.1:1884";

/// Env var to override / disable the embedded broker.
/// Values:
///   unset / ""  — use `DEFAULT_BIND`
///   "off" / "0" — disable entirely
///   "host:port" — bind there (e.g. "0.0.0.0:1884" or "127.0.0.1:18830")
pub const ENV_VAR: &str = "SMART_HOME_EMBEDDED_BROKER";

/// Started broker. Dropping this value does NOT shut the broker down —
/// rumqttd 0.20 has no public shutdown API. Kept as a handle mainly so
/// callers can log the effective bind address.
pub struct EmbeddedBroker {
    bind: SocketAddr,
}

impl EmbeddedBroker {
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Honor `SMART_HOME_EMBEDDED_BROKER` if set, otherwise spawn at
    /// the default bind. Returns `None` when the broker was explicitly
    /// disabled or the bind failed.
    pub fn from_env_or_default() -> Option<Self> {
        match std::env::var(ENV_VAR) {
            Ok(v) if v.is_empty() => Self::spawn_default(),
            Ok(v) if v.eq_ignore_ascii_case("off") || v == "0" => {
                log::info!(
                    "[smart_home::mqtt] embedded broker disabled via {}={}",
                    ENV_VAR,
                    v
                );
                None
            }
            Ok(v) => match v.parse::<SocketAddr>() {
                Ok(addr) => Self::spawn(addr),
                Err(e) => {
                    log::warn!(
                        "[smart_home::mqtt] invalid {}={} ({}); using {}",
                        ENV_VAR,
                        v,
                        e,
                        DEFAULT_BIND
                    );
                    Self::spawn_default()
                }
            },
            Err(_) => Self::spawn_default(),
        }
    }

    fn spawn_default() -> Option<Self> {
        let addr: SocketAddr = DEFAULT_BIND
            .parse()
            .expect("DEFAULT_BIND is a valid socket addr");
        Self::spawn(addr)
    }

    /// Spawn the broker on a dedicated OS thread. `rumqttd::Broker::start`
    /// creates its own threads per protocol server and joins them, so we
    /// park the whole thing off the tokio runtime.
    ///
    /// Returns `None` when the address can't be bound (port already in
    /// use, permission denied on low ports, etc.) — `smart_home::init`
    /// continues in that case; the supervisor just has no in-process
    /// broker to use.
    pub fn spawn(bind: SocketAddr) -> Option<Self> {
        // Probe the bind before handing rumqttd a config it would fail
        // on inside a background thread. Structured log here is easier
        // to trace than a rumqttd IO error surfaced after thread spawn.
        match TcpListener::bind(bind) {
            Ok(listener) => drop(listener),
            Err(e) => {
                log::warn!(
                    "[smart_home::mqtt] embedded broker bind {} failed ({}); skipping",
                    bind,
                    e
                );
                return None;
            }
        }

        let config = build_config(bind);
        let handle = std::thread::Builder::new()
            .name("syntaur-mqtt-broker".into())
            .spawn(move || {
                let mut broker = Broker::new(config);
                if let Err(e) = broker.start() {
                    log::error!("[smart_home::mqtt] embedded broker exited: {}", e);
                }
            });
        if let Err(e) = handle {
            log::warn!("[smart_home::mqtt] broker thread spawn failed: {}", e);
            return None;
        }

        log::info!("[smart_home::mqtt] embedded broker listening on {}", bind);
        Some(Self { bind })
    }
}

fn build_config(bind: SocketAddr) -> Config {
    let mut v4 = HashMap::new();
    v4.insert(
        "v4-syntaur".to_string(),
        ServerSettings {
            name: "v4-syntaur".into(),
            listen: bind,
            tls: None,
            next_connection_delay_ms: 1,
            connections: ConnectionSettings {
                connection_timeout_ms: 60_000,
                // 256 KB is comfortable for retained Z2M `bridge/devices`
                // with ~100 devices; rumqttd's default 20KB is too tight.
                max_payload_size: 256 * 1024,
                max_inflight_count: 500,
                auth: None,
                external_auth: None,
                dynamic_filters: true,
            },
        },
    );

    Config {
        id: 0,
        router: RouterConfig {
            // v1 is a home-scale broker — 256 is far beyond what a
            // typical household needs. Bumping later is cheap.
            max_connections: 256,
            max_outgoing_packet_count: 200,
            // 8 MB × 4 segments = 32 MB retained storage ceiling.
            // rumqttd's 100 MB × 10 default is wasteful on a VM.
            max_segment_size: 8 * 1024 * 1024,
            max_segment_count: 4,
            custom_segment: None,
            initialized_filters: None,
            shared_subscriptions_strategy: Default::default(),
        },
        v4: Some(v4),
        v5: None,
        ws: None,
        cluster: None,
        console: None,
        bridge: None,
        prometheus: None,
        metrics: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_has_v4_server() {
        let cfg = build_config("127.0.0.1:18830".parse().unwrap());
        let v4 = cfg.v4.as_ref().expect("v4 configured");
        assert_eq!(v4.len(), 1);
        let s = v4.values().next().unwrap();
        assert_eq!(s.listen.port(), 18830);
        assert!(s.tls.is_none());
        assert!(s.connections.external_auth.is_none());
    }

    #[test]
    fn build_config_router_caps_are_modest() {
        let cfg = build_config(DEFAULT_BIND.parse().unwrap());
        assert!(cfg.router.max_connections >= 16 && cfg.router.max_connections <= 1024);
        assert!(cfg.router.max_segment_count >= 2);
    }

    #[test]
    fn default_bind_parses() {
        let addr: SocketAddr = DEFAULT_BIND.parse().unwrap();
        assert_eq!(addr.port(), 1884);
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn spawn_returns_none_on_address_in_use() {
        // Claim an ephemeral port ourselves, then ask the broker to
        // bind the same one — must return None, never panic.
        let listener = TcpListener::bind("127.0.0.1:0").expect("probe listener");
        let addr = listener.local_addr().unwrap();
        // Leave the listener bound for the duration of the call.
        let out = EmbeddedBroker::spawn(addr);
        assert!(out.is_none(), "expected soft-fail on EADDRINUSE");
        drop(listener);
    }
}
