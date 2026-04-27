//! Natural-language → automation-AST compiler (Week 7 deliverable).
//!
//! User types English at `/api/smart-home/automation/compile`; we ask
//! Claude Opus (via OpenRouter, openrouter key from syntaur-vault) to
//! produce a canonical `AutomationSpec` AST. Server validates every
//! device_id / room_id / scene_id reference against the caller's live
//! inventory, collects warnings for any dangling refs, and returns
//! a `CompilePreview` for the UI to render. We NEVER persist without
//! an explicit follow-up POST to `/automations` — the user always sees
//! the translation before it commits.
//!
//! ## Why OpenRouter + Opus
//!
//! Matches `syntaur-verify`'s `OpusClient::from_vault` pattern: one
//! `openrouter` vault credential drives every "ask Opus about X" surface
//! in the gateway. No new key management. Override the model via
//! `SMART_HOME_NL_MODEL` env var when a newer Opus ships. Local
//! TurboQuant fallback is a follow-up — v1 keeps the LLM path explicit
//! so the translation quality is predictable.
//!
//! ## Dangling refs are warnings, not errors
//!
//! If Opus references a device that isn't in the inventory (rare —
//! we embed the full inventory in the system prompt) we surface a
//! warning in the preview rather than failing. The UI highlights the
//! offending card so the user can fix it in the builder before saving.
//! This is the "preview + confirm" policy from the plan's positioning
//! — LLM-produced specs are never a fait accompli.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Context;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::automation::{Action, AutomationSpec, Condition, Trigger};

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "anthropic/claude-opus-4";

#[derive(Debug, Clone, Deserialize)]
pub struct CompileRequest {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompilePreview {
    /// One-sentence plain-English restatement of what we'll do.
    pub summary: String,
    /// Compiled AST. May reference ids that don't exist — see `warnings`.
    pub spec: AutomationSpec,
    /// Non-fatal issues (unknown ids, empty triggers, etc.). The UI
    /// shows these alongside the preview so the user knows what to
    /// eyeball before saving.
    pub warnings: Vec<String>,
}

pub async fn compile(
    user_id: i64,
    db_path: PathBuf,
    req: CompileRequest,
) -> Result<CompilePreview, String> {
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("prompt is empty".to_string());
    }

    let inventory = load_inventory(user_id, db_path)
        .await
        .map_err(|e| format!("load inventory: {e}"))?;
    let api_key = fetch_openrouter_key().map_err(|e| format!("vault: {e}"))?;
    let sys_prompt = build_system_prompt(&inventory);
    let model = std::env::var("SMART_HOME_NL_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": sys_prompt},
            {"role": "user", "content": prompt},
        ],
        "response_format": {"type": "json_object"},
        "max_tokens": 1200,
    });

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("reqwest build: {e}"))?;

    let resp = http
        .post(OPENROUTER_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("HTTP-Referer", "https://github.com/buddyholly007/syntaur")
        .header("X-Title", "syntaur-nl-automation")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("openrouter POST: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read openrouter response: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "openrouter {}: {}",
            status,
            text.chars().take(300).collect::<String>()
        ));
    }

    let api: Value = serde_json::from_str(&text).map_err(|e| format!("parse api: {e}"))?;
    let content = api["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "no content in OpenRouter response".to_string())?
        .trim()
        .to_string();
    let json_str = strip_code_fence(&content).to_string();
    let raw: RawCompile = serde_json::from_str(&json_str).map_err(|e| {
        format!(
            "parse LLM JSON: {e}\nraw response (truncated): {}",
            json_str.chars().take(400).collect::<String>()
        )
    })?;

    let warnings = validate_spec(&raw.spec, &inventory);
    Ok(CompilePreview {
        summary: raw.summary,
        spec: raw.spec,
        warnings,
    })
}

#[derive(Debug, Deserialize)]
struct RawCompile {
    #[serde(default)]
    summary: String,
    spec: AutomationSpec,
}

fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        rest.trim_start_matches('\n')
            .trim_end_matches('\n')
            .trim_end_matches("```")
            .trim()
    } else if let Some(rest) = t.strip_prefix("```") {
        rest.trim_start_matches('\n')
            .trim_end_matches('\n')
            .trim_end_matches("```")
            .trim()
    } else {
        t
    }
}

fn fetch_openrouter_key() -> anyhow::Result<String> {
    use syntaur_vault_core::{
        agent::{request, AgentRequest, AgentResponse},
        default_socket_path,
    };
    let socket = default_socket_path();
    if !socket.exists() {
        anyhow::bail!(
            "vault agent not running at {} — run `syntaur-vault unlock` first. \
             Natural-language compile needs the `openrouter` entry.",
            socket.display()
        );
    }
    let resp = request(
        &socket,
        &AgentRequest::Get {
            name: "openrouter".into(),
        },
    )
    .context("asking vault for openrouter key")?;
    match resp {
        AgentResponse::Value { value } => Ok(value),
        AgentResponse::Error { message } => {
            anyhow::bail!("vault refused openrouter: {message}")
        }
        other => anyhow::bail!("unexpected vault response: {other:?}"),
    }
}

