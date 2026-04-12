//! Fast-path regex intent matcher.
//!
//! Before calling the LLM, run the transcript through compiled regex
//! patterns for common voice commands. If a pattern matches, execute
//! the corresponding tool directly — sub-200ms response instead of
//! the ~2-5s LLM round-trip.
//!
//! This mirrors HA's `prefer_local_intents=True` but in Rust.

use regex::Regex;
use std::sync::OnceLock;
use tracing::info;

pub struct IntentMatch {
    pub tool_name: String,
    pub args: serde_json::Value,
}

struct IntentPattern {
    pattern: Regex,
    builder: fn(&regex::Captures) -> Option<IntentMatch>,
}

fn patterns() -> &'static Vec<IntentPattern> {
    static PATTERNS: OnceLock<Vec<IntentPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // "turn on/off the X lights"
            IntentPattern {
                pattern: Regex::new(r"(?i)turn\s+(on|off)\s+(?:the\s+)?(.+?)\s+lights?$").unwrap(),
                builder: |caps| {
                    let action = if caps[1].eq_ignore_ascii_case("on") { "turn_on" } else { "turn_off" };
                    let area = caps[2].to_lowercase().replace(' ', "_");
                    Some(IntentMatch {
                        tool_name: "control_light".to_string(),
                        args: serde_json::json!({
                            "entity_id": format!("light.{}_lights", area),
                            "action": action,
                        }),
                    })
                },
            },
            // "set a N minute/second timer"
            IntentPattern {
                pattern: Regex::new(r"(?i)set\s+(?:a\s+)?(\d+)\s+(minute|second|hour)s?\s+timer(?:\s+(?:called|named|for)\s+(.+))?$").unwrap(),
                builder: |caps| {
                    let n: u64 = caps[1].parse().ok()?;
                    let unit = &caps[2];
                    let secs = match unit.to_lowercase().as_str() {
                        "second" => n,
                        "minute" => n * 60,
                        "hour" => n * 3600,
                        _ => n,
                    };
                    let name = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("timer");
                    Some(IntentMatch {
                        tool_name: "timer".to_string(),
                        args: serde_json::json!({
                            "action": "start",
                            "duration_seconds": secs,
                            "name": name,
                        }),
                    })
                },
            },
            // "what's the weather" / "what is the weather"
            IntentPattern {
                pattern: Regex::new(r"(?i)(?:what(?:'s| is) the )?weather(?:\s+in\s+(.+))?$").unwrap(),
                builder: |caps| {
                    let location = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                    Some(IntentMatch {
                        tool_name: "weather".to_string(),
                        args: serde_json::json!({"location": location}),
                    })
                },
            },
            // "add X to my shopping list"
            IntentPattern {
                pattern: Regex::new(r"(?i)add\s+(.+?)\s+to\s+(?:my\s+)?(?:the\s+)?(\w+)?\s*list$").unwrap(),
                builder: |caps| {
                    let item = caps[1].trim().to_string();
                    let list = caps.get(2).map(|m| m.as_str()).unwrap_or("shopping");
                    Some(IntentMatch {
                        tool_name: "shopping_list".to_string(),
                        args: serde_json::json!({
                            "action": "add",
                            "item": item,
                            "list_name": list,
                        }),
                    })
                },
            },
            // "pause" / "play" / "stop" (media control, no target specified)
            IntentPattern {
                pattern: Regex::new(r"(?i)^(pause|play|stop|skip|next|previous)(?:\s+(?:the\s+)?(.+))?$").unwrap(),
                builder: |caps| {
                    let lowered = caps[1].to_lowercase();
                    let action = match lowered.as_str() {
                        "skip" | "next" => "next",
                        "previous" => "previous",
                        _ => lowered.as_str(),
                    };
                    let target = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("auto");
                    Some(IntentMatch {
                        tool_name: "media_control".to_string(),
                        args: serde_json::json!({
                            "action": action,
                            "target": target,
                        }),
                    })
                },
            },
        ]
    })
}

/// Try to match a transcript against the fast-path patterns.
/// Returns Some(IntentMatch) if a pattern matched, None if the LLM should handle it.
pub fn match_intent(transcript: &str) -> Option<IntentMatch> {
    let transcript = transcript.trim();
    if transcript.is_empty() {
        return None;
    }

    for pattern in patterns() {
        if let Some(caps) = pattern.pattern.captures(transcript) {
            if let Some(m) = (pattern.builder)(&caps) {
                info!("[intent] fast match: '{}' → {}({})", transcript, m.tool_name,
                    serde_json::to_string(&m.args).unwrap_or_default());
                return Some(m);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_on() {
        let m = match_intent("turn on the kitchen lights").unwrap();
        assert_eq!(m.tool_name, "control_light");
        assert_eq!(m.args["action"], "turn_on");
        assert_eq!(m.args["entity_id"], "light.kitchen_lights");
    }

    #[test]
    fn light_off() {
        let m = match_intent("turn off the office lights").unwrap();
        assert_eq!(m.tool_name, "control_light");
        assert_eq!(m.args["action"], "turn_off");
    }

    #[test]
    fn timer_basic() {
        let m = match_intent("set a 5 minute timer").unwrap();
        assert_eq!(m.tool_name, "timer");
        assert_eq!(m.args["duration_seconds"], 300);
    }

    #[test]
    fn timer_named() {
        let m = match_intent("set a 30 second timer called chicken").unwrap();
        assert_eq!(m.tool_name, "timer");
        assert_eq!(m.args["duration_seconds"], 30);
        assert_eq!(m.args["name"], "chicken");
    }

    #[test]
    fn weather_default() {
        let m = match_intent("what's the weather").unwrap();
        assert_eq!(m.tool_name, "weather");
    }

    #[test]
    fn weather_location() {
        let m = match_intent("weather in New York").unwrap();
        assert_eq!(m.tool_name, "weather");
        assert_eq!(m.args["location"], "New York");
    }

    #[test]
    fn shopping_list() {
        let m = match_intent("add milk to my shopping list").unwrap();
        assert_eq!(m.tool_name, "shopping_list");
        assert_eq!(m.args["item"], "milk");
    }

    #[test]
    fn pause_media() {
        let m = match_intent("pause").unwrap();
        assert_eq!(m.tool_name, "media_control");
        assert_eq!(m.args["action"], "pause");
    }

    #[test]
    fn no_match() {
        assert!(match_intent("what is a quokka").is_none());
        assert!(match_intent("how are my trading bots doing").is_none());
    }
}
