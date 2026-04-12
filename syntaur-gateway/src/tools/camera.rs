//! Camera tool — query Frigate NVR + Reolink cameras directly.
//!
//! Frigate REST API at http://192.168.1.239:5000 provides:
//! - Recent events (person/car/animal detections)
//! - Camera status (recording, detecting, fps)
//! - Snapshot URLs
//!
//! Reolink cameras have their own REST API but are also managed by Frigate,
//! so we primarily use Frigate for everything.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

const FRIGATE_URL: &str = "http://192.168.1.239:5000";

pub struct CameraTool;

#[async_trait]
impl Tool for CameraTool {
    fn name(&self) -> &str { "camera" }

    fn description(&self) -> &str {
        "Check security cameras via Frigate NVR. See recent detections \
         (people, cars, animals), camera status, or get a current snapshot. \
         Cameras: front door, driveway, backyard, side yard, garage."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["events", "status", "snapshot"],
                    "description": "events = recent detections, status = camera health, snapshot = current image URL."
                },
                "camera": {
                    "type": "string",
                    "description": "Camera name (e.g. 'front_door', 'driveway'). Default: all cameras."
                },
                "limit": {
                    "type": "integer",
                    "description": "For events: max number to return. Default: 5."
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let client = ctx.http.as_ref().ok_or("no HTTP client")?;
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("events");
        let camera = args.get("camera").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);

        match action {
            "events" => {
                let mut url = format!("{}/api/events?limit={}", FRIGATE_URL, limit);
                if !camera.is_empty() {
                    url = format!("{}&camera={}", url, camera);
                }
                let resp = client
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                    .map_err(|e| format!("Frigate API: {}", e))?;

                if !resp.status().is_success() {
                    return Err(format!("Frigate API: HTTP {}", resp.status()));
                }

                let events: Vec<Value> = resp.json().await.map_err(|e| format!("parse: {}", e))?;

                if events.is_empty() {
                    return Ok(RichToolResult::text("No recent camera events."));
                }

                let lines: Vec<String> = events
                    .iter()
                    .take(limit as usize)
                    .map(|e| {
                        let label = e.get("label").and_then(|v| v.as_str()).unwrap_or("?");
                        let cam = e.get("camera").and_then(|v| v.as_str()).unwrap_or("?");
                        let score = e.get("top_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let start = e.get("start_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let dt = chrono::DateTime::from_timestamp(start as i64, 0)
                            .map(|d| d.format("%I:%M %p").to_string())
                            .unwrap_or_else(|| "?".to_string());
                        format!("- {} detected on {} at {} ({:.0}% confidence)", label, cam, dt, score * 100.0)
                    })
                    .collect();

                Ok(RichToolResult::text(format!(
                    "{} recent event{}:\n{}",
                    lines.len(),
                    if lines.len() == 1 { "" } else { "s" },
                    lines.join("\n")
                )))
            }
            "status" => {
                let url = format!("{}/api/stats", FRIGATE_URL);
                let resp = client
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                    .map_err(|e| format!("Frigate API: {}", e))?;

                if !resp.status().is_success() {
                    return Err(format!("Frigate API: HTTP {}", resp.status()));
                }

                let stats: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;

                let cameras = stats.get("cameras").and_then(|c| c.as_object());
                match cameras {
                    Some(cams) => {
                        let lines: Vec<String> = cams
                            .iter()
                            .map(|(name, info)| {
                                let fps = info
                                    .get("camera_fps")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);
                                let detect = info
                                    .get("detection_fps")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);
                                format!("- {}: {:.1} fps, detect {:.1} fps", name, fps, detect)
                            })
                            .collect();
                        Ok(RichToolResult::text(format!(
                            "Camera status:\n{}",
                            lines.join("\n")
                        )))
                    }
                    None => Ok(RichToolResult::text("No camera stats available.")),
                }
            }
            "snapshot" => {
                let cam = if camera.is_empty() { "front_door" } else { camera };
                let url = format!("{}/api/{}/latest.jpg", FRIGATE_URL, cam);
                Ok(RichToolResult::text(format!(
                    "Snapshot URL for {}: {}",
                    cam, url
                )))
            }
            other => Err(format!("camera: unknown action '{}'", other)),
        }
    }
}
