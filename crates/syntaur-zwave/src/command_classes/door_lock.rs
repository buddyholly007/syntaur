//! COMMAND_CLASS_DOOR_LOCK (0x62) — V1/V2 subset.
//!
//! ```text
//!   OperationSet    = [0x62, 0x01, door_lock_mode]      // 0xFF=locked, 0x00=unlocked
//!   OperationGet    = [0x62, 0x02]
//!   OperationReport = [0x62, 0x03, door_lock_mode,
//!                       inside_handle_modes,
//!                       outside_handle_modes,
//!                       door_condition,
//!                       lock_timeout_min,
//!                       lock_timeout_sec]
//! ```
//!
//! `door_lock_mode`: 0x00 unsecured, 0x01 unsecured-with-timeout,
//! 0x10 inside handles unsecured, 0x11 inside handles unsecured
//! w/ timeout, 0x20 outside handles unsecured, 0x21 same w/ timeout,
//! 0xFF secured.

use super::CC_DOOR_LOCK;

pub const CMD_OPERATION_SET: u8 = 0x01;
pub const CMD_OPERATION_GET: u8 = 0x02;
pub const CMD_OPERATION_REPORT: u8 = 0x03;

pub const MODE_UNSECURED: u8 = 0x00;
pub const MODE_UNSECURED_TIMEOUT: u8 = 0x01;
pub const MODE_INSIDE_UNSECURED: u8 = 0x10;
pub const MODE_INSIDE_UNSECURED_TIMEOUT: u8 = 0x11;
pub const MODE_OUTSIDE_UNSECURED: u8 = 0x20;
pub const MODE_OUTSIDE_UNSECURED_TIMEOUT: u8 = 0x21;
pub const MODE_SECURED: u8 = 0xFF;

/// Typed OperationReport. `locked` is the boolean answer the UI wants;
/// raw fields preserved for callers doing finer analysis (e.g. Fibaro
/// Yubico-style locks that expose inside/outside handle state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoorLockReport {
    pub mode: u8,
    pub locked: bool,
    pub inside_handle_modes: u8,
    pub outside_handle_modes: u8,
    pub door_condition: u8,
    pub lock_timeout_min: u8,
    pub lock_timeout_sec: u8,
}

pub struct DoorLockCc;

impl DoorLockCc {
    /// Convenience: `encode_set(true)` → lock, `encode_set(false)` → unlock.
    pub fn encode_set(locked: bool) -> Vec<u8> {
        vec![
            CC_DOOR_LOCK,
            CMD_OPERATION_SET,
            if locked { MODE_SECURED } else { MODE_UNSECURED },
        ]
    }

    /// Low-level: hand over the exact mode byte. Use when the caller
    /// wants a handle-specific mode (e.g. unlock inside only).
    pub fn encode_set_raw(mode: u8) -> Vec<u8> {
        vec![CC_DOOR_LOCK, CMD_OPERATION_SET, mode]
    }

    pub fn encode_get() -> Vec<u8> {
        vec![CC_DOOR_LOCK, CMD_OPERATION_GET]
    }

    pub fn parse_report(payload: &[u8]) -> Option<DoorLockReport> {
        if payload.len() < 8
            || payload[0] != CC_DOOR_LOCK
            || payload[1] != CMD_OPERATION_REPORT
        {
            return None;
        }
        let mode = payload[2];
        Some(DoorLockReport {
            mode,
            locked: mode == MODE_SECURED,
            inside_handle_modes: payload[3],
            outside_handle_modes: payload[4],
            door_condition: payload[5],
            lock_timeout_min: payload[6],
            lock_timeout_sec: payload[7],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_set_lock_vs_unlock() {
        assert_eq!(DoorLockCc::encode_set(true), vec![0x62, 0x01, 0xFF]);
        assert_eq!(DoorLockCc::encode_set(false), vec![0x62, 0x01, 0x00]);
    }

    #[test]
    fn encode_get_two_bytes() {
        assert_eq!(DoorLockCc::encode_get(), vec![0x62, 0x02]);
    }

    #[test]
    fn parse_report_locked() {
        let r =
            DoorLockCc::parse_report(&[0x62, 0x03, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00])
                .expect("report");
        assert!(r.locked);
        assert_eq!(r.mode, MODE_SECURED);
    }

    #[test]
    fn parse_report_unlocked_with_timeout() {
        let r =
            DoorLockCc::parse_report(&[0x62, 0x03, 0x01, 0x00, 0x00, 0x01, 0x01, 0x1E])
                .expect("report");
        assert!(!r.locked);
        assert_eq!(r.mode, MODE_UNSECURED_TIMEOUT);
        assert_eq!(r.lock_timeout_min, 1);
        assert_eq!(r.lock_timeout_sec, 0x1E);
    }

    #[test]
    fn parse_report_rejects_short_payload() {
        assert!(DoorLockCc::parse_report(&[0x62, 0x03, 0xFF, 0, 0]).is_none());
    }

    #[test]
    fn parse_report_rejects_wrong_cc_or_command() {
        assert!(
            DoorLockCc::parse_report(&[0x99, 0x03, 0xFF, 0, 0, 0, 0, 0]).is_none()
        );
        assert!(
            DoorLockCc::parse_report(&[0x62, 0x01, 0xFF, 0, 0, 0, 0, 0]).is_none()
        );
    }
}
