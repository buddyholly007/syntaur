//! Dialect-neutral command vocabulary.
//!
//! The automation engine, the API control handler, and the future
//! voice-assistant all speak the same `MqttCommand` enum. Each
//! [`super::dialects::Dialect`] translates from this shared vocabulary
//! into its native wire format via `Dialect::encode_command`, so adding
//! a new MQTT flavor doesn't require touching the dispatch layer.
//!
//! v1 keeps the vocabulary small on purpose — enough to drive on/off,
//! lighting, climate, and covers, plus a `Raw(Value)` escape hatch for
//! dialect-specific richness that doesn't fit neatly into a type. Add
//! new variants only after two dialects independently need them.

use rumqttc::QoS;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Dialect-neutral control intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MqttCommand {
    /// Turn on/off. Maps to `ON`/`OFF` for most dialects, `{"state":"ON"}`
    /// for Z2M, `{"method":"Switch.Set"}` for Shelly Gen2.
    SetOn(bool),
    /// Brightness 0..=255 (Z2M/MQTT native). Dialects scale to their
    /// own range as needed. Matches the Z2M and Tasmota default.
    SetBrightness(u8),
    /// Color temperature in mireds. Preserves the unit used by Z2M and
    /// HA Discovery — dialects that want Kelvin convert via
    /// `kelvin = 1_000_000 / mireds`.
    SetColorTempMireds(u16),
    /// Color in RGB 0..=255.
    SetColorRgb(u8, u8, u8),
    /// Thermostat target temperature in °C. Dialects handle unit
    /// conversion if the device wants °F.
    SetTargetTemp(f32),
    /// HVAC mode — "heat" | "cool" | "off" | "auto" | "dry" | "fan_only".
    SetHvacMode(String),
    /// Cover position 0..=100 (0 = closed, 100 = open).
    SetCoverPosition(u8),
    /// Escape hatch for anything not captured above. Dialect-specific
    /// payload. Avoid unless adding a proper variant would over-fit
    /// the vocabulary.
    Raw(Value),
}

/// Output of `Dialect::encode_command`. The caller publishes this on
/// the session's broker.
#[derive(Debug, Clone)]
pub struct EncodedCommand {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: QoS,
    pub retain: bool,
}

impl EncodedCommand {
    /// QoS 1, non-retained — the plan's rule (never QoS 2; retention
    /// on command topics breaks replay after reconnect). Sole
    /// constructor so no caller accidentally flips the retain bit.
    pub fn new(topic: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            topic: topic.into(),
            payload: payload.into(),
            qos: QoS::AtLeastOnce,
            retain: false,
        }
    }
}

