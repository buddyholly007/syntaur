//! COMMAND_CLASS_SENSOR_MULTILEVEL (0x31).
//!
//! Read a physical sensor's value — temperature, humidity, power,
//! luminance, CO2, etc. The Report packs `sensor_type`, a
//! precision/scale/size "level byte", then the signed value bytes.
//!
//! ```text
//!   Get V1    = [0x31, 0x04]                          // all sensors
//!   Get V5+   = [0x31, 0x04, sensor_type, scale]      // one specific reading
//!   Report    = [0x31, 0x05, sensor_type, level_byte, value_bytes...]
//! ```

use super::{parse_encoded_value, CC_SENSOR_MULTILEVEL};

pub const CMD_GET: u8 = 0x04;
pub const CMD_REPORT: u8 = 0x05;

// Common sensor type ids from the spec. Not exhaustive — callers that
// need exotic types can match on the raw `sensor_type` byte in the
// returned report.
pub const SENSOR_TYPE_TEMPERATURE: u8 = 0x01;
pub const SENSOR_TYPE_LUMINANCE: u8 = 0x03;
pub const SENSOR_TYPE_POWER: u8 = 0x04;
pub const SENSOR_TYPE_HUMIDITY: u8 = 0x05;
pub const SENSOR_TYPE_VELOCITY: u8 = 0x06;
pub const SENSOR_TYPE_VOLTAGE: u8 = 0x0F;
pub const SENSOR_TYPE_CURRENT: u8 = 0x10;
pub const SENSOR_TYPE_CO2: u8 = 0x11;
pub const SENSOR_TYPE_TARGET_TEMPERATURE: u8 = 0x17;

/// Typed Report payload. `value` is already divided by 10^precision and
/// sign-corrected. `scale` tells you which unit: for Temperature
/// 0 = °C, 1 = °F; for Luminance 0 = %, 1 = lux; etc. — the unit set
/// is per-`sensor_type`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorReading {
    pub sensor_type: u8,
    pub value: f64,
    pub scale: u8,
    pub precision: u8,
    /// Width of the value field on the wire (1, 2, or 4 bytes). Kept
    /// so callers that want to re-serialize can round-trip.
    pub size: u8,
}

pub struct SensorMultilevelCc;

impl SensorMultilevelCc {
    /// V1 `Get` — ask the node for its default / primary sensor reading.
    pub fn encode_get() -> Vec<u8> {
        vec![CC_SENSOR_MULTILEVEL, CMD_GET]
    }

    /// V5+ `Get` — request a specific sensor_type at a specific scale.
    /// The `scale` byte's layout is `bit 3 reserved, bits 4-5 = scale
    /// (0..=3)`, but zwave-js and the reference stack just place the
    /// 2-bit scale value in the byte (caller gives 0..=3); more exotic
    /// scales use the higher-version Get V11 variant we don't implement
    /// yet.
    pub fn encode_get_specific(sensor_type: u8, scale: u8) -> Vec<u8> {
        vec![
            CC_SENSOR_MULTILEVEL,
            CMD_GET,
            sensor_type,
            (scale & 0x03) << 3,
        ]
    }

    pub fn parse_report(payload: &[u8]) -> Option<SensorReading> {
        if payload.len() < 4 {
            return None;
        }
        if payload[0] != CC_SENSOR_MULTILEVEL || payload[1] != CMD_REPORT {
            return None;
        }
        let sensor_type = payload[2];
        let (ev, _consumed) = parse_encoded_value(&payload[3..])?;
        Some(SensorReading {
            sensor_type,
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
    fn encode_get_v1_no_params() {
        assert_eq!(SensorMultilevelCc::encode_get(), vec![0x31, 0x04]);
    }

    #[test]
    fn encode_get_specific_places_scale_in_bit_3_4() {
        // scale=1 → 0b0000_1000 = 0x08
        assert_eq!(
            SensorMultilevelCc::encode_get_specific(SENSOR_TYPE_TEMPERATURE, 1),
            vec![0x31, 0x04, 0x01, 0x08]
        );
    }

    #[test]
    fn parse_report_temperature_celsius() {
        // sensor_type=0x01 (temp), level=0x22 (prec=1, scale=0 °C, size=2),
        // value=0x00D7 = 215 → 21.5
        let r = SensorMultilevelCc::parse_report(&[0x31, 0x05, 0x01, 0x22, 0x00, 0xD7])
            .expect("report");
        assert_eq!(r.sensor_type, SENSOR_TYPE_TEMPERATURE);
        assert!((r.value - 21.5).abs() < 1e-9);
        assert_eq!(r.scale, 0); // °C
    }

    #[test]
    fn parse_report_humidity_one_byte() {
        // sensor_type=0x05, level=0x01 (prec=0, scale=0 %, size=1), value=0x3C = 60
        let r = SensorMultilevelCc::parse_report(&[0x31, 0x05, 0x05, 0x01, 0x3C])
            .expect("report");
        assert_eq!(r.sensor_type, SENSOR_TYPE_HUMIDITY);
        assert!((r.value - 60.0).abs() < 1e-9);
    }

    #[test]
    fn parse_report_negative_temp() {
        // -5.0°C: prec=1, scale=0, size=2, raw = -50 = 0xFFCE
        let r = SensorMultilevelCc::parse_report(&[0x31, 0x05, 0x01, 0x22, 0xFF, 0xCE])
            .expect("report");
        assert!((r.value - (-5.0)).abs() < 1e-9);
    }

    #[test]
    fn parse_report_rejects_wrong_cc_or_command() {
        assert!(SensorMultilevelCc::parse_report(&[0x99, 0x05, 0x01, 0x01, 0x00]).is_none());
        assert!(SensorMultilevelCc::parse_report(&[0x31, 0x04, 0x01, 0x01, 0x00]).is_none());
    }

    #[test]
    fn parse_report_rejects_short_payload() {
        assert!(SensorMultilevelCc::parse_report(&[0x31, 0x05, 0x01]).is_none());
    }
}
