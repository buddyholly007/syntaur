//! COMMAND_CLASS_NOTIFICATION (0x71) — Report parsing for V3+.
//!
//! Notifications carry the events door/window sensors, motion sensors,
//! smoke alarms, tamper switches etc. push to the controller. We parse
//! the V3+ report (the common case on modern devices) and leave the
//! pre-V3 legacy "Alarm" fields exposed raw for callers who care.
//!
//! V3+ Report layout:
//!
//! ```text
//!   [0x71, 0x05,
//!    v1_alarm_type,           // zero on V3+ devices
//!    v1_alarm_level,          // zero on V3+ devices
//!    reserved_or_zensornet,
//!    status,                  // 0xFF = enabled, 0x00 = disabled/idle
//!    notification_type,
//!    event,
//!    event_parameters_length,
//!    ...event_parameters]
//! ```
//!
//! Notable notification types (spec §4.65):
//!   0x06 = Access Control (door/window) — event 0x16 door open, 0x17 close
//!   0x07 = Home Security — event 0x08 motion detected, 0x03 tamper
//!   0x08 = Power Management — 0x01 power applied, 0x05 AC disconnected
//!   0x0E = Siren — 0x01 siren active

use super::CC_NOTIFICATION;

pub const CMD_GET: u8 = 0x04;
pub const CMD_REPORT: u8 = 0x05;

pub const TYPE_ACCESS_CONTROL: u8 = 0x06;
pub const TYPE_HOME_SECURITY: u8 = 0x07;
pub const TYPE_POWER_MANAGEMENT: u8 = 0x08;
pub const TYPE_EMERGENCY: u8 = 0x0A;
pub const TYPE_SIREN: u8 = 0x0E;

// Common event codes within TYPE_ACCESS_CONTROL.
pub const EVENT_DOOR_OPEN: u8 = 0x16;
pub const EVENT_DOOR_CLOSED: u8 = 0x17;

// Common event codes within TYPE_HOME_SECURITY.
pub const EVENT_MOTION_DETECTED: u8 = 0x08;
pub const EVENT_TAMPER: u8 = 0x03;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// V1 legacy alarm type. Zero when the device is V3+ and using the
    /// notification fields below.
    pub v1_alarm_type: u8,
    pub v1_alarm_level: u8,
    /// 0xFF = notification enabled / event active; 0x00 = idle / cleared.
    pub status: u8,
    pub notification_type: u8,
    pub event: u8,
    /// Raw event-specific parameters. For door sensors this is often
    /// empty; for some Fibaro/Aeotec devices it carries a user code.
    pub event_parameters: Vec<u8>,
}

impl Notification {
    /// Convenience: "is this an active state" — non-idle status + any
    /// non-zero event id.
    pub fn is_active(&self) -> bool {
        self.status != 0x00 && self.event != 0x00
    }

    /// Convenience: does the notification indicate a door/window opened?
    pub fn is_door_opened(&self) -> bool {
        self.notification_type == TYPE_ACCESS_CONTROL && self.event == EVENT_DOOR_OPEN
    }

    /// Convenience: motion detected?
    pub fn is_motion_detected(&self) -> bool {
        self.notification_type == TYPE_HOME_SECURITY && self.event == EVENT_MOTION_DETECTED
    }
}

pub struct NotificationCc;

impl NotificationCc {
    pub fn encode_get(notification_type: u8, event: u8) -> Vec<u8> {
        vec![
            CC_NOTIFICATION,
            CMD_GET,
            // V2 Get layout: v1_alarm_type, notification_type, event.
            // We hardcode v1_alarm_type=0 since V3+ devices ignore it.
            0x00,
            notification_type,
            event,
        ]
    }

    /// Parse a V3+ Notification Report. Rejects anything shorter than
    /// the minimum 9-byte layout.
    pub fn parse_report(payload: &[u8]) -> Option<Notification> {
        if payload.len() < 9 {
            return None;
        }
        if payload[0] != CC_NOTIFICATION || payload[1] != CMD_REPORT {
            return None;
        }
        let v1_alarm_type = payload[2];
        let v1_alarm_level = payload[3];
        // payload[4] is reserved / zensor_net_node_id on ancient kit — ignore.
        let status = payload[5];
        let notification_type = payload[6];
        let event = payload[7];
        let params_len = payload[8] as usize;
        if payload.len() < 9 + params_len {
            return None;
        }
        let event_parameters = payload[9..9 + params_len].to_vec();
        Some(Notification {
            v1_alarm_type,
            v1_alarm_level,
            status,
            notification_type,
            event,
            event_parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_get_v2_layout() {
        assert_eq!(
            NotificationCc::encode_get(TYPE_ACCESS_CONTROL, EVENT_DOOR_OPEN),
            vec![0x71, 0x04, 0x00, 0x06, 0x16]
        );
    }

    #[test]
    fn parse_report_door_opened() {
        //   [0x71, 0x05, 0x00, 0x00, 0x00, 0xFF, 0x06, 0x16, 0x00]
        //     cc    cmd  v1_at v1_al rsv   stat  ntyp   evt  paramLen
        let n = NotificationCc::parse_report(&[
            0x71, 0x05, 0x00, 0x00, 0x00, 0xFF, 0x06, 0x16, 0x00,
        ])
        .expect("report");
        assert_eq!(n.notification_type, TYPE_ACCESS_CONTROL);
        assert_eq!(n.event, EVENT_DOOR_OPEN);
        assert!(n.is_door_opened());
        assert!(n.is_active());
        assert!(n.event_parameters.is_empty());
    }

    #[test]
    fn parse_report_motion_with_params() {
        let n = NotificationCc::parse_report(&[
            0x71, 0x05, 0x00, 0x00, 0x00, 0xFF, 0x07, 0x08, 0x02, 0xAA, 0xBB,
        ])
        .expect("report");
        assert!(n.is_motion_detected());
        assert_eq!(n.event_parameters, vec![0xAA, 0xBB]);
    }

    #[test]
    fn parse_report_idle_when_status_zero() {
        let n = NotificationCc::parse_report(&[
            0x71, 0x05, 0x00, 0x00, 0x00, 0x00, 0x06, 0x17, 0x00,
        ])
        .expect("report");
        // Spec: status 0x00 indicates "notification not active" / cleared.
        assert!(!n.is_active());
        assert_eq!(n.event, EVENT_DOOR_CLOSED);
    }

    #[test]
    fn parse_report_rejects_short_payload() {
        assert!(NotificationCc::parse_report(&[0x71, 0x05, 0x00]).is_none());
    }

    #[test]
    fn parse_report_rejects_inconsistent_params_len() {
        // Says 5 params but only 2 bytes follow.
        assert!(NotificationCc::parse_report(&[
            0x71, 0x05, 0x00, 0x00, 0x00, 0xFF, 0x07, 0x08, 0x05, 0xAA, 0xBB
        ])
        .is_none());
    }
}
