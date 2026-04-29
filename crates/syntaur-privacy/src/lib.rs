//! Syntaur smart-device privacy plumbing.
//!
//! Two tiers, per `projects/syntaur_privacy_mode`:
//!
//! - **Privacy** (this crate, fully built): DNS sinkhole + per-device policy +
//!   monitoring. Runs on the same host as the main Syntaur app. No special
//!   hardware required.
//! - **Lockdown** (stub only — see `policy::Mode::Lockdown`): full L3 gateway
//!   with nft / DHCP / IPv6 policy. Gated on dedicated-hardware deploy
//!   (Sean's plan: N100 mini-PC post-HA-phaseout). Code lives in
//!   a future `gateway` module that this crate intentionally does not
//!   ship today, so the UI cannot truthfully claim Lockdown is available.

pub mod dns_sinkhole;
pub mod error;
pub mod monitor;
pub mod policy;
pub mod registry;

pub use error::{Error, Result};
pub use policy::{DevicePolicy, Mode};
pub use registry::{CloudDomainRegistry, VendorEntry};
