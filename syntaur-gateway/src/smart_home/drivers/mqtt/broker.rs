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

/// Constant-time byte compare — avoids timing leaks on the password
/// check. `subtle` would be the right dep but it isn't in the workspace
/// and the password check runs once per connect (not per message), so
/// a hand-rolled loop is fine here.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

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

/// Env var controlling connection authentication. Phase E-2:
///   unset / ""         — noauth (default; localhost-only is safe)
///   "open" / "off"     — noauth, explicit
///   "password:<pw>"    — any username, `<pw>` required
/// Per-user HMAC tokens + ACL rewriting into `u/<user_id>/...`
/// namespace are a future iteration tied to the settings UI. For v1 a
/// static shared secret is enough to cover the "I want to bind to
/// 0.0.0.0" scenario without a full identity model.
pub const AUTH_ENV_VAR: &str = "SMART_HOME_EMBEDDED_BROKER_AUTH";

/// Parsed auth policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPolicy {
    /// No authentication — anyone who can reach the listener connects.
    Open,
    /// Single shared secret; any username accepted if password matches.
    Password(String),
}

impl AuthPolicy {
    /// Parse the `SMART_HOME_EMBEDDED_BROKER_AUTH` environment
    /// surface. Unknown values log a warning and degrade to `Open` so
    /// a typo doesn't silently lock the broker.
    pub fn from_env() -> Self {
        match std::env::var(AUTH_ENV_VAR) {
            Ok(v) => Self::parse(&v),
            Err(_) => Self::Open,
        }
    }

    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("open")
            || trimmed.eq_ignore_ascii_case("off")
        {
            return Self::Open;
        }
        if let Some(pw) = trimmed.strip_prefix("password:") {
            if pw.is_empty() {
                log::warn!(
                    "[smart_home::mqtt] {} password is empty — degrading to open",
                    AUTH_ENV_VAR
                );
                return Self::Open;
            }
            return Self::Password(pw.to_string());
        }
        log::warn!(
            "[smart_home::mqtt] unrecognized {} value '{}' — degrading to open",
            AUTH_ENV_VAR,
            trimmed
        );
        Self::Open
    }
}

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
                Ok(addr) => Self::spawn_with(addr, AuthPolicy::from_env()),
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
        Self::spawn_with(addr, AuthPolicy::from_env())
    }

    /// Spawn the broker with `AuthPolicy::Open`. Convenience for the
    /// default no-auth + localhost-only v1 path.
    pub fn spawn(bind: SocketAddr) -> Option<Self> {
        Self::spawn_with(bind, AuthPolicy::Open)
    }

    /// Spawn the broker on a dedicated OS thread. `rumqttd::Broker::start`
    /// creates its own threads per protocol server and joins them, so we
    /// park the whole thing off the tokio runtime.
    ///
    /// Returns `None` when the address can't be bound (port already in
    /// use, permission denied on low ports, etc.) — `smart_home::init`
    /// continues in that case; the supervisor just has no in-process
    /// broker to use.
    pub fn spawn_with(bind: SocketAddr, auth: AuthPolicy) -> Option<Self> {
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

        let auth_label = match &auth {
            AuthPolicy::Open => "open".to_string(),
            AuthPolicy::Password(_) => "password".to_string(),
        };
        let config = build_config(bind, auth);
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

        log::info!(
            "[smart_home::mqtt] embedded broker listening on {} (auth={})",
            bind,
            auth_label
        );
        Some(Self { bind })
    }
}

