//! Privacy event stream.
//!
//! Every blocked DNS query, every per-device mode change, every
//! anomaly the registry detects becomes an `Event` on the broadcast
//! channel. Subscribers (Telegram alert hook, audit logger, UI live
//! feed) consume independently.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A device tried to look up a domain in the cloud-blocklist.
    /// Emitted by `dns_sinkhole` after returning NXDOMAIN.
    DnsBlocked {
        ts: SystemTime,
        client_ip: String,
        /// Resolved device id when the client_ip is recognized in the
        /// smart_home_devices table; None when the source is unknown
        /// (e.g. a guest device that hasn't been onboarded yet).
        device_id: Option<String>,
        query_name: String,
        vendor_id: Option<String>,
    },
    /// A device tried a domain that resolved normally — mostly we
    /// don't emit these (would flood the channel), but we DO emit
    /// when the device has a policy with `extra_blocked_domains`
    /// and a query just outside its allowlist arrived. Useful as
    /// an anomaly hint.
    DnsSuspicious {
        ts: SystemTime,
        client_ip: String,
        query_name: String,
        device_id: Option<String>,
    },
    /// A device's policy mode was changed (UI action or autopilot).
    PolicyChanged {
        ts: SystemTime,
        device_id: String,
        old_mode: crate::policy::Mode,
        new_mode: crate::policy::Mode,
        actor: Actor,
    },
    /// An ephemeral "allow this device for N minutes" window opened.
    TemporaryAllow {
        ts: SystemTime,
        device_id: String,
        until: SystemTime,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User { user_id: String },
    Autopilot,
    System,
}

/// Broadcasts privacy events to all interested subscribers.
///
/// Construction: pick a buffer size based on expected event rate.
/// Default 1024 leaves room for bursty DNS traffic without a slow
/// subscriber dropping events. `subscribe()` returns a `Receiver`
/// that can be passed to a Telegram alerter, the gateway HTTP
/// /api/privacy/events SSE handler, etc.
///
/// **Subscriber lag**: tokio's broadcast channel returns
/// `RecvError::Lagged(n)` when a subscriber has fallen >n events
/// behind. Subscribers must handle this — the typical pattern is
/// to log the lag count, drop the missed events, and continue.
/// The Telegram alerter in particular should NOT page on lag (a
/// brief lag during a DNS storm is not a user-facing event).
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Construct a new bus. `buffer` must be >= 1 (tokio's broadcast
    /// channel panics on capacity 0); we clamp upward so a misconfig
    /// doesn't take the whole process down.
    pub fn new(buffer: usize) -> Self {
        let buffer = buffer.max(1);
        let (tx, _rx) = broadcast::channel(buffer);
        Self { tx }
    }

    pub fn emit(&self, event: Event) {
        // A `send` failure here just means no subscribers — that's
        // fine, the event is dropped on the floor.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Mode;

    #[tokio::test]
    async fn emit_and_receive() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        bus.emit(Event::DnsBlocked {
            ts: SystemTime::now(),
            client_ip: "192.168.20.40".into(),
            device_id: Some("meross-plug-1".into()),
            query_name: "iot.meross.com".into(),
            vendor_id: Some("meross".into()),
        });
        let got = rx.recv().await.expect("event");
        match got {
            Event::DnsBlocked { query_name, .. } => assert_eq!(query_name, "iot.meross.com"),
            _ => panic!("wrong event variant"),
        }
    }

    #[tokio::test]
    async fn no_subscribers_doesnt_panic() {
        let bus = EventBus::new(8);
        // Should silently drop.
        bus.emit(Event::PolicyChanged {
            ts: SystemTime::now(),
            device_id: "dev-1".into(),
            old_mode: Mode::Open,
            new_mode: Mode::Privacy,
            actor: Actor::Autopilot,
        });
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let bus = EventBus::new(8);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.emit(Event::DnsBlocked {
            ts: SystemTime::now(),
            client_ip: "192.168.20.40".into(),
            device_id: Some("meross-plug-1".into()),
            query_name: "iot.meross.com".into(),
            vendor_id: Some("meross".into()),
        });
        assert!(a.recv().await.is_ok());
        assert!(b.recv().await.is_ok());
    }
}
