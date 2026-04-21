//! COMMAND_CLASS_THERMOSTAT_MODE (0x40).
//!
//! Set / read the thermostat's operating mode (off, heat, cool, auto…).
//! Pairs with ThermostatSetpoint for the actual target-temperature value.
//!
//! ```text
//!   Set    = [0x40, 0x01, mode]
//!   Get    = [0x40, 0x02]
//!   Report = [0x40, 0x03, mode]
//! ```

use super::CC_THERMOSTAT_MODE;

pub const CMD_SET: u8 = 0x01;
pub const CMD_GET: u8 = 0x02;
pub const CMD_REPORT: u8 = 0x03;

pub const MODE_OFF: u8 = 0x00;
pub const MODE_HEAT: u8 = 0x01;
pub const MODE_COOL: u8 = 0x02;
pub const MODE_AUTO: u8 = 0x03;
pub const MODE_AUX_HEAT: u8 = 0x04;
pub const MODE_RESUME_ON: u8 = 0x05;
pub const MODE_FAN_ONLY: u8 = 0x06;
pub const MODE_FURNACE: u8 = 0x07;
pub const MODE_DRY_AIR: u8 = 0x08;
pub const MODE_MOIST_AIR: u8 = 0x09;
pub const MODE_AUTO_CHANGEOVER: u8 = 0x0A;
pub const MODE_ENERGY_SAVE_HEAT: u8 = 0x0B;
pub const MODE_ENERGY_SAVE_COOL: u8 = 0x0C;
pub const MODE_AWAY: u8 = 0x0D;

pub struct ThermostatModeCc;

impl ThermostatModeCc {
    pub fn encode_set(mode: u8) -> Vec<u8> {
        vec![CC_THERMOSTAT_MODE, CMD_SET, mode & 0x1F]
    }

    pub fn encode_get() -> Vec<u8> {
        vec![CC_THERMOSTAT_MODE, CMD_GET]
    }

    /// Parse Report. Returns the raw mode byte (caller matches against
    /// `MODE_*` constants); the mode field in the spec is a 5-bit value
    /// in the low bits of byte 2, but most firmwares publish the full
    /// byte — we mask to 0x1F for safety.
    pub fn parse_report(payload: &[u8]) -> Option<u8> {
        if payload.len() < 3 || payload[0] != CC_THERMOSTAT_MODE || payload[1] != CMD_REPORT {
            return None;
        }
        Some(payload[2] & 0x1F)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_set_emits_three_bytes() {
        assert_eq!(ThermostatModeCc::encode_set(MODE_HEAT), vec![0x40, 0x01, 0x01]);
        assert_eq!(ThermostatModeCc::encode_set(MODE_OFF), vec![0x40, 0x01, 0x00]);
    }

    #[test]
    fn encode_set_masks_high_bits() {
        // 0xE3 → low 5 bits = 0x03 (auto)
        assert_eq!(
            ThermostatModeCc::encode_set(0xE3),
            vec![0x40, 0x01, 0x03]
        );
    }

    #[test]
    fn parse_report_returns_raw_mode() {
        assert_eq!(
            ThermostatModeCc::parse_report(&[0x40, 0x03, MODE_COOL]),
            Some(MODE_COOL)
        );
    }

    #[test]
    fn parse_report_rejects_wrong_cc_or_command() {
        assert!(ThermostatModeCc::parse_report(&[0x99, 0x03, 0x01]).is_none());
        assert!(ThermostatModeCc::parse_report(&[0x40, 0x01, 0x01]).is_none());
        assert!(ThermostatModeCc::parse_report(&[0x40, 0x03]).is_none());
    }
}
