//! Z-Wave Command Classes (CCs) — typed encoders + decoders for the
//! application-layer messages that ride inside a `FUNC_ZW_SEND_DATA`
//! payload.
//!
//! Each CC submodule owns its command ids + Set/Get/Report byte layout
//! and exposes a small `Cc` struct with typed `encode_*` + `parse_*`
//! helpers. Controller ergonomic wrappers (e.g.
//! `Controller::switch_binary_set`) live on `Controller<T>` and delegate
//! to these modules, so callers can either call a high-level helper or
//! drop down to the raw bytes when they need something custom.
//!
//! Layout convention:
//! ```text
//!   [command_class_id] [command_id] [param_bytes ...]
//! ```
//!
//! Every CC below matches this. Higher-layer protocols (SendData
//! wrapping, MultiChannel routing, Security encapsulation) live
//! elsewhere.

pub mod door_lock;
pub mod meter;
pub mod notification;
pub mod sensor_multilevel;
pub mod switch_binary;
pub mod switch_multilevel;
pub mod thermostat_mode;
pub mod thermostat_setpoint;

pub use door_lock::DoorLockCc;
pub use meter::MeterCc;
pub use notification::NotificationCc;
pub use sensor_multilevel::SensorMultilevelCc;
pub use switch_binary::SwitchBinaryCc;
pub use switch_multilevel::SwitchMultilevelCc;
pub use thermostat_mode::ThermostatModeCc;
pub use thermostat_setpoint::ThermostatSetpointCc;

// ── Command class identifiers ───────────────────────────────────────────

pub const CC_SWITCH_BINARY: u8 = 0x25;
pub const CC_SWITCH_MULTILEVEL: u8 = 0x26;
pub const CC_SENSOR_MULTILEVEL: u8 = 0x31;
pub const CC_METER: u8 = 0x32;
pub const CC_THERMOSTAT_MODE: u8 = 0x40;
pub const CC_THERMOSTAT_SETPOINT: u8 = 0x43;
pub const CC_DOOR_LOCK: u8 = 0x62;
pub const CC_NOTIFICATION: u8 = 0x71;

// ── Value conventions shared across CCs ─────────────────────────────────

/// Z-Wave BasicValue / SwitchBinaryValue / SwitchMultilevelValue — the
/// spec reserves the full 0..=0xFF byte with well-known sentinels.
///   0x00           — off / level 0%
///   0x01..=0x63    — brightness % (1..99)
///   0x64..=0xFE    — reserved
///   0xFF           — on, or "restore last-known level" for multilevel
pub const VALUE_OFF: u8 = 0x00;
pub const VALUE_ON: u8 = 0xFF;
pub const VALUE_LAST_LEVEL: u8 = 0xFF;
pub const MAX_LEVEL_PERCENT: u8 = 99;

/// Clamp a user-supplied 0..=100 percent level to the 0..=99 byte the
/// spec uses for SwitchMultilevel. 100 becomes 99 (the spec max);
/// anything above is saturated. Kept public so other CCs that share
/// the same byte convention (e.g. SceneActuatorConf) can reuse it.
pub fn clamp_level_percent(pct: u8) -> u8 {
    pct.min(MAX_LEVEL_PERCENT)
}

/// Decode the wire byte back into "percent on" for UI display. The
/// inverse of `clamp_level_percent` on the sane range, with the two
/// sentinels folded in:
///   0x00            → 0
///   0x01..=0x63     → N
///   0xFF            → 100 (we don't keep "last known" state here)
///   anything else   → 0 (reserved — defensive)
pub fn level_byte_to_percent(b: u8) -> u8 {
    match b {
        0x00 => 0,
        0x01..=0x63 => b,
        0xFF => 100,
        _ => 0,
    }
}

// ── Shared numeric encoding (SensorMultilevel, Meter, ...) ──────────────
//
// Both CCs prefix their value with a single "level byte" packing
// precision (how many decimal places), scale (which unit within the
// CC's scale set), and size (1, 2, or 4 bytes of signed integer):
//
// ```text
//   bits 7-5  precision    0..=7  → value / 10^precision
//   bits 4-3  scale        0..=3
//   bits 2-0  size         1, 2, or 4
// ```
//
// Value bytes are signed big-endian, interpreted as the given size.

