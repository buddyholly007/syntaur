//! `syntaur-zwave` — pure-Rust Z-Wave Serial API client.
//!
//! Smart Home and Network module (Track D) depends on this crate for all
//! Z-Wave device control. No sidecars: the abandoned `rzw` crate is
//! being rebuilt here against the published Z-Wave Serial API spec, same
//! wire protocol the (TypeScript) `zwave-js` project speaks.
//!
//! ## Layering
//!
//! ```text
//!  ┌─────────────────────────────────────────────────────────────┐
//!  │  application          (smart_home/drivers/zwave.rs, CCs)    │
//!  ├─────────────────────────────────────────────────────────────┤
//!  │  controller           (inclusion / exclusion / routing)     │
//!  ├─────────────────────────────────────────────────────────────┤
//!  │  frame                (Serial API Data Frame codec)         │
//!  ├─────────────────────────────────────────────────────────────┤
//!  │  serial (this crate's  (SOF / ACK / NAK / CAN, retransmit,  │
//!  │          current scope)  1.5 s ACK timeout, checksum)       │
//!  └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! Week 1 ships the link layer in `serial` + `frame` — enough to send
//! and receive framed bytes to a real Z-Wave controller stick.
//! Subsequent weeks add controller init, command classes, and S2
//! security per the plan calendar.

pub mod command_classes;
pub mod controller;
pub mod frame;
pub mod serial;

pub use command_classes::{
    DoorLockCc, MeterCc, NotificationCc, SensorMultilevelCc, SwitchBinaryCc,
    SwitchMultilevelCc, ThermostatModeCc, ThermostatSetpointCc,
};
pub use controller::{
    ApiCapabilities, Controller, ControllerCapabilities, ControllerError, HomeId, InitData,
    NodeInfo, NodeInfoCache, SendDataReport, VersionInfo,
};
pub use frame::{Frame, FrameKind, FrameParseError};
pub use serial::{LinkError, LinkLayer};
