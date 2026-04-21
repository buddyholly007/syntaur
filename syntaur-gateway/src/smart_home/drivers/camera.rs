//! Camera driver — v1 ships Frigate-backed discovery + event timeline. ONVIF direct
//! support is nice-to-have but Frigate already speaks ONVIF to every
//! camera it records, so we inherit that reach without pulling in a
//! second dep tree. v1.x can add `onvif-rs` for non-Frigate cameras.
//!
//! Scan: GET Frigate's `/api/config` → one `ScanCandidate` per camera
//! it knows about. The camera's `name` in Frigate becomes our
//! `external_id`; the RTSP stream URL + enabled detectors go in
//! `details`. Scan fails gracefully (empty vec) if Frigate is offline
//! or unreachable — that keeps the rest of the scan pipeline healthy.
//!
//! Configuration: `SMART_HOME_FRIGATE_URL` env var overrides the
//! default `http://192.168.1.239:5000`. Matches the existing
//! `tools::camera::FRIGATE_URL` default so today's install works
//! immediately.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::smart_home::scan::ScanCandidate;

/// One detection event from Frigate, projected into the shape the
/// dashboard timeline renders. Kept tight so the JSON payload stays
/// small on slow LANs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraEvent {
    pub id: String,
    pub camera: String,
    pub label: String,
    pub score: f64,
    /// Unix seconds of the start of the detection.
    pub start_time: i64,
    /// Unix seconds of the end (None if still active).
    pub end_time: Option<i64>,
    pub has_snapshot: bool,
    pub has_clip: bool,
    /// Thumbnail URL — Frigate serves a jpg keyed by event id.
    pub thumbnail_url: String,
}

const DEFAULT_FRIGATE_URL: &str = "http://192.168.1.239:5000";

fn frigate_url() -> String {
    std::env::var("SMART_HOME_FRIGATE_URL")
        .unwrap_or_else(|_| DEFAULT_FRIGATE_URL.to_string())
}

/// Fetch recent detection events from Frigate. Returns empty on any
/// network failure — timeline UI should show "no events" rather than
/// erroring out.
pub async fn recent_events(camera: Option<&str>, limit: u32) -> Vec<CameraEvent> {
    let base = frigate_url();
    let mut url = format!("{}/api/events?limit={}", base, limit.min(500));
    if let Some(c) = camera {
        if !c.is_empty() {
            url.push_str(&format!("&camera={}", c));
        }
    }
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let raw: Vec<Value> = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    raw.iter()
        .filter_map(|e| parse_event(e, &base))
        .collect()
}

pub fn parse_event(e: &Value, frigate_base: &str) -> Option<CameraEvent> {
    let id = e.get("id")?.as_str()?.to_string();
    let camera = e.get("camera")?.as_str()?.to_string();
    let label = e
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("object")
        .to_string();
    let score = e
        .get("top_score")
        .or_else(|| e.get("score"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let start_time = e
        .get("start_time")
        .and_then(|v| v.as_f64())
        .map(|f| f as i64)
        .unwrap_or(0);
    let end_time = e.get("end_time").and_then(|v| v.as_f64()).map(|f| f as i64);
    let has_snapshot = e
        .get("has_snapshot")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_clip = e
        .get("has_clip")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let thumbnail_url = format!("{}/api/events/{}/thumbnail.jpg", frigate_base, id);
    Some(CameraEvent {
        id,
        camera,
        label,
        score,
        start_time,
        end_time,
        has_snapshot,
        has_clip,
        thumbnail_url,
    })
}

pub async fn scan() -> Vec<ScanCandidate> {
    let base = frigate_url();
    let url = format!("{}/api/config", base);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[smart_home::camera] http client build failed: {}", e);
            return Vec::new();
        }
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            log::info!(
                "[smart_home::camera] Frigate unreachable at {} ({}); skipping scan",
                base,
                e
            );
            return Vec::new();
        }
    };
    if !resp.status().is_success() {
        log::info!(
            "[smart_home::camera] Frigate returned HTTP {}; skipping scan",
            resp.status()
        );
        return Vec::new();
    }
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[smart_home::camera] Frigate config parse failed: {}", e);
            return Vec::new();
        }
    };
    parse_config(&body, &base)
}

/// Public for unit testing against a captured config.json blob.
pub fn parse_config(body: &Value, frigate_base: &str) -> Vec<ScanCandidate> {
    let Some(cameras) = body.get("cameras").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    cameras
        .iter()
        .map(|(name, cfg)| candidate_from_camera(name, cfg, frigate_base))
        .collect()
}