/// Shallow interpretation of a state-patch `Value` (the kind
/// `handle_control` and `automation::Action::SetDevice` both publish)
/// into zero-or-more `MqttCommand`s. Order matters for lighting — on
/// first, then level, then color — so a `{on:false, brightness:0}`
/// patch fires an explicit off before the brightness goes through.
///
/// Unknown keys are ignored silently rather than erroring: the state
/// surface is expanding and dialects should see only what they
/// understand.
pub fn commands_from_state_patch(state: &Value) -> Vec<MqttCommand> {
    let Some(obj) = state.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    if let Some(on) = obj.get("on").and_then(|v| v.as_bool()) {
        out.push(MqttCommand::SetOn(on));
    }

    // Accept both "brightness" (0..=255 native) and "level" (0..=100
    // scaled — used by HA Discovery and Matter cluster docs). "level"
    // may be 0..=100 or a float 0..=1.0.
    if let Some(b) = obj.get("brightness").and_then(|v| v.as_u64()) {
        out.push(MqttCommand::SetBrightness(b.min(255) as u8));
    } else if let Some(level) = obj.get("level").and_then(|v| v.as_f64()) {
        let scaled = if level <= 1.0 {
            (level * 255.0).round() as u32
        } else {
            ((level / 100.0) * 255.0).round() as u32
        };
        out.push(MqttCommand::SetBrightness(scaled.min(255) as u8));
    }

    if let Some(mireds) = obj.get("color_temp_mireds").and_then(|v| v.as_u64()) {
        out.push(MqttCommand::SetColorTempMireds(mireds.min(u16::MAX as u64) as u16));
    } else if let Some(kelvin) = obj.get("color_temp_kelvin").and_then(|v| v.as_u64()) {
        if kelvin > 0 {
            let mireds = (1_000_000_u64 / kelvin).min(u16::MAX as u64) as u16;
            out.push(MqttCommand::SetColorTempMireds(mireds));
        }
    }

    if let Some(rgb) = obj.get("rgb").and_then(|v| v.as_array()) {
        if rgb.len() == 3 {
            let c = |i: usize| rgb[i].as_u64().unwrap_or(0).min(255) as u8;
            out.push(MqttCommand::SetColorRgb(c(0), c(1), c(2)));
        }
    }

    if let Some(t) = obj.get("target_temp").and_then(|v| v.as_f64()) {
        out.push(MqttCommand::SetTargetTemp(t as f32));
    }
    if let Some(mode) = obj.get("hvac_mode").and_then(|v| v.as_str()) {
        out.push(MqttCommand::SetHvacMode(mode.to_string()));
    }

    if let Some(p) = obj.get("cover_position").and_then(|v| v.as_u64()) {
        out.push(MqttCommand::SetCoverPosition(p.min(100) as u8));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn on_maps_to_set_on() {
        let cmds = commands_from_state_patch(&json!({"on": true}));
        assert!(matches!(cmds.as_slice(), [MqttCommand::SetOn(true)]));
    }

    #[test]
    fn brightness_maps_native_range() {
        let cmds = commands_from_state_patch(&json!({"brightness": 180}));
        assert!(matches!(
            cmds.as_slice(),
            [MqttCommand::SetBrightness(180)]
        ));
    }

    #[test]
    fn level_percent_scales_to_255() {
        let cmds = commands_from_state_patch(&json!({"level": 50}));
        // 50% → 127.5 → 128 rounded
        assert!(matches!(cmds.as_slice(), [MqttCommand::SetBrightness(128)]));
    }

    #[test]
    fn level_fraction_scales_to_255() {
        let cmds = commands_from_state_patch(&json!({"level": 0.5}));
        assert!(matches!(cmds.as_slice(), [MqttCommand::SetBrightness(128)]));
    }

    #[test]
    fn color_temp_kelvin_converts_to_mireds() {
        // 2700K → ~370 mireds
        let cmds = commands_from_state_patch(&json!({"color_temp_kelvin": 2700}));
        match cmds.as_slice() {
            [MqttCommand::SetColorTempMireds(m)] => {
                assert!((*m as i32 - 370).abs() <= 1);
            }
            other => panic!("expected SetColorTempMireds, got {:?}", other),
        }
    }

    #[test]
    fn rgb_array_maps_to_rgb() {
        let cmds = commands_from_state_patch(&json!({"rgb": [255, 128, 0]}));
        assert!(matches!(
            cmds.as_slice(),
            [MqttCommand::SetColorRgb(255, 128, 0)]
        ));
    }

    #[test]
    fn patch_with_on_and_brightness_emits_both_in_order() {
        let cmds = commands_from_state_patch(&json!({"on": true, "brightness": 200}));
        assert!(matches!(
            cmds.as_slice(),
            [MqttCommand::SetOn(true), MqttCommand::SetBrightness(200)]
        ));
    }

    #[test]
    fn cover_position_clamps_to_100() {
        let cmds = commands_from_state_patch(&json!({"cover_position": 250}));
        assert!(matches!(
            cmds.as_slice(),
            [MqttCommand::SetCoverPosition(100)]
        ));
    }

    #[test]
    fn empty_patch_emits_nothing() {
        let cmds = commands_from_state_patch(&json!({}));
        assert!(cmds.is_empty());
    }

    #[test]
    fn encoded_command_is_qos1_non_retained() {
        let e = EncodedCommand::new("cmnd/plug/POWER", b"ON".to_vec());
        assert_eq!(e.qos, QoS::AtLeastOnce);
        assert!(!e.retain);
    }
}
