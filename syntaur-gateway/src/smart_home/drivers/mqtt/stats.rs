//! Per-supervisor + per-session + per-dialect observability counters.
//!
//! Exposed via [`crate::smart_home::drivers::mqtt::MqttSupervisor::stats_snapshot`]
//! so `/api/smart-home/diagnostics` can render "how many messages has
//! this broker chewed through, how often did the session reconnect,
//! is the state-diff layer actually suppressing anything?"
//!
//! All counters are `AtomicU64` + `AtomicI64` to keep the hot path
//! lock-free. Per-dialect counts live under an `RwLock<HashMap>` — the
//! map's cardinality is small (≤ number of registered dialects) and
//! only write-locks when we see a dialect for the first time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Process-wide stats, shared across every `MqttSession` +
/// `StateCache` + `MqttSupervisor`. Cheap to clone (single Arc).
#[derive(Clone, Default)]
pub struct SupervisorStats {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// One slot per live session. Tracked by `user_id::label` composite
    /// key so restarting a session (same user+label) reuses its slot.
    sessions: RwLock<HashMap<String, Arc<SessionCounters>>>,
    /// `DeviceStateUpdate`s observed by the state cache — before the
    /// hash-diff check.
    state_updates_received: AtomicU64,
    /// State updates that actually emitted a `DeviceStateChanged`
    /// (passed the hash-diff).
    state_diffs_emitted: AtomicU64,
    /// `Availability` messages observed.
    availability_updates_received: AtomicU64,
    /// Availability flips that emitted a `DeviceStateChanged`.
    availability_transitions_emitted: AtomicU64,
    /// Total BridgeEvent frames observed (currently logged, no bus
    /// emit). Counted so we can spot Z2M `bridge/event` floods.
    bridge_events_observed: AtomicU64,
}

/// Counter block for one session. Public so the supervisor can hand
/// out `Arc<SessionCounters>` refs to each `MqttSession`.
#[derive(Default)]
pub struct SessionCounters {
    pub connected_since: AtomicI64,
    pub reconnects_total: AtomicU64,
    pub last_reconnect_at: AtomicI64,
    pub messages_in_total: AtomicU64,
    pub messages_out_total: AtomicU64,
    pub messages_by_dialect: RwLock<HashMap<String, u64>>,
}

impl SessionCounters {
    pub fn mark_connected(&self, now: i64) {
        self.connected_since.store(now, Ordering::Relaxed);
    }

    pub fn mark_reconnect(&self, now: i64) {
        self.reconnects_total.fetch_add(1, Ordering::Relaxed);
        self.last_reconnect_at.store(now, Ordering::Relaxed);
    }

    pub fn bump_in(&self) {
        self.messages_in_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn bump_out(&self) {
        self.messages_out_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Bump the per-dialect in-counter. Write-locks the inner map
    /// only — rare contention on a household broker.
    pub async fn bump_dialect(&self, dialect_id: &str) {
        let mut map = self.messages_by_dialect.write().await;
        *map.entry(dialect_id.to_string()).or_insert(0) += 1;
    }
}

impl SupervisorStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or retrieve) the counter block for a session. Two
    /// calls with the same key return the same Arc so reconnects
    /// keep their history.
    pub async fn session(&self, user_id: i64, label: &str) -> Arc<SessionCounters> {
        let key = format!("{}::{}", user_id, label);
        {
            let map = self.inner.sessions.read().await;
            if let Some(c) = map.get(&key) {
                return c.clone();
            }
        }
        let mut map = self.inner.sessions.write().await;
        map.entry(key)
            .or_insert_with(|| Arc::new(SessionCounters::default()))
            .clone()
    }

