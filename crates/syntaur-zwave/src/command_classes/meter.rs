//! COMMAND_CLASS_METER (0x32) — V1/V2 subset.
//!
//! Electric / gas / water / heating / cooling meter readings.
//!
//! ```text
//!   Get V1   = [0x32, 0x01]
//!   Report V1 = [0x32, 0x02, meter_type_byte, level_byte, value_bytes...]
//!   Report V2+= [0x32, 0x02, meter_type_byte, level_byte, value_bytes...,
//!                 delta_time_hi, delta_time_lo, prev_value_bytes...]
//! ```
//!
//! `meter_type_byte`:
//! ```text
//!   bit 7     scale bit 2 (V3+) — we ignore; set to 0
//!   bits 5-6  rate type: 0 = unspecified, 1 = import, 2 = export
//!   bits 0-4  meter type: 1 = electric, 2 = gas, 3 = water, 4 = heating, 5 = cooling
//! ```
//!
//! `level_byte` is shared with SensorMultilevel (precision / scale / size).

use super::{parse_encoded_value, CC_METER};

pub const CMD_GET: u8 = 0x01;
pub const CMD_REPORT: u8 = 0x02;

pub const METER_TYPE_ELECTRIC: u8 = 1;
pub const METER_TYPE_GAS: u8 = 2;
pub const METER_TYPE_WATER: u8 = 3;
pub const METER_TYPE_HEATING: u8 = 4;
pub const METER_TYPE_COOLING: u8 = 5;

pub const RATE_TYPE_UNSPECIFIED: u8 = 0;
pub const RATE_TYPE_IMPORT: u8 = 1;
pub const RATE_TYPE_EXPORT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterReading {
    pub meter_type: u8,
    pub rate_type: u8,
    pub value: f64,
    pub scale: u8,
    pub precision: u8,
    /// V2+ delta_time in seconds since the previous report. None on V1.
    pub delta_time_sec: Option<u16>,
    /// V2+ previous value. None on V1 reports.
    pub previous_value: Option<f64>,
}

pub struct MeterCc;

impl MeterCc {
    pub fn encode_get() -> Vec<u8> {
        vec![CC_METER, CMD_GET]
    }

    pub fn parse_report(payload: &[u8]) -> Option<MeterReading> {
        if payload.len() < 4 {
            return None;
        }
        if payload[0] != CC_METER || payload[1] != CMD_REPORT {
            return None;
        }
        let type_byte = payload[2];
        let meter_type = type_byte & 0x1F;
        let rate_type = (type_byte >> 5) & 0x03;

        let (ev, consumed) = parse_encoded_value(&payload[3..])?;
        let mut cursor = 3 + consumed;

        let (delta_time_sec, previous_value) = if payload.len() >= cursor + 2 {
            let dt = u16::from_be_bytes([payload[cursor], payload[cursor + 1]]);
            cursor += 2;
            // Previous value uses the SAME size as current. parse_encoded_value
            // expects its own level byte — V2+ layout re-uses the outer
            // level byte by convention (spec §4.63.5), so synthesize it
            // and feed only the raw bytes.
            let prev = if payload.len() >= cursor + ev.size as usize {
                let raw = &payload[cursor..cursor + ev.size as usize];
                let signed: i64 = match ev.size {
                    1 => raw[0] as i8 as i64,
                    2 => i16::from_be_bytes([raw[0], raw[1]]) as i64,
                    4 => i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64,
                    _ => return None,
                };
                Some(signed as f64 / 10f64.powi(ev.precision as i32))
            } else {
                None
            };
            (Some(dt), prev)
        } else {
            (None, None)
        };

        Some(MeterReading {
            meter_type,
            rate_type,
            value: ev.value,
            scale: ev.scale,
            precision: ev.precision,
            delta_time_sec,
            previous_value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_get_v1() {
        assert_eq!(MeterCc::encode_get(), vec![0x32, 0x01]);
    }

    #[test]
    fn parse_report_v1_electric_kwh() {
        // type_byte = 0b0010_0001 = 0x21 — rate_type=1 (import), meter_type=1 (electric)
        // level = 0x22 — precision=1, scale=0 (kWh), size=2
        // raw = 0x0CE4 = 3300 → value = 330.0
        let r = MeterCc::parse_report(&[0x32, 0x02, 0x21, 0x22, 0x0C, 0xE4]).expect("report");
        assert_eq!(r.meter_type, METER_TYPE_ELECTRIC);
        assert_eq!(r.rate_type, RATE_TYPE_IMPORT);
        assert!((r.value - 330.0).abs() < 1e-6);
        assert_eq!(r.scale, 0);
        assert!(r.delta_time_sec.is_none());
        assert!(r.previous_value.is_none());
    }

    #[test]
    fn parse_report_v2_with_delta_and_previous() {
        // Same base reading + 60-second delta + previous value 325.0 (0x0CB2 = 3250).
        let r = MeterCc::parse_report(&[
            0x32, 0x02, 0x21, 0x22, 0x0C, 0xE4, 0x00, 0x3C, 0x0C, 0xB2,
        ])
        .expect("report");
        assert_eq!(r.delta_time_sec, Some(60));
        assert!((r.previous_value.unwrap() - 325.0).abs() < 1e-6);
    }

    #[test]
    fn parse_report_water_meter() {
        // type=0x03 (water), rate=0 → 0x03. level=0x23 prec=1 scale=0 size=3?
        // size=3 is invalid per spec. Use size=4 (0x24) — 4-byte value = 12345678 → 1234567.8
        let r = MeterCc::parse_report(&[
            0x32, 0x02, 0x03, 0x24, 0x00, 0xBC, 0x61, 0x4E,
        ])
        .expect("report");
        assert_eq!(r.meter_type, METER_TYPE_WATER);
        assert!((r.value - 1234567.8).abs() < 1e-3);
    }

    #[test]
    fn parse_report_rejects_wrong_cc() {
        assert!(MeterCc::parse_report(&[0x99, 0x02, 0x01, 0x01, 0x00]).is_none());
    }

    #[test]
    fn parse_report_rejects_short_payload() {
        assert!(MeterCc::parse_report(&[0x32, 0x02, 0x01]).is_none());
    }
}
