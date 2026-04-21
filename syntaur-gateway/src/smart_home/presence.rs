//! Presence fusion — BLE proxies, phone GPS, voice utterances, door-lock
//! events, motion sensors. Outputs room-level signals with confidence
//! to `smart_home_presence_signals` and exposes a "who's in room X?"
//! query used by automation conditions.
//!
//! Week 7 stands up the BLE-proxy ingestor + Bermuda trilateration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceSnapshot {
    pub person: String,
    pub room_id: Option<i64>,
    pub confidence: f64,
    pub ts: i64,
}