fn candidate_from_camera(name: &str, cfg: &Value, frigate_base: &str) -> ScanCandidate {
    // Frigate exposes `ffmpeg.inputs[].path` as RTSP URLs; the first
    // role: ["detect"] input is the ML stream. Pull whichever we can
    // find for the "what's at the other end" detail.
    let primary_rtsp = cfg
        .get("ffmpeg")
        .and_then(|f| f.get("inputs"))
        .and_then(|v| v.as_array())
        .and_then(|inputs| inputs.first())
        .and_then(|i| i.get("path"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let enabled = cfg
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    ScanCandidate {
        driver: "camera_frigate".into(),
        external_id: format!("frigate:{}", name),
        name: name.to_string(),
        kind: "camera".into(),
        vendor: cfg
            .get("motion")
            .and_then(|_| cfg.get("detect"))
            .and_then(|_| Some("Frigate-managed".to_string())),
        ip: None,
        mac: None,
        details: serde_json::json!({
            "source": "frigate",
            "enabled": enabled,
            "rtsp": primary_rtsp,
            "snapshot_url": format!("{}/api/{}/latest.jpg", frigate_base, name),
            "stream_url": format!("{}/api/{}/latest.webp", frigate_base, name),
            "events_url": format!("{}/api/events?camera={}&limit=50", frigate_base, name),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_config() -> Value {
        json!({
            "cameras": {
                "driveway": {
                    "enabled": true,
                    "ffmpeg": {
                        "inputs": [
                            { "path": "rtsp://192.168.1.240/driveway", "roles": ["detect","record"] }
                        ]
                    },
                    "motion": {},
                    "detect": {}
                },
                "porch": {
                    "enabled": true,
                    "ffmpeg": {
                        "inputs": [
                            { "path": "rtsp://192.168.1.241/porch", "roles": ["detect"] }
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn parse_config_emits_one_candidate_per_camera() {
        let out = parse_config(&sample_config(), "http://frigate.local:5000");
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|c| c.external_id == "frigate:driveway"));
        assert!(out.iter().any(|c| c.external_id == "frigate:porch"));
    }

    #[test]
    fn candidate_details_include_rtsp_and_snapshot() {
        let out = parse_config(&sample_config(), "http://frigate.local:5000");
        let drv = out.iter().find(|c| c.name == "driveway").unwrap();
        assert_eq!(drv.driver, "camera_frigate");
        assert_eq!(drv.kind, "camera");
        let rtsp = drv.details.get("rtsp").and_then(|v| v.as_str()).unwrap();
        assert_eq!(rtsp, "rtsp://192.168.1.240/driveway");
        let snap = drv
            .details
            .get("snapshot_url")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(snap, "http://frigate.local:5000/api/driveway/latest.jpg");
    }

    #[test]
    fn parse_config_handles_missing_cameras_block() {
        assert_eq!(parse_config(&json!({}), "http://x").len(), 0);
    }

    #[test]
    fn parse_event_extracts_fields() {
        let e = json!({
            "id": "1712345678.123456-abcdef",
            "camera": "driveway",
            "label": "person",
            "top_score": 0.87,
            "start_time": 1712345678.123,
            "end_time": 1712345712.456,
            "has_snapshot": true,
            "has_clip": false
        });
        let ev = parse_event(&e, "http://frigate.local:5000").expect("event");
        assert_eq!(ev.camera, "driveway");
        assert_eq!(ev.label, "person");
        assert!((ev.score - 0.87).abs() < 1e-9);
        assert_eq!(ev.start_time, 1712345678);
        assert_eq!(ev.end_time, Some(1712345712));
        assert!(ev.has_snapshot);
        assert!(!ev.has_clip);
        assert_eq!(
            ev.thumbnail_url,
            "http://frigate.local:5000/api/events/1712345678.123456-abcdef/thumbnail.jpg"
        );
    }

    #[test]
    fn parse_event_requires_id_and_camera() {
        // Missing id
        assert!(parse_event(&json!({ "camera": "x" }), "http://x").is_none());
        // Missing camera
        assert!(parse_event(&json!({ "id": "abc" }), "http://x").is_none());
    }

    #[test]
    fn parse_event_treats_active_detection_as_none_end_time() {
        let e = json!({ "id": "x", "camera": "y", "label": "dog", "start_time": 100.0 });
        let ev = parse_event(&e, "http://x").unwrap();
        assert!(ev.end_time.is_none());
    }

    #[test]
    fn parse_config_defaults_to_enabled_when_field_missing() {
        // A minimal entry without an explicit `enabled` field should
        // still surface as a candidate (Frigate defaults enabled=true).
        let blob = json!({
            "cameras": { "garage": { "ffmpeg": { "inputs": [ { "path": "rtsp://x" } ] } } }
        });
        let out = parse_config(&blob, "http://x");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].details.get("enabled").and_then(|v| v.as_bool()), Some(true));
    }
}
