//! Path C Phase 4 — BLE central transport for Matter commissioning.
//!
//! Status: **scan + filter shipped; BTP session layer WIP**. The scan
//! pathway ([`scan_for_discriminator`]) already compiles + runs on a
//! Linux host with BlueZ and returns matching advertisements. The
//! full BTP framing over GATT C1/C2 characteristics is the remaining
//! ~500 LoC — see `btp.rs` skeleton + the inline TODOs.
//!
//! When Phase 4 finishes, this crate's `BleCommissionExchange` will
//! implement the `syntaur_matter::CommissionExchange` trait, letting
//! the state machine in [`syntaur_matter::commission`] drive a
//! factory-fresh device end-to-end over BLE.

pub mod btp;
pub mod scan;

pub use scan::{scan_for_discriminator, CommissionableDevice, MATTER_SERVICE_UUID};