// ── Inventory loading ───────────────────────────────────────────────────

#[derive(Debug)]
struct Inventory {
    devices: Vec<InvDevice>,
    rooms: Vec<InvRoom>,
    scenes: Vec<InvScene>,
}

#[derive(Debug, Serialize)]
struct InvDevice {
    id: i64,
    name: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
}

#[derive(Debug, Serialize)]
struct InvRoom {
    id: i64,
    name: String,
}

#[derive(Debug, Serialize)]
struct InvScene {
    id: i64,
    name: String,
}

async fn load_inventory(user_id: i64, db_path: PathBuf) -> anyhow::Result<Inventory> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Inventory> {
        let conn = Connection::open(&db_path)?;

        let mut stmt = conn.prepare(
            "SELECT d.id, d.name, d.kind, r.name
               FROM smart_home_devices d
               LEFT JOIN smart_home_rooms r ON d.room_id = r.id
              WHERE d.user_id = ?
              ORDER BY d.id",
        )?;
        let devices: Vec<InvDevice> = stmt
            .query_map(rusqlite::params![user_id], |r| {
                Ok(InvDevice {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    room: r.get(3)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();

        let mut stmt = conn.prepare(
            "SELECT id, name FROM smart_home_rooms WHERE user_id = ? ORDER BY id",
        )?;
        let rooms: Vec<InvRoom> = stmt
            .query_map(rusqlite::params![user_id], |r| {
                Ok(InvRoom {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();

        let mut stmt = conn.prepare(
            "SELECT id, name FROM smart_home_scenes WHERE user_id = ? ORDER BY id",
        )?;
        let scenes: Vec<InvScene> = stmt
            .query_map(rusqlite::params![user_id], |r| {
                Ok(InvScene {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();

        Ok(Inventory {
            devices,
            rooms,
            scenes,
        })
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?
}

// ── Prompt construction ─────────────────────────────────────────────────

fn build_system_prompt(inv: &Inventory) -> String {
    let devices_json =
        serde_json::to_string_pretty(&inv.devices).unwrap_or_else(|_| "[]".into());
    let rooms_json = serde_json::to_string_pretty(&inv.rooms).unwrap_or_else(|_| "[]".into());
    let scenes_json = serde_json::to_string_pretty(&inv.scenes).unwrap_or_else(|_| "[]".into());

    format!(
        r#"You compile user natural-language descriptions of home-automation rules
into a strict JSON AutomationSpec. Output ONLY valid JSON — no prose,
no code fences.

## User's inventory

Devices (use these exact ids in any device_id field):
{devices_json}

Rooms (use these exact ids in any room_id field):
{rooms_json}

Scenes (use these exact ids in any scene_id field):
{scenes_json}

## Output shape

{{
  "summary": "one-sentence plain-English restatement of what will happen",
  "spec": {{
    "triggers":   [ ...one or more trigger objects... ],
    "conditions": [ ...zero or more condition objects... ],
    "actions":    [ ...one or more action objects... ]
  }}
}}

## Trigger kinds

{{"kind": "time",         "at": "HH:MM",                     "offset_min": 0}}
{{"kind": "device_state", "device_id": N,                    "equals": <any JSON value>}}
{{"kind": "presence",     "room_id": N, "person": "any",     "state": "entered" | "left"}}
{{"kind": "sensor",       "device_id": N, "above": <num|null>, "below": <num|null>}}

`at` accepts "HH:MM" 24-hour strings. Sunrise/sunset are not yet
supported — if requested, approximate with "06:00" / "18:00" and note
it in the summary.

## Condition kinds

{{"kind": "device_state", "device_id": N, "equals": <any JSON value>}}
{{"kind": "time_range",   "start": "HH:MM", "end": "HH:MM"}}
{{"kind": "anyone_home",  "expect": true | false}}

## Action kinds

{{"kind": "set_device", "device_id": N, "state": <JSON object>}}
{{"kind": "scene",      "scene_id": N}}
{{"kind": "notify",     "target": "telegram", "text": "..."}}
{{"kind": "delay",      "seconds": N}}

Conventional `state` payloads:
  light/switch/plug: {{"on": true}} / {{"on": false}}
  dimmable light:    {{"on": true, "brightness_pct": 50}}
  color light:       {{"on": true, "brightness_pct": 80, "color": {{"h": 30, "s": 100}} }}
  thermostat:        {{"target_temp_c": 20.5, "mode": "heat"}}
  cover/blind:       {{"position_pct": 100}}

## Rules

1. Use only ids that appear in the inventory above. If the user names
   a device that isn't in inventory, pick the closest match by kind +
   room + name, and mention the ambiguity in `summary`.
2. Every spec needs >= 1 trigger and >= 1 action. If you can't compile
   a valid rule, return a spec with the triggers/actions arrays you
   could figure out plus a summary starting with "Could not compile: "
   that explains what's missing.
3. Be conservative with presence triggers. If the user just says "when
   I'm home" without naming a room, prefer `anyone_home` as a condition
   rather than a presence trigger.
4. Delays go between actions, never at the start.
"#
    )
}

// ── Post-compile validation ─────────────────────────────────────────────

fn validate_spec(spec: &AutomationSpec, inv: &Inventory) -> Vec<String> {
    let mut warnings = Vec::new();
    let device_ids: HashSet<i64> = inv.devices.iter().map(|d| d.id).collect();
    let room_ids: HashSet<i64> = inv.rooms.iter().map(|r| r.id).collect();
    let scene_ids: HashSet<i64> = inv.scenes.iter().map(|s| s.id).collect();

    if spec.triggers.is_empty() {
        warnings.push("no triggers — automation will never fire".into());
    }
    if spec.actions.is_empty() {
        warnings.push("no actions — automation has nothing to do".into());
    }

    for t in &spec.triggers {
        match t {
            Trigger::DeviceState { device_id, .. } | Trigger::Sensor { device_id, .. } => {
                if !device_ids.contains(device_id) {
                    warnings
                        .push(format!("trigger references unknown device_id={device_id}"));
                }
            }
            Trigger::Presence { room_id, .. } => {
                if !room_ids.contains(room_id) {
                    warnings.push(format!("trigger references unknown room_id={room_id}"));
                }
            }
            Trigger::Time { .. } | Trigger::Voice { .. } => {}
        }
    }
    for c in &spec.conditions {
        if let Condition::DeviceState { device_id, .. } = c {
            if !device_ids.contains(device_id) {
                warnings.push(format!("condition references unknown device_id={device_id}"));
            }
        }
    }
    for a in &spec.actions {
        match a {
            Action::SetDevice { device_id, .. } => {
                if !device_ids.contains(device_id) {
                    warnings.push(format!("action references unknown device_id={device_id}"));
                }
            }
            Action::Scene { scene_id } => {
                if !scene_ids.contains(scene_id) {
                    warnings.push(format!("action references unknown scene_id={scene_id}"));
                }
            }
            Action::Notify { .. } | Action::Delay { .. } => {}
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_inventory() -> Inventory {
        Inventory {
            devices: vec![
                InvDevice {
                    id: 1,
                    name: "Porch light".into(),
                    kind: "light".into(),
                    room: Some("Porch".into()),
                },
                InvDevice {
                    id: 2,
                    name: "Kitchen sensor".into(),
                    kind: "sensor_motion".into(),
                    room: Some("Kitchen".into()),
                },
            ],
            rooms: vec![
                InvRoom { id: 10, name: "Porch".into() },
                InvRoom { id: 11, name: "Kitchen".into() },
            ],
            scenes: vec![InvScene { id: 100, name: "Evening".into() }],
        }
    }

    #[test]
    fn validate_flags_unknown_device_ref() {
        let inv = sample_inventory();
        let spec = AutomationSpec {
            triggers: vec![Trigger::Time {
                at: "18:00".into(),
                offset_min: 0,
            }],
            trigger_logic: Default::default(),
            conditions: vec![],
            actions: vec![Action::SetDevice {
                device_id: 999, // not in inventory
                state: json!({ "on": true }),
            }],
        };
        let w = validate_spec(&spec, &inv);
        assert!(w.iter().any(|s| s.contains("device_id=999")));
    }

    #[test]
    fn validate_flags_unknown_room_ref() {
        let inv = sample_inventory();
        let spec = AutomationSpec {
            triggers: vec![Trigger::Presence {
                room_id: 77,
                person: "Sean".into(),
                state: "entered".into(),
            }],
            trigger_logic: Default::default(),
            conditions: vec![],
            actions: vec![Action::Notify {
                target: "telegram".into(),
                text: "home".into(),
            }],
        };
        let w = validate_spec(&spec, &inv);
        assert!(w.iter().any(|s| s.contains("room_id=77")));
    }

    #[test]
    fn validate_flags_empty_triggers_or_actions() {
        let inv = sample_inventory();
        let spec = AutomationSpec {
            triggers: vec![],
            trigger_logic: Default::default(),
            conditions: vec![],
            actions: vec![],
        };
        let w = validate_spec(&spec, &inv);
        assert!(w.iter().any(|s| s.contains("no triggers")));
        assert!(w.iter().any(|s| s.contains("no actions")));
    }

    #[test]
    fn validate_accepts_clean_spec() {
        let inv = sample_inventory();
        let spec = AutomationSpec {
            triggers: vec![Trigger::Time {
                at: "18:30".into(),
                offset_min: 0,
            }],
            trigger_logic: Default::default(),
            conditions: vec![Condition::AnyoneHome { expect: true }],
            actions: vec![
                Action::SetDevice {
                    device_id: 1,
                    state: json!({ "on": true }),
                },
                Action::Scene { scene_id: 100 },
            ],
        };
        let w = validate_spec(&spec, &inv);
        assert!(w.is_empty(), "expected no warnings, got {w:?}");
    }

    #[test]
    fn strip_code_fence_handles_json_wrap() {
        let s = "```json\n{\"summary\":\"x\"}\n```";
        assert_eq!(strip_code_fence(s), "{\"summary\":\"x\"}");
    }
}
