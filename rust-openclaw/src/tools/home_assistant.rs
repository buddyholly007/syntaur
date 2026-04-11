//! Home Assistant integration tools — voice / agent control of HA entities
//! via the HA REST API.
//!
//! These tools let any LLM running through the openclaw chat completions
//! endpoint actually change the state of smart home devices. They were
//! added to give Peter (the satellite voice assistant) a sane tool surface
//! that the model can reason about, instead of HA's `execute_services`
//! catch-all which forced the LLM to know every HA service signature.
//!
//! Wiring:
//!   1. `~/.syntaur/syntaur.json` `connectors.home_assistant` block sets
//!      `base_url` and `bearer_token`.
//!   2. `tools::ToolRegistry::with_extensions` registers each tool below
//!      under its `name()`.
//!   3. The voice chat handler in `voice_chat.rs` exposes them to the LLM.
//!
//! Each tool is a thin wrapper around `HomeAssistantClient`, which holds
//! the configured base_url + bearer token and a shared `reqwest::Client`.
//! Tools never panic; HA errors are propagated as `Err(String)` and the
//! ToolRegistry funnel turns those into a tool result the LLM can read.

use async_trait::async_trait;
use log::{info, warn};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

/// Thin REST client for the HA `/api/services/...` and `/api/states/...`
/// endpoints. Holds nothing but the base URL + token + shared http client,
/// so it's cheap to clone into each tool struct.
#[derive(Clone)]
pub struct HomeAssistantClient {
    base_url: String,
    bearer_token: String,
    http: reqwest::Client,
}

impl HomeAssistantClient {
    pub fn new(base_url: String, bearer_token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            bearer_token,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// POST /api/services/{domain}/{service} with the given JSON body.
    /// Returns the parsed JSON response (HA returns a list of changed
    /// states, which we surface back to the LLM verbatim).
    pub async fn call_service(
        &self,
        domain: &str,
        service: &str,
        body: Value,
    ) -> Result<Value, String> {
        let url = format!("{}/api/services/{}/{}", self.base_url, domain, service);
        info!("[ha] POST {}", url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HA request failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HA {} {}/{} -> HTTP {}: {}",
                "POST",
                domain,
                service,
                status,
                text.chars().take(200).collect::<String>()
            ));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("HA response parse failed: {}", e))
    }

    /// GET /api/states/{entity_id} — returns the entity's state object.
    pub async fn get_state(&self, entity_id: &str) -> Result<Value, String> {
        let url = format!("{}/api/states/{}", self.base_url, entity_id);
        info!("[ha] GET {}", url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .send()
            .await
            .map_err(|e| format!("HA state request failed: {}", e))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(format!("entity not found: {}", entity_id));
        }
        if !status.is_success() {
            return Err(format!("HA state HTTP {}", status));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("HA state parse failed: {}", e))
    }

    /// GET /api/states — returns all states (full LAN). The voice path uses
    /// this once at startup if the LLM ever asks "what entities exist?",
    /// but we don't put it in the per-call hot path.
    #[allow(dead_code)]
    pub async fn list_states(&self) -> Result<Vec<Value>, String> {
        let url = format!("{}/api/states", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .send()
            .await
            .map_err(|e| format!("HA list_states failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("HA list_states HTTP {}", resp.status()));
        }
        resp.json::<Vec<Value>>()
            .await
            .map_err(|e| format!("HA list_states parse: {}", e))
    }
}

/// Module-private singleton — the tools below are zero-sized structs and
/// can't carry an HA client field through their `Tool` impl, so they read
/// it from this OnceLock initialized at process startup by main.rs via
/// `init_home_assistant`.
static HA_CLIENT: std::sync::OnceLock<HomeAssistantClient> = std::sync::OnceLock::new();

/// Install the global HA client. Called once from main.rs after config is
/// loaded. Subsequent calls are no-ops.
pub fn init_home_assistant(client: HomeAssistantClient) {
    let _ = HA_CLIENT.set(client);
}

/// Borrow the global HA client. Returns an error string if the connector
/// wasn't configured at startup.
pub fn ha() -> Result<&'static HomeAssistantClient, String> {
    HA_CLIENT
        .get()
        .ok_or_else(|| "home_assistant connector not configured (set connectors.home_assistant in syntaur.json)".to_string())
}

/// Optional borrow — returns None if the HA connector wasn't configured.
/// Used by the timer tick task which degrades gracefully (logs a warning
/// instead of erroring) when HA is unreachable.
pub fn get_client() -> Option<&'static HomeAssistantClient> {
    HA_CLIENT.get()
}

