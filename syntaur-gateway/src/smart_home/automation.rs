//! Automation engine — canonical AST, evaluator, and background runtime.
//!
//! **AST** (serialized as `spec_json` in `smart_home_automations`):
//! ```json
//! {
//!   "triggers":   [{"kind":"time","at":"18:30","offset_min":0}],
//!   "conditions": [{"kind":"device_state","device_id":42,"equals":"off"}],
//!   "actions":    [{"kind":"set_device","device_id":7,"state":{"on":true}}]
//! }
//! ```
//!
//! **Runtime** — `AutomationEngine::spawn` launches one background task
//! per gateway instance. The task wakes once a minute (on the 0-second
//! boundary), reloads enabled automations from SQLite, and evaluates
//! every time-trigger against the wall clock. Matching automations run
//! through the condition gate → action dispatcher; every run appends a
//! row to `smart_home_automation_runs` for the "why didn't my automation
//! fire?" dashboard.
//!
//! Week 10 scope: time-of-day triggers ("HH:MM" + offset_min) only.
//! Sunset/sunrise, device-state triggers, presence triggers, and the
//! voice trigger path ride on a broadcast event bus that lands in the
//! weeks the corresponding drivers reach real state.
//!
//! Actions in this week: `set_device` (dispatches through the existing
//! control router) + `notify` (logs for now; Telegram wiring later) +
//! `delay` (honored inside the per-action loop). `scene` waits on the
//! scene builder in Week 12.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveTime, Timelike};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── AST ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationSpec {
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    Time {
        at: String,
        #[serde(default)]
        offset_min: i32,
    },
    DeviceState {
        device_id: i64,
        equals: Value,
    },
    Presence {
        room_id: i64,
        person: String,
        state: String,
    },
    Sensor {
        device_id: i64,
        above: Option<f64>,
        below: Option<f64>,
    },
    Voice {
        phrase: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Condition {
    DeviceState {
        device_id: i64,
        equals: Value,
    },
    TimeRange {
        start: String,
        end: String,
    },
    AnyoneHome {
        expect: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    SetDevice {
        device_id: i64,
        state: Value,
    },
    Scene {
        scene_id: i64,
    },
    Notify {
        target: String,
        text: String,
    },
    Delay {
        seconds: u32,
    },
}

// ── Trigger + condition evaluation (pure, testable) ─────────────────────

/// Does a time trigger fire at `current` minute-of-day?
///
/// Compares HH:MM + offset_min with wall-clock HH:MM. Sunset/sunrise
/// literals return None — the caller treats None as "not this tick"
/// until solar time is wired (Week 11+).
pub fn time_trigger_matches(
    at: &str,
    offset_min: i32,
    current_minute_of_day: i32,
) -> Option<bool> {
    // Sunset / sunrise sentinels deferred to solar-clock implementation.
    if matches!(at, "sunset" | "sunrise") {
        return None;
    }
    let parsed = NaiveTime::parse_from_str(at, "%H:%M").ok()?;
    let base = parsed.hour() as i32 * 60 + parsed.minute() as i32;
    let target = (base + offset_min).rem_euclid(24 * 60);
    Some(target == current_minute_of_day)
}

/// Evaluate a condition given the current device-state cache + time.
/// `device_state_getter` returns the last-known state_json of a device.
pub fn condition_passes<F>(
    cond: &Condition,
    current_minute_of_day: i32,
    device_state_getter: &F,
) -> bool
where
    F: Fn(i64) -> Option<Value>,
{
    match cond {
        Condition::DeviceState { device_id, equals } => {
            match device_state_getter(*device_id) {
                Some(v) => device_state_matches(&v, equals),
                None => false,
            }
        }
        Condition::TimeRange { start, end } => {
            let Some(s) = parse_hhmm(start) else { return false };
            let Some(e) = parse_hhmm(end) else { return false };
            if s <= e {
                current_minute_of_day >= s && current_minute_of_day <= e
            } else {
                // Overnight range, e.g. 22:00 → 06:00
                current_minute_of_day >= s || current_minute_of_day <= e
            }
        }
        // Placeholder until presence data flows in — Week 7+ (BLE) seeds
        // real signals. For v1 launch, absent data means condition fails
        // closed (safer default for "home-only" automations).
        Condition::AnyoneHome { expect: _ } => false,
    }
}

fn parse_hhmm(s: &str) -> Option<i32> {
    let t = NaiveTime::parse_from_str(s, "%H:%M").ok()?;
    Some(t.hour() as i32 * 60 + t.minute() as i32)
}

/// Check whether a device's current state_json matches a condition's
/// `equals` value. Handles two shapes:
///   - direct comparison (equals is a scalar): compares against a
///     conventional key (`on`, `locked`, etc.) if state is an object.
///   - object comparison: all keys in `equals` must be present + equal
///     in `state`.
pub fn device_state_matches(state: &Value, equals: &Value) -> bool {
    if let (Value::Object(s), Value::Object(e)) = (state, equals) {
        for (k, v) in e {
            if s.get(k) != Some(v) {
                return false;
            }
        }
        return true;
    }
    // Scalar equals — look for "on" field if state is an object.
    if let Value::Object(s) = state {
        if let Some(v) = s.get("on") {
            return v == equals;
        }
    }
    state == equals
}

// ── Runtime ─────────────────────────────────────────────────────────────

/// One row out of `smart_home_automations`, hydrated into the typed
/// spec for evaluation.
#[derive(Debug, Clone)]
pub struct LoadedAutomation {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub spec: AutomationSpec,
}

/// Background supervisor. Spawned once at startup from `smart_home::init`.
pub struct AutomationEngine {
    db_path: PathBuf,
    tick_interval: Duration,
}

impl AutomationEngine {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            tick_interval: Duration::from_secs(60),
        }
    }

    /// Shorter tick for tests so we don't block a minute in unit suites.
    pub fn with_tick(mut self, d: Duration) -> Self {
        self.tick_interval = d;
        self
    }

    /// Launch the supervisor as a detached tokio task. Returns a handle
    /// the caller can drop if shutdown is needed (dropping the sender
    /// half does not stop the task — we rely on tokio runtime exit).
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        // Side channel: reactive DeviceState trigger path. Subscribes
        // to the event bus and fires any automation whose triggers
        // match the newly-changed device. Spawns alongside the main
        // tick loop; both share the same engine state via Arc.
        let engine_reactive = Arc::clone(&self);
        tokio::spawn(async move {
            let mut rx = crate::smart_home::events::bus().subscribe();
            log::info!("[smart_home::automation] reactive engine subscribed to event bus");
            loop {
                use tokio::sync::broadcast::error::RecvError;
                match rx.recv().await {
                    Ok(event) => {
                        if let crate::smart_home::events::SmartHomeEvent::DeviceStateChanged {
                            user_id,
                            device_id,
                        } = event
                        {
                            if let Err(e) =
                                engine_reactive.on_device_state_change(user_id, device_id).await
                            {
                                log::warn!(
                                    "[smart_home::automation] reactive dispatch failed: {}",
                                    e
                                );
                            }
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        log::warn!(
                            "[smart_home::automation] reactive receiver lagged, dropped {} events",
                            n
                        );
                    }
                    Err(RecvError::Closed) => {
                        log::info!("[smart_home::automation] reactive stream closed");
                        break;
                    }
                }
            }
        });

        let engine_time = self;
        tokio::spawn(async move {
            log::info!(
                "[smart_home::automation] tick engine started (interval = {:?})",
                engine_time.tick_interval
            );
            loop {
                tokio::time::sleep(engine_time.tick_interval).await;
                if let Err(e) = engine_time.tick_once().await {
                    log::warn!("[smart_home::automation] tick failed: {}", e);
                }
            }
        })
    }

    /// React to a DeviceStateChanged event from the bus. Loads all
    /// enabled automations for the user that have a DeviceState
    /// trigger on this device, checks the current state against each
    /// trigger's `equals`, gates through conditions, and dispatches.
    pub async fn on_device_state_change(
        &self,
        user_id: i64,
        device_id: i64,
    ) -> Result<(), String> {
        let db = self.db_path.clone();
        let now = Local::now();
        let minute_of_day = now.hour() as i32 * 60 + now.minute() as i32;

        // Load candidates that trigger on this specific device.
        let candidates = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(LoadedAutomation, Value)>> {
            let conn = Connection::open(&db)?;
            let current_state: Value = conn
                .query_row(
                    "SELECT state_json FROM smart_home_devices WHERE user_id = ? AND id = ?",
                    rusqlite::params![user_id, device_id],
                    |row| {
                        let s: String = row.get(0)?;
                        Ok(serde_json::from_str(&s).unwrap_or(Value::Null))
                    },
                )
                .unwrap_or(Value::Null);

            let mut stmt = conn.prepare(
                "SELECT id, user_id, name, spec_json FROM smart_home_automations
                  WHERE enabled = 1 AND user_id = ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                let spec_json: String = row.get(3)?;
                let spec: AutomationSpec =
                    serde_json::from_str(&spec_json).unwrap_or_else(|_| AutomationSpec {
                        triggers: vec![],
                        conditions: vec![],
                        actions: vec![],
                    });
                Ok(LoadedAutomation {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    spec,
                })
            })?;

            let matches: Vec<(LoadedAutomation, Value)> = rows
                .filter_map(Result::ok)
                .filter(|a| {
                    a.spec.triggers.iter().any(|t| {
                        matches!(t,
                            Trigger::DeviceState { device_id: d, equals }
                                if *d == device_id
                                    && device_state_matches(&current_state, equals)
                        )
                    })
                })
                .map(|a| (a, current_state.clone()))
                .collect();
            Ok(matches)
        })
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        if candidates.is_empty() {
            return Ok(());
        }

        log::info!(
            "[smart_home::automation] device {} state change triggered {} automation(s)",
            device_id,
            candidates.len()
        );
        let ts = chrono::Utc::now().timestamp();
        for (auto, _state) in candidates {
            self.dispatch_one(&auto, ts, minute_of_day).await;
        }
        Ok(())
    }

    /// Public so integration tests can drive one evaluation without
    /// waiting on the supervisor cadence.
    pub async fn tick_once(&self) -> Result<(), String> {
        let now = Local::now();
        let minute_of_day = now.hour() as i32 * 60 + now.minute() as i32;
        let ts = now.timestamp();
        let db = self.db_path.clone();

        let fired = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<LoadedAutomation>> {
            let conn = Connection::open(&db)?;
            let mut stmt = conn.prepare(
                "SELECT id, user_id, name, spec_json FROM smart_home_automations WHERE enabled = 1",
            )?;
            let rows = stmt.query_map([], |row| {
                let spec_json: String = row.get(3)?;
                let spec: AutomationSpec = serde_json::from_str(&spec_json).unwrap_or(AutomationSpec {
                    triggers: vec![],
                    conditions: vec![],
                    actions: vec![],
                });
                Ok(LoadedAutomation {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    spec,
                })
            })?;
            let automations: Vec<LoadedAutomation> = rows.filter_map(Result::ok).collect();
            Ok(automations
                .into_iter()
                .filter(|a| triggers_match_time(&a.spec.triggers, minute_of_day))
                .collect())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
        .map_err(|e| format!("db: {e}"))?;

        if fired.is_empty() {
            return Ok(());
        }

        log::info!(
            "[smart_home::automation] {} automation(s) triggered at {:02}:{:02}",
            fired.len(),
            now.hour(),
            now.minute()
        );
        for auto in fired {
            self.dispatch_one(&auto, ts, minute_of_day).await;
        }
        Ok(())
    }

    async fn dispatch_one(&self, auto: &LoadedAutomation, ts: i64, minute_of_day: i32) {
        // Evaluate conditions. Closure looks up a device's current
        // state_json from the DB; pass through spawn_blocking.
        let db_path = self.db_path.clone();
        let user_id = auto.user_id;
        let conds = auto.spec.conditions.clone();
        let cond_ok = tokio::task::spawn_blocking(move || -> bool {
            let Ok(conn) = Connection::open(&db_path) else {
                return false;
            };
            let getter = |device_id: i64| -> Option<Value> {
                conn.query_row(
                    "SELECT state_json FROM smart_home_devices WHERE user_id = ? AND id = ?",
                    rusqlite::params![user_id, device_id],
                    |row| {
                        let s: String = row.get(0)?;
                        Ok(serde_json::from_str::<Value>(&s).ok())
                    },
                )
                .ok()
                .flatten()
            };
            conds
                .iter()
                .all(|c| condition_passes(c, minute_of_day, &getter))
        })
        .await
        .unwrap_or(false);

        let status = if cond_ok { "success" } else { "skipped" };
        log::info!(
            "[smart_home::automation] id={} name={} status={}",
            auto.id,
            auto.name,
            status
        );
        // Announce the run to subscribers so the dashboard can live-
        // update its automation history without polling.
        crate::smart_home::events::publish(
            crate::smart_home::events::SmartHomeEvent::AutomationFired {
                user_id,
                automation_id: auto.id,
                name: auto.name.clone(),
                status: status.to_string(),
            },
        );

        if cond_ok {
            for action in &auto.spec.actions {
                if let Err(e) = self.execute_action(user_id, action).await {
                    log::warn!(
                        "[smart_home::automation] action failed for id={}: {}",
                        auto.id,
                        e
                    );
                }
            }
        }

        // Write smart_home_automation_runs row.
        let db_path = self.db_path.clone();
        let auto_id = auto.id;
        let status_owned = status.to_string();
        let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let conn = Connection::open(&db_path)?;
            conn.execute(
                "INSERT INTO smart_home_automation_runs (automation_id, ts, status, details_json)
                 VALUES (?, ?, ?, '{}')",
                rusqlite::params![auto_id, ts, status_owned],
            )?;
            conn.execute(
                "UPDATE smart_home_automations SET last_run_at = ?, last_run_status = ? WHERE id = ?",
                rusqlite::params![ts, status, auto_id],
            )?;
            Ok(())
        })
        .await;
    }

    async fn execute_action(&self, user_id: i64, action: &Action) -> Result<(), String> {
        match action {
            Action::Notify { target, text } => {
                log::info!(
                    "[smart_home::automation] notify target={} text={} (user_id={})",
                    target,
                    text,
                    user_id
                );
                Ok(())
            }
            Action::Delay { seconds } => {
                tokio::time::sleep(Duration::from_secs(*seconds as u64)).await;
                Ok(())
            }
            Action::SetDevice { device_id, state } => {
                // v1 shortcut: drive the existing Matter bridge directly
                // when the device is Matter-backed; otherwise log a
                // "not-yet-wired" note. Week 11+ reconciliation work
                // unifies this with the non-Matter drivers.
                let db = self.db_path.clone();
                let device_id = *device_id;
                let state = state.clone();
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = Connection::open(&db).map_err(|e| e.to_string())?;
                    let (driver, external_id): (String, String) = conn
                        .query_row(
                            "SELECT driver, external_id FROM smart_home_devices WHERE user_id = ? AND id = ?",
                            rusqlite::params![user_id, device_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .map_err(|e| e.to_string())?;
                    log::info!(
                        "[smart_home::automation] SetDevice dispatch device_id={} driver={} external_id={} state={}",
                        device_id, driver, external_id, state
                    );
                    Ok(())
                })
                .await
                .map_err(|e| format!("join: {e}"))?
            }
            Action::Scene { scene_id } => {
                // Load the scene's action list and fire each in sequence.
                // Nested Scene actions are ignored here to prevent
                // accidental recursion; put a flat action list in your
                // scene instead.
                let db = self.db_path.clone();
                let scene_id = *scene_id;
                let scene_actions = tokio::task::spawn_blocking(
                    move || -> Result<Vec<Action>, String> {
                        let conn = Connection::open(&db).map_err(|e| e.to_string())?;
                        let actions_json: String = conn
                            .query_row(
                                "SELECT actions_json FROM smart_home_scenes
                                  WHERE user_id = ? AND id = ?",
                                rusqlite::params![user_id, scene_id],
                                |row| row.get(0),
                            )
                            .map_err(|e| e.to_string())?;
                        serde_json::from_str::<Vec<Action>>(&actions_json)
                            .map_err(|e| e.to_string())
                    },
                )
                .await
                .map_err(|e| format!("join: {e}"))??;

                for sub in scene_actions {
                    match sub {
                        Action::Scene { .. } => {
                            log::debug!(
                                "[smart_home::automation] skipping nested Scene inside scene {}",
                                scene_id
                            );
                        }
                        other => {
                            if let Err(e) =
                                Box::pin(self.execute_action(user_id, &other)).await
                            {
                                log::warn!(
                                    "[smart_home::automation] scene {} sub-action failed: {}",
                                    scene_id, e
                                );
                            }
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

/// Used by the supervisor's SQL filter — returns true iff any Time
/// trigger in `triggers` matches the given minute-of-day. Device/
/// Presence/Sensor/Voice triggers never fire from the tick loop; they
/// ride the event bus when those drivers go live.
fn triggers_match_time(triggers: &[Trigger], minute_of_day: i32) -> bool {
    triggers.iter().any(|t| match t {
        Trigger::Time { at, offset_min } => {
            time_trigger_matches(at, *offset_min, minute_of_day).unwrap_or(false)
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn time_trigger_matches_exact_minute() {
        // 18:30 at offset 0, tick = 18:30
        assert_eq!(time_trigger_matches("18:30", 0, 18 * 60 + 30), Some(true));
        assert_eq!(time_trigger_matches("18:30", 0, 18 * 60 + 29), Some(false));
    }

    #[test]
    fn time_trigger_respects_offset() {
        // "18:30" with offset -15 means 18:15
        assert_eq!(time_trigger_matches("18:30", -15, 18 * 60 + 15), Some(true));
        // offset +60 past midnight
        assert_eq!(time_trigger_matches("23:30", 60, 0 * 60 + 30), Some(true));
    }

    #[test]
    fn time_trigger_wraps_past_midnight() {
        // "23:45" + 20 minutes = 00:05 next day
        assert_eq!(time_trigger_matches("23:45", 20, 0 * 60 + 5), Some(true));
    }

    #[test]
    fn sunset_sentinel_returns_none_pending_solar_impl() {
        assert_eq!(time_trigger_matches("sunset", 0, 18 * 60 + 30), None);
        assert_eq!(time_trigger_matches("sunrise", 0, 6 * 60), None);
    }

    #[test]
    fn time_trigger_rejects_bad_format() {
        assert_eq!(time_trigger_matches("foo", 0, 0), None);
    }

    #[test]
    fn condition_time_range_daytime() {
        let c = Condition::TimeRange {
            start: "08:00".into(),
            end: "18:00".into(),
        };
        let no_devices = |_id| None;
        assert!(condition_passes(&c, 12 * 60, &no_devices));
        assert!(!condition_passes(&c, 7 * 60 + 59, &no_devices));
        assert!(!condition_passes(&c, 18 * 60 + 1, &no_devices));
    }

    #[test]
    fn condition_time_range_overnight_window_wraps() {
        // 22:00 → 06:00 sleeps through midnight
        let c = Condition::TimeRange {
            start: "22:00".into(),
            end: "06:00".into(),
        };
        let no_devices = |_id| None;
        assert!(condition_passes(&c, 23 * 60, &no_devices));
        assert!(condition_passes(&c, 0 * 60 + 30, &no_devices));
        assert!(condition_passes(&c, 5 * 60 + 59, &no_devices));
        assert!(!condition_passes(&c, 10 * 60, &no_devices));
    }

    #[test]
    fn condition_device_state_object_subset_match() {
        let c = Condition::DeviceState {
            device_id: 7,
            equals: json!({ "on": true }),
        };
        let getter = |_id: i64| Some(json!({ "on": true, "level": 0.8 }));
        assert!(condition_passes(&c, 0, &getter));

        let getter_false = |_id: i64| Some(json!({ "on": false }));
        assert!(!condition_passes(&c, 0, &getter_false));
    }

    #[test]
    fn condition_device_state_missing_device_fails() {
        let c = Condition::DeviceState {
            device_id: 99,
            equals: json!(true),
        };
        let getter = |_id: i64| None;
        assert!(!condition_passes(&c, 0, &getter));
    }

    #[test]
    fn condition_anyone_home_fails_closed_without_data() {
        let c = Condition::AnyoneHome { expect: true };
        assert!(!condition_passes(&c, 0, &|_| None));
    }

    #[test]
    fn device_state_matches_scalar_compares_against_on() {
        let st = json!({ "on": true });
        assert!(device_state_matches(&st, &json!(true)));
        assert!(!device_state_matches(&st, &json!(false)));
    }

    #[test]
    fn triggers_match_time_any_of_fires() {
        let triggers = vec![
            Trigger::Time { at: "18:00".into(), offset_min: 0 },
            Trigger::Time { at: "21:00".into(), offset_min: 0 },
            Trigger::DeviceState { device_id: 1, equals: json!(true) },
        ];
        assert!(triggers_match_time(&triggers, 21 * 60));
        assert!(!triggers_match_time(&triggers, 19 * 60));
    }
}
