//! COMMAND_CLASS_SWITCH_MULTILEVEL (0x26).
//!
//! Set: drive the node to a target level (0-99 or 0xFF = "last known").
//! Get: read the current level.
//! Report: Get response or unsolicited — current (+ V4 target + duration).
//!
//! Byte layout:
//!
//! ```text
//!   Set    = [0x26, 0x01, value]                    // V1
//!         = [0x26, 0x01, value, duration]          // V2+  duration: 0x00=instant, 0x01..=0x7F sec, 0x80..=0xFE min
//!   Get    = [0x26, 0x02]
//!   Report = [0x26, 0x03, current]                  // V1
//!         = [0x26, 0x03, current, target, duration] // V4+
//! ```

use super::{clamp_level_percent, level_byte_to_percent, CC_SWITCH_MULTILEVEL, VALUE_LAST_LEVEL};

pub const CMD_SET: u8 = 0x01;
pub const CMD_GET: u8 = 0x02;
pub const CMD_REPORT: u8 = 0x03;

/// Duration sentinel: 0x00 = instant, 0x01..=0x7F = seconds,
/// 0x80..=0xFE = minutes (0x80 == 1 minute, 0xFE == 127 minutes),
/// 0xFF = spec-undefined, we treat as "default".
pub const DURATION_INSTANT: u8 = 0x00;
pub const DURATION_DEFAULT: u8 = 0xFF;

/// Typed Report payload. `current_percent` folds the spec sentinels
/// into a clean 0..=100 scale for UI display (see
/// `super::level_byte_to_percent`); the raw byte is preserved as
/// `current_raw` for edge cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchMultilevelReport {
    pub current_raw: u8,
    pub current_percent: u8,
    pub target_raw: Option<u8>,
    pub duration: Option<u8>,
}

pub struct SwitchMultilevelCc;

impl SwitchMultilevelCc {
    /// Encode `Set` at the CC level. `percent` is clamped to 0..=99
    /// (the spec's brightness range); pass `VALUE_LAST_LEVEL` (0xFF)
    /// directly via `encode_set_raw` to request "restore last-known".
    pub fn encode_set(percent: u8, duration: Option<u8>) -> Vec<u8> {
        Self::encode_set_raw(clamp_level_percent(percent), duration)
    }

    /// Encode `Set` with the "restore last-known level" sentinel. Useful
    /// for scenes where the user wants the bulb to come back to its
    /// prior brightness after a stretch of being off.
    pub fn encode_set_restore_last(duration: Option<u8>) -> Vec<u8> {
        Self::encode_set_raw(VALUE_LAST_LEVEL, duration)
    }

    /// Low-level: send the exact `value` byte without clamping. Use
    /// this when the caller knows it's a reserved sentinel the clamp
    /// helper would mangle.
    pub fn encode_set_raw(value: u8, duration: Option<u8>) -> Vec<u8> {
        let mut v = vec![CC_SWITCH_MULTILEVEL, CMD_SET, value];
        if let Some(d) = duration {
            v.push(d);
        }
        v
    }

    pub fn encode_get() -> Vec<u8> {
        vec![CC_SWITCH_MULTILEVEL, CMD_GET]
    }

    pub fn parse_report(payload: &[u8]) -> Option<SwitchMultilevelReport> {
        if payload.len() < 3 {
            return None;
        }
        if payload[0] != CC_SWITCH_MULTILEVEL || payload[1] != CMD_REPORT {
            return None;
        }
        let current_raw = payload[2];
        let (target_raw, duration) = if payload.len() >= 5 {
            (Some(payload[3]), Some(payload[4]))
        } else {
            (None, None)
        };
        Some(SwitchMultilevelReport {
            current_raw,
            current_percent: level_byte_to_percent(current_raw),
            target_raw,
            duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_set_clamps_percent_to_99() {
        assert_eq!(
            SwitchMultilevelCc::encode_set(50, None),
            vec![0x26, 0x01, 50]
        );
        assert_eq!(
            SwitchMultilevelCc::encode_set(100, None),
            vec![0x26, 0x01, 99]
        );
        assert_eq!(
            SwitchMultilevelCc::encode_set(0, None),
            vec![0x26, 0x01, 0]
        );
    }

    #[test]
    fn encode_set_emits_duration_byte_when_supplied() {
        assert_eq!(
            SwitchMultilevelCc::encode_set(40, Some(DURATION_INSTANT)),
            vec![0x26, 0x01, 40, 0x00]
        );
        // 5-second transition.
        assert_eq!(
            SwitchMultilevelCc::encode_set(80, Some(0x05)),
            vec![0x26, 0x01, 80, 0x05]
        );
    }

    #[test]
    fn encode_set_restore_last_uses_ff_sentinel() {
        assert_eq!(
            SwitchMultilevelCc::encode_set_restore_last(None),
            vec![0x26, 0x01, 0xFF]
        );
    }

    #[test]
    fn parse_report_v1_single_byte() {
        let r = SwitchMultilevelCc::parse_report(&[0x26, 0x03, 50])
            .expect("v1 report");
        assert_eq!(r.current_raw, 50);
        assert_eq!(r.current_percent, 50);
        assert_eq!(r.target_raw, None);
        assert_eq!(r.duration, None);
    }

    #[test]
    fn parse_report_v4_includes_target_and_duration() {
        let r = SwitchMultilevelCc::parse_report(&[0x26, 0x03, 20, 60, 0x0A])
            .expect("v4 report");
        assert_eq!(r.current_percent, 20);
        assert_eq!(r.target_raw, Some(60));
        assert_eq!(r.duration, Some(0x0A));
    }

    #[test]
    fn parse_report_off_and_on_sentinels() {
        // 0x00 → percent 0
        let off = SwitchMultilevelCc::parse_report(&[0x26, 0x03, 0x00]).expect("off");
        assert_eq!(off.current_percent, 0);
        // 0xFF → "fully on" via level_byte_to_percent = 100
        let on = SwitchMultilevelCc::parse_report(&[0x26, 0x03, 0xFF]).expect("on");
        assert_eq!(on.current_percent, 100);
    }

    #[test]
    fn parse_report_rejects_wrong_cc_or_command() {
        assert!(SwitchMultilevelCc::parse_report(&[0x25, 0x03, 0x00]).is_none());
        assert!(SwitchMultilevelCc::parse_report(&[0x26, 0x01, 0x00]).is_none());
    }
}