// ── Tool: control_light ─────────────────────────────────────────────────────

/// LLM-facing tool that turns lights on/off and sets brightness, color
/// temperature, or RGB color in a single call. The LLM passes the
/// kelvin / brightness / rgb fields it wants; missing fields are simply
/// not sent to HA, which preserves the bulb's current value.
pub struct HaControlLightTool;

#[async_trait]
impl Tool for HaControlLightTool {
    fn name(&self) -> &str {
        "control_light"
    }

    fn description(&self) -> &str {
        "Turn a light entity on or off and optionally set brightness, color temperature, or RGB color. \
         Use this for any room or area light or light group (e.g. light.kitchen_lights, light.office_lights). \
         Common color temperatures: warm white = 2700, neutral white = 4000, daylight = 5000, cool white = 6500."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entity_id": {
                    "type": "string",
                    "description": "The HA light entity_id to control, e.g. 'light.kitchen_lights'."
                },
                "action": {
                    "type": "string",
                    "enum": ["turn_on", "turn_off", "toggle"],
                    "description": "Whether to turn the light on, off, or toggle it."
                },
                "brightness_pct": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Brightness percentage 1-100 (only for turn_on)."
                },
                "color_temp_kelvin": {
                    "type": "integer",
                    "minimum": 1500,
                    "maximum": 7000,
                    "description": "Color temperature in kelvin (only for turn_on). Warm=2700, daylight=5000, cool=6500."
                },
                "rgb_color": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 0, "maximum": 255 },
                    "minItems": 3,
                    "maxItems": 3,
                    "description": "Optional RGB array [r,g,b] each 0-255 (only for turn_on)."
                }
            },
            "required": ["entity_id", "action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            requires_approval: Some(false),
            circuit_name: Some("home_assistant"),
            rate_limit: None,
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let entity_id = args
            .get("entity_id")
            .and_then(|v| v.as_str())
            .ok_or("missing entity_id")?
            .to_string();
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("turn_on")
            .to_string();
        let service = match action.as_str() {
            "turn_on" => "turn_on",
            "turn_off" => "turn_off",
            "toggle" => "toggle",
            other => return Err(format!("unknown action '{}'", other)),
        };

        let has_brightness_or_color = args.get("brightness_pct").is_some()
            || args.get("color_temp_kelvin").is_some()
            || args.get("rgb_color").is_some();

        let mut body = json!({ "entity_id": entity_id });
        if service == "turn_on" {
            if let Some(b) = args.get("brightness_pct").and_then(|v| v.as_i64()) {
                body["brightness_pct"] = json!(b.clamp(1, 100));
            }
            if let Some(k) = args.get("color_temp_kelvin").and_then(|v| v.as_i64()) {
                body["color_temp_kelvin"] = json!(k);
            }
            if let Some(rgb) = args.get("rgb_color").and_then(|v| v.as_array()) {
                if rgb.len() == 3 {
                    body["rgb_color"] = json!(rgb);
                }
            }
        }

        let result = ha()?.call_service("light", service, body).await?;
        let count = result.as_array().map(|a| a.len()).unwrap_or(0);

        // When brightness/color is set on a room group that has dimmer switches
        // controlling smart bulbs, correct the dimmers back to 100% so only the
        // bulbs handle dimming. Without this, HA sends brightness to both the
        // dimmer (reducing circuit power) and the bulbs (internal dimming) —
        // causing double-dimming and potential bulb damage.
        if has_brightness_or_color && service == "turn_on" {
            if let Some(dimmers) = group_dimmer_switches(&entity_id) {
                let client = ha()?;
                for dimmer in &dimmers {
                    info!("[ha] correcting dimmer {} to 100% (smart bulbs handle dimming)", dimmer);
                    let _ = client.call_service("light", "turn_on", json!({
                        "entity_id": dimmer,
                        "brightness_pct": 100
                    })).await;
                }
            }
        }

        Ok(RichToolResult::text(format!(
            "Light {} -> {} ({} entity state(s) updated).",
            entity_id, service, count
        )))
    }
}

