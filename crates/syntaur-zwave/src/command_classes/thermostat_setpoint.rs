//! COMMAND_CLASS_THERMOSTAT_SETPOINT (0x43).
//!
//! Read / write a thermostat's target temperature for a given setpoint
//! type (heating, cooling, etc.). Value uses the shared
//! precision/scale/size encoding from `super::parse_encoded_value`.
//!
//! ```text
//!   Set    = [0x43, 0x01, setpoint_type, level_byte, value_bytes...]
//!   Get    = [0x43, 0x02, setpoint_type]
//!   Report = [0x43, 0x03, setpoint_type, level_byte, value_bytes...]
//! ```
//!
//! The `level_byte` and `value_bytes` use the same shape as
//! SensorMultilevel / Meter (precision in bits 7-5, scale in bits 4-3,
//! size in bits 2-0; value is signed big-endian at the given size).

use super::{parse_encoded_value, CC_THERMOSTAT_SETPOINT};

pub const CMD_SET: u8 = 0x01;
pub const CMD_GET: u8 = 0x02;
pub const CMD_REPORT: u8 = 0x03;

pub const SETPOINT_HEATING: u8 = 0x01;
pub const SETPOINT_COOLING: u8 = 0x02;
pub const SETPOINT_FURNACE: u8 = 0x07;
pub const SETPOINT_DRY_AIR: u8 = 0x08;
pub const SETPOINT_MOIST_AIR: u8 = 0x09;
pub const SETPOINT_AUTO_CHANGEOVER: u8 = 0x0A;
pub const SETPOINT_ENERGY_SAVE_HEATING: u8 = 0x0B;
pub const SETPOINT_ENERGY_SAVE_COOLING: u8 = 0x0C;
pub const SETPOINT_AWAY_HEATING: u8 = 0x0D;

/// Scale bits for ThermostatSetpoint: 0 = °C, 1 = °F.
pub const SCALE_CELSIUS: u8 = 0;
pub const SCALE_FAHRENHEIT: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetpointReading {
    pub setpoint_type: u8,
    pub value: f64,
    pub scale: u8,
    pub precision: u8,
    pub size: u8,
}

pub struct ThermostatSetpointCc;

impl ThermostatSetpointCc {
    /// Encode a `Set` with the typed precision/scale/size packing.
    /// `precision` is 0..=7 decimal places (1 is the norm on US
    /// thermostats: "72.0°F" → precision=1, raw=720).
    pub fn encode_set(
        setpoint_type: u8,
        value: f64,
        scale: u8,
        precision: u8,
        size: u8,
    ) -> Option<Vec<u8>> {
        if size != 1 && size != 2 && size != 4 {
            return None;
        }
        let scaled = (value * 10f64.powi(precision as i32)).round() as i64;
        let level_byte =
            ((precision & 0x07) << 5) | ((scale & 0x03) << 3) | (size & 0x07);
        let mut v = vec![CC_THERMOSTAT_SETPOINT, CMD_SET, setpoint_type, level_byte];
        match size {
            1 => v.push(scaled as i8 as u8),
            2 => v.extend_from_slice(&(scaled as i16).to_be_bytes()),
            4 => v.extend_from_slice(&(scaled as i32).to_be_bytes()),
            _ => return None,
        }
        Some(v)
    }

    /// Convenience: most UI flows just pass a Fahrenheit or Celsius
    /// integer target. Emits the V1-standard precision=1, size=2 form.
    pub fn encode_set_fahrenheit_integer(setpoint_type: u8, degrees_f: i16) -> Vec<u8> {
        // precision=1, scale=1 (°F), size=2 → level byte 0x2A
        let level_byte = (1u8 << 5) | (1u8 << 3) | 2u8;
        let raw = (degrees_f as i64 * 10) as i16;
        let mut v = vec![CC_THERMOSTAT_SETPOINT, CMD_SET, setpoint_type, level_byte];
        v.extend_from_slice(&raw.to_be_bytes());
        v
    }

    pub fn encode_get(setpoint_type: u8) -> Vec<u8> {
        vec![CC_THERMOSTAT_SETPOINT, CMD_GET, setpoint_type]
    }

    pub fn parse_report(payload: &[u8]) -> Option<SetpointReading> {
        if payload.len() < 4 {
            return None;
        }
        if payload[0] != CC_THERMOSTAT_SETPOINT || payload[1] != CMD_REPORT {
            return None;
        }
        let setpoint_type = payload[2] & 0x0F;
        let (ev, _consumed) = parse_encoded_value(&payload[3..])?;
        Some(SetpointReading {
            setpoint_type,
            value: ev.value,
            scale: ev.scale,
            precision: ev.precision,
            size: ev.size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_set_fahrenheit_72_degrees() {
        // 72°F, precision=1 → raw = 720 = 0x02D0. level byte 0x2A.
        let bytes = ThermostatSetpointCc::encode_set_fahrenheit_integer(SETPOINT_HEATING, 72);
        assert_eq!(bytes, vec![0x43, 0x01, 0x01, 0x2A, 0x02, 0xD0]);
    }

    #[test]
    fn encode_set_generic_celsius() {
        // 21.5°C at precision=1, scale=0, size=2 → raw=215=0x00D7, level byte 0x22
        let bytes =
            ThermostatSetpointCc::encode_set(SETPOINT_HEATING, 21.5, SCALE_CELSIUS, 1, 2)
                .expect("encode");
        assert_eq!(bytes, vec![0x43, 0x01, 0x01, 0x22, 0x00, 0xD7]);
    }

    #[test]
    fn encode_set_rejects_bad_size() {
        assert!(
            ThermostatSetpointCc::encode_set(SETPOINT_HEATING, 20.0, 0, 1, 3).is_none()
        );
    }

    #[test]
    fn encode_get_single_byte_setpoint_type() {
        assert_eq!(
            ThermostatSetpointCc::encode_get(SETPOINT_COOLING),
            vec![0x43, 0x02, 0x02]
        );
    }

    #[test]
    fn parse_report_heating_setpoint_fahrenheit() {
        // payload: [cc, cmd, setpoint_type=1, level=0x2A (prec=1 scale=1 size=2), 0x02, 0xD0]
        let r =
            ThermostatSetpointCc::parse_report(&[0x43, 0x03, 0x01, 0x2A, 0x02, 0xD0]).expect("r");
        assert_eq!(r.setpoint_type, SETPOINT_HEATING);
        assert!((r.value - 72.0).abs() < 1e-9);
        assert_eq!(r.scale, SCALE_FAHRENHEIT);
    }

    #[test]
    fn parse_report_rejects_bad_shape() {
        assert!(ThermostatSetpointCc::parse_report(&[0x43, 0x03, 0x01]).is_none());
        assert!(
            ThermostatSetpointCc::parse_report(&[0x43, 0x02, 0x01, 0x2A, 0x02, 0xD0])
                .is_none()
        );
    }
}
