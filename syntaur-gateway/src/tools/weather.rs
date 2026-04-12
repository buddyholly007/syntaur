//! Pure-Rust weather tool — talks directly to wttr.in (no API key).
//!
//! This is the first Phase 1 pure-Rust skill: no HA involvement, no
//! Python, no external service beyond the free weather API. The tool
//! is registered in the ToolRouter so `find_tool` dispatches intents
//! like "what's the weather" and "will it rain tomorrow" here.
//!
//! ## Why wttr.in instead of NWS
//!
//! wttr.in is simpler (single HTTP GET, clean JSON), works globally
//! (NWS is US-only), requires no geocoding step, and accepts city names
//! directly in the URL. The format=j1 endpoint returns a structured JSON
//! payload with current conditions + 3-day forecast + hourly.
//!
//! ## Voice UX
//!
//! The tool returns a concise text summary suitable for Peter to read
//! aloud. No markdown, no tables, no long lists. Example output:
//! "Sacramento right now: 72°F, partly cloudy. High today 81°F, low 54°F.
//! Tomorrow: sunny, high 85°F."

use async_trait::async_trait;
use log::warn;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

/// Default location when the user doesn't specify one. Sean is in Sacramento.
const DEFAULT_LOCATION: &str = "Sacramento";

pub struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "Get current weather conditions and forecast for a location. \
         Returns temperature, conditions, and tomorrow's forecast. \
         Accepts city names, zip codes, or 'here' for the default location."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "City name, zip code, or airport code. Default: Sacramento."
                }
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let location = args
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_LOCATION)
            .trim();
        let location = if location.is_empty() || location.eq_ignore_ascii_case("here") {
            DEFAULT_LOCATION
        } else {
            location
        };

        let client = ctx
            .http
            .as_ref()
            .ok_or_else(|| "weather: no HTTP client available".to_string())?;

        let url = format!("https://wttr.in/{}?format=j1", urlencoded(location));
        let resp = client
            .get(&url)
            .header("User-Agent", "syntaur/0.1 weather-tool")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("weather fetch failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("wttr.in returned {}", resp.status()));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("weather parse failed: {}", e))?;

        // Extract current conditions
        let current = body
            .get("current_condition")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first());
        let forecast = body
            .get("weather")
            .and_then(|v| v.as_array());

        let mut parts = Vec::new();

        if let Some(c) = current {
            let temp_f = c.get("temp_F").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = c
                .get("weatherDesc")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|d| d.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let humidity = c.get("humidity").and_then(|v| v.as_str()).unwrap_or("?");
            let wind_mph = c.get("windspeedMiles").and_then(|v| v.as_str()).unwrap_or("?");
            parts.push(format!(
                "{} right now: {}°F, {}. Humidity {}%, wind {} mph.",
                location, temp_f, desc.to_lowercase(), humidity, wind_mph
            ));
        }

        if let Some(days) = forecast {
            // Today
            if let Some(today) = days.first() {
                let hi = today.get("maxtempF").and_then(|v| v.as_str()).unwrap_or("?");
                let lo = today.get("mintempF").and_then(|v| v.as_str()).unwrap_or("?");
                parts.push(format!("High today {}°F, low {}°F.", hi, lo));
            }
            // Tomorrow
            if let Some(tomorrow) = days.get(1) {
                let hi = tomorrow.get("maxtempF").and_then(|v| v.as_str()).unwrap_or("?");
                let lo = tomorrow.get("mintempF").and_then(|v| v.as_str()).unwrap_or("?");
                let desc = tomorrow
                    .get("hourly")
                    .and_then(|h| h.as_array())
                    .and_then(|a| a.get(4)) // ~noon
                    .and_then(|h| h.get("weatherDesc"))
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|d| d.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                parts.push(format!(
                    "Tomorrow: {}, high {}°F, low {}°F.",
                    desc.to_lowercase(),
                    hi,
                    lo
                ));
            }
        }

        if parts.is_empty() {
            return Err(format!("No weather data for '{}'", location));
        }

        Ok(RichToolResult::text(parts.join(" ")))
    }
}

/// Minimal URL encoding for location names with spaces/special chars.
fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('#', "%23")
}