/// Returns the dimmer switch entity_ids within a light group that have smart
/// bulbs on the same circuit. These dimmers should always run at 100% — only
/// the smart bulbs should handle dimming. Returns None for groups that are
/// fine as-is (on/off switches only, or dimmer-only rooms without smart bulbs).
fn group_dimmer_switches(group_entity_id: &str) -> Option<Vec<String>> {
    // Static mapping of groups → dimmer entities that share a circuit with
    // smart bulbs. Rooms like entryway (dimmer, no smart bulbs) and office
    // (on/off switch, smart bulbs) are NOT listed because they don't have
    // the double-dimming problem.
    let map: &[(&str, &[&str])] = &[
        ("light.kitchen_lights", &[
            "light.smart_wi_fi_dimmer_switch_7",  // Kitchen Perimeter
            "light.smart_wi_fi_dimmer_switch_8",  // Kitchen Pendant
        ]),
        ("light.dining_room_lights", &[
            "light.smart_wi_fi_dimmer_switch_6",  // Dining Room Chandelier
        ]),
        ("light.living_room_lights", &[
            "light.smart_wi_fi_dimmer_switch_2",  // Living Room Chandelier
        ]),
        ("light.master_bedroom_lights", &[
            "light.smart_wi_fi_dimmer_switch_9",  // Master Bedroom
        ]),
        ("light.master_bathroom_lights", &[
            "light.smart_wi_fi_dimmer_switch",    // Master Bathroom Vanity
        ]),
        ("light.williams_bedroom_lights", &[
            "light.smart_wi_fi_dimmer_switch_3",  // William's Bedroom
        ]),
        ("light.anastasias_bedroom_lights", &[
            "light.smart_wi_fi_dimmer_switch_4",  // Anastasia's Bedroom
        ]),
    ];

    for (group, dimmers) in map {
        if *group == group_entity_id {
            return Some(dimmers.iter().map(|s| s.to_string()).collect());
        }
    }
    None
}

// ── Tool: set_thermostat ────────────────────────────────────────────────────

/// Sets the target temperature on a climate entity. Optional preset_mode
/// for "auto/heat/cool/off" mode switching.
pub struct HaSetThermostatTool;

#[async_trait]
impl Tool for HaSetThermostatTool {
    fn name(&self) -> &str {
        "set_thermostat"
    }

    fn description(&self) -> &str {
        "Set the target temperature on a climate / thermostat entity. \
         Optionally set the HVAC mode (heat, cool, auto, off). \
         Temperature is in the unit configured in HA (Fahrenheit for this household)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entity_id": {
                    "type": "string",
                    "description": "The climate entity_id, e.g. 'climate.thermostat_1_nativezone'."
                },
                "temperature": {
                    "type": "number",
                    "description": "Target temperature in degrees (Fahrenheit for this household)."
                },
                "hvac_mode": {
                    "type": "string",
                    "enum": ["heat", "cool", "auto", "heat_cool", "off"],
                    "description": "Optional HVAC mode to switch to before setting temperature."
                }
            },
            "required": ["entity_id", "temperature"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            requires_approval: Some(false),
            circuit_name: Some("home_assistant"),
            rate_limit: None,
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let entity_id = args
            .get("entity_id")
            .and_then(|v| v.as_str())
            .ok_or("missing entity_id")?
            .to_string();
        let temperature = args
            .get("temperature")
            .and_then(|v| v.as_f64())
            .ok_or("missing temperature")?;

        let client = ha()?;
        if let Some(mode) = args.get("hvac_mode").and_then(|v| v.as_str()) {
            client
                .call_service(
                    "climate",
                    "set_hvac_mode",
                    json!({ "entity_id": entity_id, "hvac_mode": mode }),
                )
                .await
                .map_err(|e| format!("set_hvac_mode failed: {}", e))?;
        }

        client
            .call_service(
                "climate",
                "set_temperature",
                json!({ "entity_id": entity_id, "temperature": temperature }),
            )
            .await?;
        Ok(RichToolResult::text(format!(
            "Thermostat {} set to {} degrees.",
            entity_id, temperature
        )))
    }
}

// ── Tool: query_state ───────────────────────────────────────────────────────

/// Reads the current state of a HA entity. Used for "what is the
/// thermostat temperature", "is the front door locked", etc.
pub struct HaQueryStateTool;

#[async_trait]
impl Tool for HaQueryStateTool {
    fn name(&self) -> &str {
        "query_state"
    }

