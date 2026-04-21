//! Per-protocol driver registry. Each submodule owns one
//! protocol's device lifecycle + state cache + event feed into the
//! module-wide broadcast channel.
//!
//! Stubs-only at Track A week 1 — real drivers land per the plan
//! calendar: wifi_lan (week 2), matter (weeks 3–4), zigbee (weeks 5–6),
//! ble (week 7), mqtt (week 8), camera (week 9), zwave (week 13),
//! cloud adapters (week 11).

pub mod ble;
pub mod camera;
pub mod cloud;
pub mod matter;
pub mod mqtt;
pub mod wifi_lan;
pub mod zigbee;
pub mod zwave;