fn build_config(bind: SocketAddr, auth: AuthPolicy) -> Config {
    let mut settings = ServerSettings {
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
    };
    match auth {
        AuthPolicy::Open => {}
        AuthPolicy::Password(expected) => {
            // rumqttd::ConnectionSettings::set_auth_handler expects a
            // closure returning `impl IntoFuture<Output = bool>`. Plain
            // async closures satisfy that; the password is captured by
            // value and compared constant-time against the incoming
            // frame.
            settings.set_auth_handler(
                move |_client_id: rumqttd::ClientId,
                      _user: rumqttd::AuthUser,
                      pass: rumqttd::AuthPass| {
                    let expected = expected.clone();
                    async move { ct_eq(pass.as_bytes(), expected.as_bytes()) }
                },
            );
        }
    }
    let mut v4 = HashMap::new();
    v4.insert("v4-syntaur".to_string(), settings);

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
        let cfg = build_config("127.0.0.1:18830".parse().unwrap(), AuthPolicy::Open);
        let v4 = cfg.v4.as_ref().expect("v4 configured");
        assert_eq!(v4.len(), 1);
        let s = v4.values().next().unwrap();
        assert_eq!(s.listen.port(), 18830);
        assert!(s.tls.is_none());
        assert!(s.connections.external_auth.is_none());
    }

    #[test]
    fn build_config_with_password_installs_auth_handler() {
        let cfg = build_config(
            "127.0.0.1:18831".parse().unwrap(),
            AuthPolicy::Password("hunter2".into()),
        );
        let v4 = cfg.v4.as_ref().unwrap();
        let s = v4.values().next().unwrap();
        assert!(s.connections.external_auth.is_some());
    }

    #[test]
    fn build_config_router_caps_are_modest() {
        let cfg = build_config(DEFAULT_BIND.parse().unwrap(), AuthPolicy::Open);
        assert!(cfg.router.max_connections >= 16 && cfg.router.max_connections <= 1024);
        assert!(cfg.router.max_segment_count >= 2);
    }

    #[test]
    fn auth_policy_parses_open_variants() {
        assert_eq!(AuthPolicy::parse(""), AuthPolicy::Open);
        assert_eq!(AuthPolicy::parse("open"), AuthPolicy::Open);
        assert_eq!(AuthPolicy::parse("OFF"), AuthPolicy::Open);
        assert_eq!(AuthPolicy::parse("  open  "), AuthPolicy::Open);
    }

    #[test]
    fn auth_policy_parses_password_form() {
        assert_eq!(
            AuthPolicy::parse("password:hunter2"),
            AuthPolicy::Password("hunter2".into())
        );
    }

    #[test]
    fn auth_policy_empty_password_degrades_to_open() {
        assert_eq!(AuthPolicy::parse("password:"), AuthPolicy::Open);
    }

    #[test]
    fn auth_policy_unknown_scheme_degrades_to_open() {
        assert_eq!(AuthPolicy::parse("bearer:x"), AuthPolicy::Open);
        assert_eq!(AuthPolicy::parse("garbage"), AuthPolicy::Open);
    }

    #[test]
    fn ct_eq_distinguishes_lengths_and_contents() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    /// Live wire-level auth: spawn a broker with Password("correct"),
    /// try to connect with the wrong password, assert rumqttc's event
    /// loop surfaces a connection error rather than completing the
    /// handshake. Then connect with the right password and prove a
    /// subsequent publish-subscribe round trips.
    #[test]
    fn password_auth_rejects_bad_and_accepts_good() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::time::Duration;
        use tokio::time::timeout;

        let mut broker_opt = None;
        let mut bind = None;
        for _ in 0..3 {
            let addr = claim_ephemeral_port();
            if let Some(b) = EmbeddedBroker::spawn_with(
                addr,
                AuthPolicy::Password("correct-horse-battery".into()),
            ) {
                broker_opt = Some(b);
                bind = Some(addr);
                break;
            }
        }
        let _broker = broker_opt.expect("broker spawn");
        let addr = bind.unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            for _ in 0..40 {
                if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50))
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            // Wrong password: event loop must surface an error within
            // a couple of seconds. rumqttc returns the error on the
            // poll that follows the rejected CONNECT.
            let mut bad_opts =
                MqttOptions::new("auth-bad", addr.ip().to_string(), addr.port());
            bad_opts.set_keep_alive(Duration::from_secs(5));
            bad_opts.set_credentials("anyuser", "definitely-wrong");
            let (_bad_client, mut bad_loop) = AsyncClient::new(bad_opts, 32);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let mut saw_error = false;
            while tokio::time::Instant::now() < deadline {
                match timeout(Duration::from_millis(300), bad_loop.poll()).await {
                    Ok(Err(_e)) => {
                        saw_error = true;
                        break;
                    }
                    Ok(Ok(Event::Incoming(Incoming::ConnAck(ack)))) => {
                        // Some brokers reply with a failing ConnAck
                        // before dropping the socket — also acceptable.
                        if matches!(
                            ack.code,
                            rumqttc::ConnectReturnCode::BadUserNamePassword
                                | rumqttc::ConnectReturnCode::NotAuthorized
                        ) {
                            saw_error = true;
                            break;
                        }
                    }
                    _ => continue,
                }
            }
            assert!(
                saw_error,
                "expected bad-password client to be rejected by broker"
            );

            // Right password: handshake + sub + pub round trip must
            // succeed, proving the auth handler accepts the matching
            // credential (not just rejecting everything).
            let mut good_opts =
                MqttOptions::new("auth-good", addr.ip().to_string(), addr.port());
            good_opts.set_keep_alive(Duration::from_secs(5));
            good_opts.set_credentials("anyuser", "correct-horse-battery");
            let (good_client, mut good_loop) = AsyncClient::new(good_opts, 32);
            good_client
                .subscribe("auth_ok/#", QoS::AtLeastOnce)
                .await
                .unwrap();
            // Service the event loop in the background while we also
            // publish from the same client.
            let poll_task = tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
                let mut saw = false;
                while tokio::time::Instant::now() < deadline {
                    match timeout(Duration::from_millis(300), good_loop.poll()).await {
                        Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                            if p.topic == "auth_ok/hello" && &*p.payload == b"y" {
                                saw = true;
                                break;
                            }
                        }
                        _ => continue,
                    }
                }
                saw
            });
            // Small wait so SUBACK lands before the publish.
            tokio::time::sleep(Duration::from_millis(300)).await;
            good_client
                .publish("auth_ok/hello", QoS::AtLeastOnce, false, b"y".to_vec())
                .await
                .unwrap();
            let saw = poll_task.await.unwrap();
            assert!(saw, "good-password client didn't receive its own publish");
        });
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

    /// Claim an ephemeral port by binding + dropping a probe listener.
    /// There's a tiny TOCTOU window before the broker takes the same
    /// port; single-process test suites rarely collide, and if one
    /// does, `EmbeddedBroker::spawn` cleanly returns `None` so we
    /// retry. Used by the round-trip test below.
    fn claim_ephemeral_port() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").expect("probe listener");
        let a = l.local_addr().unwrap();
        drop(l);
        a
    }

    /// Full publish/subscribe round trip through the embedded broker,
    /// using rumqttc (the same client the `MqttSession` uses in prod)
    /// to prove the broker accepts connections, delivers messages, and
    /// preserves retained state across a fresh subscription.
    ///
    /// The test body is multi-async and uses a bespoke tokio runtime
    /// because `spawn_returns_none_on_address_in_use` and the rest of
    /// the broker tests are plain `#[test]`s; importing `#[tokio::test]`
    /// just for one case would blow up build time on every broker
    /// change.
    #[test]
    fn round_trip_pub_sub_and_retained() {
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::time::Duration;
        use tokio::time::timeout;

        // Try up to 3 ephemeral ports in case the TOCTOU window bites.
        let mut broker_opt = None;
        let mut bind = None;
        for _ in 0..3 {
            let addr = claim_ephemeral_port();
            if let Some(b) = EmbeddedBroker::spawn(addr) {
                broker_opt = Some(b);
                bind = Some(addr);
                break;
            }
        }
        let broker = broker_opt.expect("embedded broker spawn (after retries)");
        let addr = bind.unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            // Wait for the broker's accept loop to be ready. `spawn_blocking`
            // returns before rumqttd's OS thread is listening, so we do a
            // short bounded connect-retry instead of a bare sleep.
            for _ in 0..40 {
                if std::net::TcpStream::connect_timeout(
                    &addr,
                    Duration::from_millis(50),
                )
                .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let mut sub_opts =
                MqttOptions::new("test-sub", addr.ip().to_string(), addr.port());
            sub_opts.set_keep_alive(Duration::from_secs(5));
            let (sub_client, mut sub_loop) = AsyncClient::new(sub_opts, 32);
            sub_client
                .subscribe("round_trip/#", QoS::AtLeastOnce)
                .await
                .expect("subscribe");

            // Drain the initial SubAck etc. so the poll loop is past
            // handshake before the publisher joins.
            for _ in 0..4 {
                let _ = timeout(Duration::from_millis(500), sub_loop.poll()).await;
            }

            let mut pub_opts =
                MqttOptions::new("test-pub", addr.ip().to_string(), addr.port());
            pub_opts.set_keep_alive(Duration::from_secs(5));
            let (pub_client, mut pub_loop) = AsyncClient::new(pub_opts, 32);
            // Service the publisher's event loop in the background — without
            // it `publish` queues but nothing flushes.
            tokio::spawn(async move {
                for _ in 0..50 {
                    let _ = timeout(Duration::from_millis(200), pub_loop.poll()).await;
                }
            });

            pub_client
                .publish(
                    "round_trip/hello",
                    QoS::AtLeastOnce,
                    true, // retained — verify the broker honors it
                    b"world".to_vec(),
                )
                .await
                .expect("publish");

            // Poll until we see the publish arrive. 5s cap — broker is
            // local, this runs well under 100ms on healthy hardware.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut saw = false;
            while tokio::time::Instant::now() < deadline && !saw {
                match timeout(Duration::from_millis(200), sub_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        if p.topic == "round_trip/hello" && &*p.payload == b"world" {
                            saw = true;
                        }
                    }
                    _ => continue,
                }
            }
            assert!(saw, "subscriber did not receive retained publish within 5s");

            // Fresh subscriber after the retained publish lands: it
            // should get the message without the publisher re-sending.
            let mut late_opts =
                MqttOptions::new("test-late", addr.ip().to_string(), addr.port());
            late_opts.set_keep_alive(Duration::from_secs(5));
            let (late_client, mut late_loop) = AsyncClient::new(late_opts, 32);
            late_client
                .subscribe("round_trip/#", QoS::AtLeastOnce)
                .await
                .unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut saw_retained = false;
            while tokio::time::Instant::now() < deadline && !saw_retained {
                match timeout(Duration::from_millis(200), late_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        if p.topic == "round_trip/hello" && p.retain {
                            saw_retained = true;
                        }
                    }
                    _ => continue,
                }
            }
            assert!(
                saw_retained,
                "late subscriber did not receive retained publish"
            );
        });

        // Broker handle is kept alive so we can log its bind on failure.
        drop(broker);
    }

    /// End-to-end: publish a Z2M `bridge/devices` array through the
    /// embedded broker, hand every received frame to `DialectRouter`,
    /// assert the Zigbee2Mqtt dialect surfaces the inventory as
    /// `DialectMessage::Discoveries` with the right device count.
    ///
    /// This is the smallest "real" driver path validation — proves the
    /// wire → router handoff works against a running broker without
    /// standing up the full `MqttSession` + DB chain.
    #[test]
    fn z2m_bridge_devices_parse_via_broker() {
        use crate::smart_home::drivers::mqtt::dialects::{DialectMessage, DialectRouter};
        use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
        use std::time::Duration;
        use tokio::time::timeout;

        let mut broker_opt = None;
        let mut bind = None;
        for _ in 0..3 {
            let addr = claim_ephemeral_port();
            if let Some(b) = EmbeddedBroker::spawn(addr) {
                broker_opt = Some(b);
                bind = Some(addr);
                break;
            }
        }
        let _broker = broker_opt.expect("embedded broker spawn (after retries)");
        let addr = bind.unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            for _ in 0..40 {
                if std::net::TcpStream::connect_timeout(
                    &addr,
                    Duration::from_millis(50),
                )
                .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let router = DialectRouter::v1();

            // Subscriber first — then publish retained inventory.
            let mut sub_opts =
                MqttOptions::new("z2m-sub", addr.ip().to_string(), addr.port());
            sub_opts.set_keep_alive(Duration::from_secs(5));
            let (sub_client, mut sub_loop) = AsyncClient::new(sub_opts, 128);
            for topic in router.subscribe_topics() {
                sub_client.subscribe(topic, QoS::AtMostOnce).await.ok();
            }
            for _ in 0..4 {
                let _ = timeout(Duration::from_millis(300), sub_loop.poll()).await;
            }

            let mut pub_opts =
                MqttOptions::new("z2m-pub", addr.ip().to_string(), addr.port());
            pub_opts.set_keep_alive(Duration::from_secs(5));
            let (pub_client, mut pub_loop) = AsyncClient::new(pub_opts, 32);
            tokio::spawn(async move {
                for _ in 0..50 {
                    let _ = timeout(Duration::from_millis(200), pub_loop.poll()).await;
                }
            });

            // A minimally-valid Z2M inventory: one coordinator (skipped),
            // one end-device light (surfaces), one end-device sensor
            // (surfaces).
            let payload = serde_json::json!([
                {
                    "ieee_address": "0x1111",
                    "type": "Coordinator",
                    "friendly_name": "Coordinator"
                },
                {
                    "ieee_address": "0x0001",
                    "type": "EndDevice",
                    "friendly_name": "living_room_light",
                    "manufacturer": "IKEA",
                    "definition": {
                        "exposes": [
                            {
                                "type": "light",
                                "features": [
                                    {"name": "state", "property": "state"},
                                    {"name": "brightness", "property": "brightness"}
                                ]
                            }
                        ]
                    }
                },
                {
                    "ieee_address": "0x0002",
                    "type": "EndDevice",
                    "friendly_name": "kitchen_motion",
                    "manufacturer": "Aqara",
                    "definition": {
                        "exposes": [
                            {"name": "occupancy", "property": "occupancy"}
                        ]
                    }
                }
            ])
            .to_string();
            pub_client
                .publish(
                    "zigbee2mqtt/bridge/devices",
                    QoS::AtLeastOnce,
                    true,
                    payload.into_bytes(),
                )
                .await
                .expect("publish");

            // Pull frames until we see the Z2M inventory or time out.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut got_inventory: Vec<_> = Vec::new();
            while tokio::time::Instant::now() < deadline && got_inventory.is_empty() {
                match timeout(Duration::from_millis(300), sub_loop.poll()).await {
                    Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                        if let Some(msg) = router.parse(&p.topic, &p.payload) {
                            if let DialectMessage::Discoveries(list) = msg {
                                got_inventory = list;
                            }
                        }
                    }
                    _ => continue,
                }
            }

            assert_eq!(
                got_inventory.len(),
                2,
                "expected 2 candidates (coordinator skipped), got {}: {:?}",
                got_inventory.len(),
                got_inventory
                    .iter()
                    .map(|c| &c.external_id)
                    .collect::<Vec<_>>()
            );
            let kinds: Vec<&str> = got_inventory.iter().map(|c| c.kind.as_str()).collect();
            assert!(kinds.contains(&"light"), "expected a light, got {:?}", kinds);
            assert!(
                kinds.contains(&"sensor_motion"),
                "expected sensor_motion, got {:?}",
                kinds
            );
        });
    }

    /// Full-stack roundtrip — the plan's Phase G-2 path, proving a real
    /// device flip lands on the module-wide event bus:
    ///
    /// 1. Temp DB with users + smart_home_credentials + smart_home_devices
    /// 2. Seed a Tasmota device row (driver=mqtt, external_id=tasmota_topic:test_plug)
    /// 3. Ephemeral EmbeddedBroker, credential pointing at it
    /// 4. MqttSupervisor::spawn(db_path) — starts one MqttSession
    /// 5. External rumqttc publisher sends `stat/test_plug/POWER ON`
    /// 6. Subscriber on events::bus() receives DeviceStateChanged within 5s,
    ///    source="tasmota", device_id matching the seeded row
    ///
    /// Caveats:
    ///   - events::bus() is a process-global OnceLock; other tests may
    ///     pollute it. We subscribe then drain anything already queued
    ///     before the publish, then match by device_id + source.
    ///   - Supervisor runs as a detached tokio task; test keeps the
    ///     runtime alive long enough for its connect → subscribe →
    ///     poll → StateCache.apply_state chain to complete.
    #[test]
    fn full_stack_session_emits_device_state_changed() {
        use crate::crypto;
        use crate::smart_home::credentials;
        use crate::smart_home::drivers::mqtt::MqttSupervisor;
        use crate::smart_home::events::{self, SmartHomeEvent};
        use rumqttc::{AsyncClient, MqttOptions, QoS};
        use std::time::Duration;
        use tempfile::TempDir;
        use tokio::time::timeout;

        // Broker first — we need its ephemeral port for the credential URL.
        let mut broker_opt = None;
        let mut bind = None;
        for _ in 0..3 {
            let addr = claim_ephemeral_port();
            if let Some(b) = EmbeddedBroker::spawn(addr) {
                broker_opt = Some(b);
                bind = Some(addr);
                break;
            }
        }
        let _broker = broker_opt.expect("embedded broker spawn (after retries)");
        let addr = bind.unwrap();

        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().to_path_buf();
        let key = crypto::load_or_create_key(&data_dir).expect("master key");
        let db_path = data_dir.join("index.db");

        // Minimal schema — only the tables the supervisor + state cache touch.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY);
                 INSERT INTO users (id) VALUES (1);
                 CREATE TABLE smart_home_devices (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    room_id INTEGER,
                    driver TEXT NOT NULL,
                    external_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    capabilities_json TEXT NOT NULL DEFAULT '{}',
                    state_json TEXT NOT NULL DEFAULT '{}',
                    metadata_json TEXT NOT NULL DEFAULT '{}',
                    last_seen_at INTEGER,
                    created_at INTEGER NOT NULL,
                    UNIQUE(user_id, driver, external_id)
                 );
                 CREATE TABLE smart_home_credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    provider TEXT NOT NULL,
                    label TEXT NOT NULL,
                    secret_encrypted TEXT NOT NULL,
                    metadata_json TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL
                 );
                 INSERT INTO smart_home_devices
                    (user_id, driver, external_id, name, kind, created_at)
                  VALUES
                    (1, 'mqtt', 'tasmota_topic:test_plug', 'Test Plug', 'switch', 0);",
            )
            .unwrap();

            let secret = serde_json::json!({
                "url": format!("mqtt://{}", addr)
            });
            credentials::upsert(&conn, &key, 1, "mqtt", "default", &secret, None)
                .expect("upsert credential");
        }

        let device_id: i64 = {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.query_row(
                "SELECT id FROM smart_home_devices WHERE external_id = 'tasmota_topic:test_plug'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            // Broker accept-loop readiness.
            for _ in 0..40 {
                if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50))
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            // Subscribe to the bus BEFORE the supervisor starts so we
            // don't race the first DeviceStateChanged emission.
            let mut rx = events::bus().subscribe();

            // Start the supervisor. Gives us a session connected to
            // the embedded broker, subscribed to every v1 dialect topic.
            let _sup = MqttSupervisor::spawn(db_path).await;

            // External publisher — not the supervisor — sends the
            // Tasmota POWER frame. Simulates the real wire exchange.
            let mut pub_opts =
                MqttOptions::new("ext-pub", addr.ip().to_string(), addr.port());
            pub_opts.set_keep_alive(Duration::from_secs(5));
            let (pub_client, mut pub_loop) = AsyncClient::new(pub_opts, 32);
            tokio::spawn(async move {
                for _ in 0..80 {
                    let _ = timeout(Duration::from_millis(250), pub_loop.poll()).await;
                }
            });

            // Under parallel cargo test load the session's subscribe
            // can land a few hundred ms late. Publish repeatedly with
            // a toggling payload so the StateCache hash-diff always
            // has something new to emit even if the first one or two
            // beat the subscription handshake. Gives the test enough
            // slack to pass with --test-threads=N without flaking.
            let toggles = ["ON", "OFF", "ON", "OFF", "ON"];
            let mut republisher_done = false;
            let republisher = async {
                for payload in toggles {
                    if republisher_done {
                        break;
                    }
                    let _ = pub_client
                        .publish(
                            "stat/test_plug/POWER",
                            QoS::AtLeastOnce,
                            false,
                            payload.as_bytes().to_vec(),
                        )
                        .await;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            };

            // Wait for DeviceStateChanged on the bus while the
            // republisher runs. Either side winning cleanly ends the
            // test; we join them with tokio::select so the republisher
            // gets a chance to retry on slow startup.
            let waiter = async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                while tokio::time::Instant::now() < deadline {
                    let remaining =
                        deadline.saturating_duration_since(tokio::time::Instant::now());
                    match timeout(remaining, rx.recv()).await {
                        Ok(Ok(SmartHomeEvent::DeviceStateChanged {
                            user_id,
                            device_id: id,
                            source,
                            state,
                        })) => {
                            if user_id == 1 && id == device_id && source == "tasmota" {
                                // Assert against "relays.POWER" being
                                // either ON or OFF — we don't know which
                                // republish frame the session caught first.
                                let got = state["relays"]["POWER"].as_str().unwrap_or("");
                                assert!(
                                    got == "ON" || got == "OFF",
                                    "unexpected POWER state: {}",
                                    state
                                );
                                return true;
                            }
                        }
                        Ok(Ok(_other)) => continue,
                        Ok(Err(_lag_or_closed)) => continue,
                        Err(_timeout) => break,
                    }
                }
                false
            };

            let (matched, _) = tokio::join!(waiter, republisher);
            republisher_done = true;
            let _ = republisher_done;
            assert!(
                matched,
                "did not observe DeviceStateChanged for device_id={} within 8s",
                device_id
            );
        });
    }
}