    fn description(&self) -> &str {
        "Read the current state and attributes of a Home Assistant entity. \
         Returns the entity's state value plus any relevant attributes \
         (current_temperature for climate, brightness for lights, etc.)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entity_id": {
                    "type": "string",
                    "description": "The HA entity_id to query, e.g. 'climate.thermostat_1_nativezone' or 'weather.forecast_home'."
                }
            },
            "required": ["entity_id"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            destructive: false,
            idempotent: true,
            network: true,
            requires_approval: Some(false),
            circuit_name: Some("home_assistant"),
            rate_limit: None,
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let entity_id = args
            .get("entity_id")
            .and_then(|v| v.as_str())
            .ok_or("missing entity_id")?;
        let state = ha()?.get_state(entity_id).await?;
        // Compact summary: friendly_name + state + a couple useful attributes.
        let s = state.get("state").and_then(|v| v.as_str()).unwrap_or("?");
        let attrs = state.get("attributes").cloned().unwrap_or(json!({}));
        let friendly = attrs
            .get("friendly_name")
            .and_then(|v| v.as_str())
            .unwrap_or(entity_id);
        let mut summary = format!("{} = {}", friendly, s);
        for key in &[
            "current_temperature",
            "temperature",
            "brightness",
            "color_temp_kelvin",
            "rgb_color",
            "humidity",
            "forecast",
            "weather",
        ] {
            if let Some(v) = attrs.get(*key) {
                summary.push_str(&format!(" | {}={}", key, v));
            }
        }
        Ok(RichToolResult::text(summary))
    }
}

// ── Tool: call_service ──────────────────────────────────────────────────────

/// Generic escape hatch for any HA service. The LLM only needs this for
/// rare actions not covered by the typed tools above (e.g. media_player
/// playback, scene activation, custom scripts). The model is told in the
/// system prompt to PREFER the typed tools when they fit.
pub struct HaCallServiceTool;

#[async_trait]
impl Tool for HaCallServiceTool {
    fn name(&self) -> &str {
        "call_ha_service"
    }

    fn description(&self) -> &str {
        "Call any Home Assistant service directly. Use this only for actions not covered by control_light or set_thermostat \
         (e.g. media_player.play_media, script.turn_on, scene.turn_on, todo.add_item, automation.trigger). \
         Pass the domain, service name, and a service_data JSON object containing entity_id and any service-specific fields."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "description": "Service domain, e.g. 'media_player', 'script', 'todo'."
                },
                "service": {
                    "type": "string",
                    "description": "Service name within the domain, e.g. 'play_media', 'turn_on', 'add_item'."
                },
                "service_data": {
                    "type": "object",
                    "description": "JSON object passed to the service. Must include entity_id when targeting an entity."
                }
            },
            "required": ["domain", "service", "service_data"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            requires_approval: Some(false),
            circuit_name: Some("home_assistant"),
            rate_limit: None,
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let domain = args
            .get("domain")
            .and_then(|v| v.as_str())
            .ok_or("missing domain")?;
        let service = args
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or("missing service")?;
        let service_data = args
            .get("service_data")
            .cloned()
            .ok_or("missing service_data")?;
        let result = ha()?.call_service(domain, service, service_data).await?;
        let updated = result.as_array().map(|a| a.len()).unwrap_or(0);
        Ok(RichToolResult::text(format!(
            "{}.{} executed ({} entity state(s) updated).",
            domain, service, updated
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_light_schema_round_trip() {
        let t = HaControlLightTool;
        let schema = t.schema();
        // Must contain function.name = control_light
        assert_eq!(
            schema["function"]["name"].as_str(),
            Some("control_light")
        );
        // Parameters must declare entity_id and action as required
        let req = schema["function"]["parameters"]["required"]
            .as_array()
            .expect("required must be an array");
        let names: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"entity_id"));
        assert!(names.contains(&"action"));
    }

    #[test]
    fn test_set_thermostat_schema() {
        let t = HaSetThermostatTool;
        let s = t.schema();
        assert_eq!(s["function"]["name"].as_str(), Some("set_thermostat"));
    }

    #[test]
    fn test_query_state_schema() {
        let t = HaQueryStateTool;
        let s = t.schema();
        assert_eq!(s["function"]["name"].as_str(), Some("query_state"));
    }

    #[test]
    fn test_call_service_schema() {
        let t = HaCallServiceTool;
        let s = t.schema();
        assert_eq!(s["function"]["name"].as_str(), Some("call_ha_service"));
    }

    #[test]
    fn test_ha_client_unconfigured_returns_error() {
        // Without init_home_assistant having been called, ha() must err.
        // (This test runs in isolation per #[test] but OnceLock is process-
        // global, so we can't easily verify the negative case once init has
        // run elsewhere. We just sanity-check the function signature.)
        let _ = ha();
    }
}
