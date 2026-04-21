//! COMMAND_CLASS_SWITCH_BINARY (0x25).
//!
//! Set: toggle a binary switch on or off.
//! Get: read the current state.
//! Report: unsolicited or Get-response — the node's current value.
//!
//! Byte layout (all commands start with [CC, command_id]):
//!
//! ```text
//!   Set     = [0x25, 0x01, value]          // value: 0x00=off, 0xFF=on
//!   Get     = [0x25, 0x02]
//!   Report  = [0x25, 0x03, value]          // V1
//!           = [0x25, 0x03, current, target, duration] // V2+
//! ```

use super::{CC_SWITCH_BINARY, VALUE_OFF, VALUE_ON};

pub const CMD_SET: u8 = 0x01;
pub const CMD_GET: u8 = 0x02;
pub const CMD_REPORT: u8 = 0x03;

/// Typed SwitchBinary Report payload. `current` is authoritative for the
/// UI; `target` + `duration` describe an in-flight transition (V2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchBinaryReport {
    pub current: bool,
    /// Some(bool) when the node reports a target different from the
    /// current value — i.e. a transition is in flight. V1 devices
    /// always return None here.
    pub target: Option<bool>,
    /// Transition duration byte if the node reported one (V2+). 0x00 =
    /// instant, 0x01..=0x7F = seconds, 0x80..=0xFE = minutes, 0xFE
    /// sentinel = "unknown".
    pub duration: Option<u8>,
}

/// Zero-sized helper namespace for the CC. Lets callers write
/// `SwitchBinaryCc::encode_set(true)` etc. without a runtime struct.
pub struct SwitchBinaryCc;

impl SwitchBinaryCc {
    /// Encode `Set` — the command-class payload the caller hands to
    /// `Controller::send_data`.
    pub fn encode_set(on: bool) -> Vec<u8> {
        vec![
            CC_SWITCH_BINARY,
            CMD_SET,
            if on { VALUE_ON } else { VALUE_OFF },
        ]
    }

    pub fn encode_get() -> Vec<u8> {
        vec![CC_SWITCH_BINARY, CMD_GET]
    }

    /// Parse a Report coming back from the node. Accepts both V1
    /// (1-byte payload) and V2+ (3-byte payload) shapes.
    ///
    /// `payload` is the full Report including CC + command bytes (so
    /// whatever lives inside the SendData frame). `decode_value` treats
    /// 0x00 as off and anything else as on — matches the spec's
    /// "any-nonzero" rule for BasicValue.
    pub fn parse_report(payload: &[u8]) -> Option<SwitchBinaryReport> {
        if payload.len() < 3 {
            return None;
        }
        if payload[0] != CC_SWITCH_BINARY || payload[1] != CMD_REPORT {
            return None;
        }
        let current = decode_value(payload[2]);
        let (target, duration) = if payload.len() >= 5 {
            (Some(decode_value(payload[3])), Some(payload[4]))
        } else {
            (None, None)
        };
        Some(SwitchBinaryReport {
            current,
            target,
            duration,
        })
    }
}

fn decode_value(b: u8) -> bool {
    b != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_set_on_has_0xff_sentinel() {
        assert_eq!(SwitchBinaryCc::encode_set(true), vec![0x25, 0x01, 0xFF]);
        assert_eq!(SwitchBinaryCc::encode_set(false), vec![0x25, 0x01, 0x00]);
    }

    #[test]
    fn encode_get_is_two_bytes() {
        assert_eq!(SwitchBinaryCc::encode_get(), vec![0x25, 0x02]);
    }

    #[test]
    fn parse_report_v1_one_byte_value() {
        let r = SwitchBinaryCc::parse_report(&[0x25, 0x03, 0xFF]).expect("v1 report");
        assert!(r.current);
        assert_eq!(r.target, None);
        assert_eq!(r.duration, None);
    }

    #[test]
    fn parse_report_v2_includes_target_and_duration() {
        // Spec V2+ layout — value 0, target 255, 10-second transition.
        let r = SwitchBinaryCc::parse_report(&[0x25, 0x03, 0x00, 0xFF, 0x0A])
            .expect("v2 report");
        assert!(!r.current);
        assert_eq!(r.target, Some(true));
        assert_eq!(r.duration, Some(0x0A));
    }

    #[test]
    fn parse_report_rejects_wrong_cc() {
        assert!(SwitchBinaryCc::parse_report(&[0x99, 0x03, 0xFF]).is_none());
    }

    #[test]
    fn parse_report_rejects_non_report_command() {
        assert!(SwitchBinaryCc::parse_report(&[0x25, 0x01, 0xFF]).is_none());
    }

    #[test]
    fn parse_report_rejects_short_payload() {
        assert!(SwitchBinaryCc::parse_report(&[0x25, 0x03]).is_none());
    }

    #[test]
    fn decode_value_any_nonzero_is_on() {
        assert!(!decode_value(0x00));
        assert!(decode_value(0x01));
        assert!(decode_value(0x5A));
        assert!(decode_value(0xFF));
    }
}
