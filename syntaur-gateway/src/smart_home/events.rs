//! Cross-driver event bus.
//!
//! A single `tokio::sync::broadcast` channel that every smart-home
//! subsystem can publish to and every subscriber (automation engine's
//! DeviceState trigger path, the dashboard's SSE stream, future
//! external webhooks) can read from. Kept intentionally narrow —
//! events carry just enough for the listener to decide whether to
//! refresh something; the ground truth still lives in SQLite.
//!
//! Initialized once by `smart_home::init` via a `OnceLock` so publishers
//! don't have to plumb a handle through every driver constructor.
//!
//! Dropping a slow subscriber: broadcast lags mark the receiver as
//! lagged and drop the oldest N events; we tune `CHANNEL_CAPACITY`
//! high enough that a dashboard tab that stutters for a few seconds
//! doesn't lose signal.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 256;

static BUS: OnceLock<EventBus> = OnceLock::new();

/// Every kind of event currently published. `kind` field on the
/// serialized form is kebab-cased for JS consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "kebab-case")]
pub enum SmartHomeEvent {
    /// An automation ran. `status` = "success" / "skipped" / "failed".
    AutomationFired {
        user_id: i64,
        automation_id: i64,
        name: String,
        status: String,
    },
    /// A device transitioned (e.g. offline → online). `kind` here is the
    /// `smart_home_network_events.kind` column value.
    NetworkTransition {
        user_id: i64,
        subject: String,
        kind: String,
    },
    /// An energy sample was stored. Dashboards use this to refresh
    /// summaries without polling.
    EnergySample {
        user_id: i64,
        device_id: i64,
        watts: f64,
    },
    /// Scene was activated. `failed` counts sub-actions that reported
    /// an error.
    SceneActivated {
        user_id: i64,
        scene_id: i64,
        failed: usize,
    },
    /// Device state changed. Published when a control call succeeds,
    /// a driver subscription delivers fresh state, or a periodic poll
    /// finds new values. `state` carries the new state_json for
    /// dashboards that want to render without a DB round-trip; `source`
    /// identifies the driver ("mqtt", "matter", "matter-direct",
    /// "wifi_lan", "zwave", etc.) so subscribers can filter.
    ///
    /// Shape was locked 2026-04-21 via the MQTT/Matter parallel-session
    /// handoff (see vault/daily/2026-04-21.md and plan §6 at
    /// ~/.claude/plans/i-want-you-to-fluttering-forest.md). Adding
    /// fields is a breaking change — coordinate via claude-coord.
    DeviceStateChanged {
        user_id: i64,
        device_id: i64,
        state: serde_json::Value,
        source: String,
    },
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<SmartHomeEvent>,
}

impl EventBus {
    fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Subscribe to every event published from now on. Missed events
    /// (slow consumer) surface as `broadcast::error::RecvError::Lagged`.
    pub fn subscribe(&self) -> broadcast::Receiver<SmartHomeEvent> {
        self.tx.subscribe()
    }

    /// Fire-and-forget publish. Never blocks — if nobody is subscribed
    /// we silently discard (same as any broadcast with zero active
    /// receivers). Returns the number of receivers that got the event
    /// for observability.
    pub fn publish(&self, event: SmartHomeEvent) -> usize {
        self.tx.send(event).unwrap_or(0)
    }
}

/// Grab the process-wide bus, initializing lazily so tests that don't
/// go through `smart_home::init` can still publish + subscribe.
pub fn bus() -> &'static EventBus {
    BUS.get_or_init(EventBus::new)
}

/// Convenience — shorthand for `bus().publish(event)`. Publishers
/// use this heavily, so the short form reads better at call sites.
pub fn publish(event: SmartHomeEvent) -> usize {
    bus().publish(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_subscribers() {
        let bus = bus();
        let mut rx = bus.subscribe();
        let n = bus.publish(SmartHomeEvent::AutomationFired {
            user_id: 1,
            automation_id: 7,
            name: "Sunset porch".into(),
            status: "success".into(),
        });
        assert!(n >= 1);
        let got = rx.recv().await.expect("receive");
        match got {
            SmartHomeEvent::AutomationFired {
                automation_id,
                name,
                ..
            } => {
                assert_eq!(automation_id, 7);
                assert_eq!(name, "Sunset porch");
            }
            other => panic!("expected AutomationFired, got {:?}", other),
        }
    }

    #[test]
    fn events_serialize_with_kebab_kind() {
        let ev = SmartHomeEvent::EnergySample {
            user_id: 1,
            device_id: 42,
            watts: 123.5,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"kind\":\"energy-sample\""));
        assert!(s.contains("\"watts\":123.5"));
    }

    #[tokio::test]
    async fn publish_without_subscribers_returns_zero() {
        // Use a fresh local bus so other tests' subscribers don't skew
        // this count.
        let local = EventBus::new();
        let n = local.publish(SmartHomeEvent::SceneActivated {
            user_id: 1,
            scene_id: 1,
            failed: 0,
        });
        assert_eq!(n, 0);
    }
}