/// Parsed numeric value from a precision/scale/size-encoded payload.
/// `value` is the sign-correct f64 after dividing by 10^precision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodedValue {
    pub value: f64,
    pub scale: u8,
    pub precision: u8,
    pub size: u8,
}

/// Parse `[level_byte, ...value_bytes]` into an `EncodedValue`. Returns
/// (EncodedValue, bytes_consumed_including_level_byte) so callers that
/// sit atop a larger payload (Meter V2+ trailing delta_time +
/// previous_value) can advance.
pub fn parse_encoded_value(bytes: &[u8]) -> Option<(EncodedValue, usize)> {
    if bytes.is_empty() {
        return None;
    }
    let level = bytes[0];
    let precision = (level >> 5) & 0x07;
    let scale = (level >> 3) & 0x03;
    let size = level & 0x07;
    if size != 1 && size != 2 && size != 4 {
        return None; // spec-invalid size
    }
    if bytes.len() < 1 + size as usize {
        return None;
    }
    let raw_bytes = &bytes[1..1 + size as usize];
    let signed: i64 = match size {
        1 => raw_bytes[0] as i8 as i64,
        2 => i16::from_be_bytes([raw_bytes[0], raw_bytes[1]]) as i64,
        4 => i32::from_be_bytes([
            raw_bytes[0],
            raw_bytes[1],
            raw_bytes[2],
            raw_bytes[3],
        ]) as i64,
        _ => unreachable!(),
    };
    let divisor = 10f64.powi(precision as i32);
    let value = signed as f64 / divisor;
    Some((
        EncodedValue {
            value,
            scale,
            precision,
            size,
        },
        1 + size as usize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_level_percent_caps_at_99() {
        assert_eq!(clamp_level_percent(0), 0);
        assert_eq!(clamp_level_percent(50), 50);
        assert_eq!(clamp_level_percent(99), 99);
        assert_eq!(clamp_level_percent(100), 99);
        assert_eq!(clamp_level_percent(255), 99);
    }

    #[test]
    fn parse_encoded_value_1byte_signed() {
        // level byte: precision=1 (<<5), scale=0, size=1 → 0x21
        // payload: signed 8-bit 0xF6 = -10, so value = -10 / 10^1 = -1.0
        let (ev, n) = parse_encoded_value(&[0x21, 0xF6]).expect("parse");
        assert_eq!(ev.value, -1.0);
        assert_eq!(ev.scale, 0);
        assert_eq!(ev.precision, 1);
        assert_eq!(ev.size, 1);
        assert_eq!(n, 2);
    }

    #[test]
    fn parse_encoded_value_2byte_temperature() {
        // 21.5°C: precision=1, scale=0 (°C), size=2 → 0x22
        // raw = 215 → 0x00D7
        let (ev, n) = parse_encoded_value(&[0x22, 0x00, 0xD7]).expect("parse");
        assert!((ev.value - 21.5).abs() < 1e-9);
        assert_eq!(ev.scale, 0);
        assert_eq!(n, 3);
    }

    #[test]
    fn parse_encoded_value_4byte() {
        // precision=2, scale=1, size=4 → 0x4C
        // raw = 12_345 → value = 123.45
        let (ev, _n) = parse_encoded_value(&[0x4C, 0x00, 0x00, 0x30, 0x39]).expect("parse");
        assert!((ev.value - 123.45).abs() < 1e-6);
        assert_eq!(ev.scale, 1);
    }

    #[test]
    fn parse_encoded_value_rejects_invalid_size() {
        assert!(parse_encoded_value(&[0x03, 0x00, 0x00, 0x00]).is_none()); // size=3
        assert!(parse_encoded_value(&[0x05, 0x00]).is_none()); // size=5
    }

    #[test]
    fn parse_encoded_value_rejects_short_payload() {
        // size=4 but only 2 value bytes follow
        assert!(parse_encoded_value(&[0x04, 0x00, 0x00]).is_none());
    }

    #[test]
    fn level_byte_to_percent_handles_sentinels() {
        assert_eq!(level_byte_to_percent(0x00), 0);
        assert_eq!(level_byte_to_percent(42), 42);
        assert_eq!(level_byte_to_percent(0x63), 99);
        assert_eq!(level_byte_to_percent(0xFF), 100);
        // Reserved range (0x64..=0xFE) defensively collapses to 0.
        assert_eq!(level_byte_to_percent(0xA0), 0);
    }
}