    pub fn note_state_update(&self) {
        self.inner
            .state_updates_received
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_state_emitted(&self) {
        self.inner
            .state_diffs_emitted
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_availability_update(&self) {
        self.inner
            .availability_updates_received
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_availability_emitted(&self) {
        self.inner
            .availability_transitions_emitted
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_bridge_event(&self) {
        self.inner
            .bridge_events_observed
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Produce an owned, serialisable snapshot. Safe to publish to the
    /// diagnostics HTTP handler.
    pub async fn snapshot(&self) -> StatsSnapshot {
        let sessions_map = self.inner.sessions.read().await;
        let mut sessions = Vec::with_capacity(sessions_map.len());
        for (key, c) in sessions_map.iter() {
            let by_dialect = c.messages_by_dialect.read().await.clone();
            let (user_id_s, label) = key.split_once("::").unwrap_or(("?", key.as_str()));
            sessions.push(SessionSnapshot {
                user_id: user_id_s.parse().unwrap_or(0),
                label: label.to_string(),
                connected_since: opt_ts(c.connected_since.load(Ordering::Relaxed)),
                reconnects_total: c.reconnects_total.load(Ordering::Relaxed),
                last_reconnect_at: opt_ts(c.last_reconnect_at.load(Ordering::Relaxed)),
                messages_in_total: c.messages_in_total.load(Ordering::Relaxed),
                messages_out_total: c.messages_out_total.load(Ordering::Relaxed),
                messages_by_dialect: by_dialect,
            });
        }
        sessions.sort_by(|a, b| (a.user_id, a.label.clone()).cmp(&(b.user_id, b.label.clone())));
        StatsSnapshot {
            sessions,
            state_updates_received: self
                .inner
                .state_updates_received
                .load(Ordering::Relaxed),
            state_diffs_emitted: self
                .inner
                .state_diffs_emitted
                .load(Ordering::Relaxed),
            availability_updates_received: self
                .inner
                .availability_updates_received
                .load(Ordering::Relaxed),
            availability_transitions_emitted: self
                .inner
                .availability_transitions_emitted
                .load(Ordering::Relaxed),
            bridge_events_observed: self
                .inner
                .bridge_events_observed
                .load(Ordering::Relaxed),
        }
    }
}

fn opt_ts(raw: i64) -> Option<i64> {
    if raw == 0 {
        None
    } else {
        Some(raw)
    }
}

/// JSON-serialisable snapshot for the diagnostics surface.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatsSnapshot {
    pub sessions: Vec<SessionSnapshot>,
    pub state_updates_received: u64,
    pub state_diffs_emitted: u64,
    pub availability_updates_received: u64,
    pub availability_transitions_emitted: u64,
    pub bridge_events_observed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionSnapshot {
    pub user_id: i64,
    pub label: String,
    pub connected_since: Option<i64>,
    pub reconnects_total: u64,
    pub last_reconnect_at: Option<i64>,
    pub messages_in_total: u64,
    pub messages_out_total: u64,
    pub messages_by_dialect: HashMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_counters_share_across_lookup() {
        let s = SupervisorStats::new();
        let a = s.session(1, "default").await;
        let b = s.session(1, "default").await;
        a.bump_in();
        b.bump_in();
        assert_eq!(
            a.messages_in_total.load(Ordering::Relaxed),
            2,
            "both refs point at the same counter"
        );
    }

    #[tokio::test]
    async fn per_dialect_counters_accumulate() {
        let s = SupervisorStats::new();
        let c = s.session(1, "default").await;
        c.bump_dialect("z2m").await;
        c.bump_dialect("z2m").await;
        c.bump_dialect("tasmota").await;
        let snap = s.snapshot().await;
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].messages_by_dialect["z2m"], 2);
        assert_eq!(snap.sessions[0].messages_by_dialect["tasmota"], 1);
    }

    #[tokio::test]
    async fn snapshot_serializes_round_trip() {
        let s = SupervisorStats::new();
        s.note_state_update();
        s.note_state_emitted();
        s.note_availability_update();
        s.note_bridge_event();
        let _ = s.session(2, "garage").await;
        let snap = s.snapshot().await;
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"state_updates_received\":1"));
        assert!(json.contains("\"bridge_events_observed\":1"));
        assert!(json.contains("\"label\":\"garage\""));
        let decoded: StatsSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.state_updates_received, 1);
    }

    #[tokio::test]
    async fn unseen_session_yields_empty_snapshot() {
        let s = SupervisorStats::new();
        let snap = s.snapshot().await;
        assert!(snap.sessions.is_empty());
        assert_eq!(snap.state_updates_received, 0);
    }

    #[tokio::test]
    async fn reconnect_marker_sets_fields() {
        let s = SupervisorStats::new();
        let c = s.session(1, "x").await;
        c.mark_reconnect(1000);
        c.mark_reconnect(2000);
        assert_eq!(c.reconnects_total.load(Ordering::Relaxed), 2);
        assert_eq!(c.last_reconnect_at.load(Ordering::Relaxed), 2000);
    }
}
