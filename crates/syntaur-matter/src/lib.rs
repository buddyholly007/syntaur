//! Pure-Rust Matter commissioner owned by Syntaur. Phase 1 of Path C —
//! see [[projects/path_c_plan]] in the vault for the multi-session
//! roadmap.
//!
//! What this crate provides today (Phase 1):
//! - **Fabric generation**: wraps `rs_matter::commissioner::FabricCredentials::new`
//!   to stand up a fresh Matter fabric (CA keypair, IPK, RCAC).
//! - **Encrypted persistence**: writes each fabric to a per-fabric
//!   XChaCha20-Poly1305 blob at `~/.syntaur/matter_fabrics/<label>.enc`
//!   using a 32-byte key derived from the existing
//!   `~/.syntaur/master.key` file. Atomic writes via tmp+rename.
//! - **NOC signing interface** for Phase 3's commissioner state machine
//!   (wraps `NocGenerator::from_root_ca` and
//!   `generate_device_credentials_with_node_id`).
//!
//! What's NOT here yet (explicitly):
//! - The commissioner state machine (Phase 3)
//! - BLE central transport (Phase 4)
//! - QR / manual-code parser (Phase 2)
//! - Gateway integration (Phase 6)

pub mod commission;
pub mod error;
pub mod fabric;
pub mod pairing;
pub mod persist;
pub mod sign;
pub mod tlv_build;

pub use commission::{CommissionExchange, CommissionedDevice, Commissioner, RegulatoryConfig, WifiCredentials};
pub use error::MatterFabricError;
pub use fabric::{FabricHandle, FabricSummary};
pub use pairing::{parse_manual_code, parse_qr, CommissioningFlow, PairingPayload};
pub use persist::{default_dir, list_fabrics, load_fabric, save_fabric};
pub use sign::sign_device_noc;
