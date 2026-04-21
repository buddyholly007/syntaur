//! Pure-Rust Matter client using upstream `rs-matter` primitives.
//!
//! ## Status: SKELETON — not wired into Tool routing yet.
//!
//! This module exercises upstream `rs-matter` (current main, ~55% of
//! Controller surface merged in last 60 days — see SECURITY/architecture
//! readout in vault). It exists to:
//!
//! 1. Prove the rs-matter crate compiles into syntaur-gateway with our
//!    feature flags (`os`, `rustcrypto`, `log`) — no surprise feature
//!    gates around dependencies, no transitive cargo-audit blowups.
//! 2. Expose a typed Rust API matching what `tools/matter.rs` does today
//!    via the python-matter-server WebSocket, so the eventual cutover is
//!    a backend-selector flip rather than a rewrite.
//! 3. Document explicitly what upstream still lacks for our use case so
//!    Sean can decide which gaps to contribute upstream vs. fill locally.
//!
//! ## What we use today (production)
//!
//! `tools/matter.rs` calls `python-matter-server` via WebSocket:
//!   - `get_nodes` → list paired devices + cached attributes
//!   - `device_command` → invoke OnOff/LevelControl cluster commands
//!
//! That's the *entire* surface. We do NOT commission devices through
//! Syntaur (the bridge or a phone app does that out of band), and we do
//! NOT subscribe (we read attributes from the bridge's cache).
//!
//! ## What rs-matter has merged on `main` (HEAD 993a0763, 2026-04-20)
//!
//! - PASE Initiator (#388)              — passcode → ephemeral session
//! - CASE Initiator (#410)              — NOC → operational session
//! - IM Client read/write/invoke (#391) — single-shot cluster ops
//! - mDNS commissionable query (#380)   — find devices in pairing mode
//! - PAA trust store (#389)             — verify device attestation chain
//! - NOC generation (#394)              — sign operational certs
//! - Commissioning NVS persist (#405)   — fabric survives restart
//!
//! ## What we'd still need for our use case
//!
//! A. **Operational mDNS query** (`_matter._tcp` for paired devices).
//!    Tracked upstream in #370. Without this we can't discover where on
//!    the LAN a paired device lives — we'd have to either hardcode
//!    addresses, parse python-matter-server's cache, or implement the
//!    `_matter._tcp` browse path ourselves (3-5 days of work, builds
//!    on the existing `_matterc._udp` querier in #380).
//!
//! B. **Fabric-credential import from python-matter-server**. Sean's 31
//!    paired devices live on a fabric whose root cert + admin NOC + key
//!    + IPK are stored in `~/.python-matter-server/<hash>.json`. We can
//!    either:
//!      i.  Parse that file and import into rs-matter's `FabricMgr`.
//!      ii. Add rs-matter as a SECOND admin on the same fabric (Matter
//!          spec supports multi-admin per fabric — needs a one-time
//!          `add_noc` from the existing admin).
//!    Option (i) is faster but couples us to python-matter-server's
//!    on-disk format. Option (ii) is cleaner long-term.
//!
//! C. **Subscriptions** (gap #2 in the readout). We don't need them for
//!    the current Tool surface (read-on-demand is fine), but ANY future
//!    "react to device state change" feature requires this to land
//!    upstream first.
//!
//! ## Plan
//!
//! Stage 1 (this skeleton): dep compiles, types instantiate, `cargo
//! build` is clean. No real Matter operations yet.
//!
//! Stage 2 (next session, needs Sean): Phase B fabric extraction; wire
//! CASE init + IM Client read/invoke against one device. CLI binary so
//! Sean can smoke-test against a single bulb without disturbing the
//! bridge.
//!
//! Stage 3 (gated on upstream): operational mDNS, subscriptions. Either
//! land upstream PRs or build a Syntaur-local implementation.

#![allow(dead_code)] // Skeleton — fields exist for the API shape, not used yet.

use std::sync::Arc;

/// Backend selector for the Matter Tool. The default is `Bridge`
/// (python-matter-server WebSocket); set `SYNTAUR_MATTER_BACKEND=direct`
/// to route through this skeleton (will return clean "not implemented"
/// errors until Stage 2 lands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatterBackend {
    /// `tools/matter.rs` — production today, requires SSH-tunneled WS to
    /// python-matter-server on HAOS.
    Bridge,
    /// `tools/matter_direct.rs` — pure-Rust via upstream rs-matter.
    /// Skeleton only as of HEAD.
    Direct,
}

impl MatterBackend {
    pub fn from_env() -> Self {
        match std::env::var("SYNTAUR_MATTER_BACKEND").as_deref() {
            Ok("direct") => Self::Direct,
            _ => Self::Bridge,
        }
    }
}

/// Errors the direct backend can return — distinguishable from bridge
/// errors so the caller can fall back.
#[derive(Debug, thiserror::Error)]
pub enum DirectError {
    #[error("operational mDNS query not yet implemented (rs-matter #370 open). Cannot resolve node {node_id} address.")]
    OperationalMdnsMissing { node_id: u64 },

    #[error("fabric credentials not loaded — set SYNTAUR_MATTER_FABRIC_FILE or import from python-matter-server")]
    FabricNotLoaded,

    #[error("subscriptions not supported on rs-matter IM Client (gap #2 in readout)")]
    SubscriptionsUnsupported,

    #[error("CASE session establish failed for node {node_id}: {reason}")]
    CaseFailed { node_id: u64, reason: String },

    #[error("rs-matter internal: {0}")]
    Matter(String),
}

/// Skeleton client. Real implementation in Stage 2.
pub struct MatterDirectClient {
    /// Path to fabric-credential JSON (extracted from python-matter-server
    /// or generated via NOC generator for a fresh fabric).
    fabric_file: Option<std::path::PathBuf>,
    /// Cache of last-known node IP addresses, keyed by node_id. Populated
    /// either by parsing python-matter-server's runtime cache or by an
    /// operational mDNS query once that lands.
    address_cache: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, std::net::SocketAddr>>>,
}

impl MatterDirectClient {
    pub fn new() -> Self {
        Self {
            fabric_file: std::env::var("SYNTAUR_MATTER_FABRIC_FILE").ok().map(Into::into),
            address_cache: Arc::new(tokio::sync::RwLock::new(Default::default())),
        }
    }

    /// List paired nodes. Stage 1: returns FabricNotLoaded.
    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>, DirectError> {
        if self.fabric_file.is_none() {
            return Err(DirectError::FabricNotLoaded);
        }
        // Stage 2: enumerate fabric.devices, per-device do operational
        // mDNS lookup (when #370 lands) or address-cache hit, then CASE
        // session resume + IM read of cluster 0x0028 (Basic Information)
        // for vendor/product/etc. + cluster 6/0 OnOff state + cluster
        // 8/0 LevelControl state.
        Err(DirectError::OperationalMdnsMissing { node_id: 0 })
    }

    /// Set OnOff cluster (cluster 6) attribute or invoke command. Stage
    /// 1: returns OperationalMdnsMissing.
    pub async fn set_on_off(&self, node_id: u64, on: bool) -> Result<(), DirectError> {
        let _ = on;
        // Stage 2: address_cache.get(&node_id) or mDNS lookup; CASE
        // session establish; IM Client invoke cluster 6 command 0/1
        // (Off/On).
        Err(DirectError::OperationalMdnsMissing { node_id })
    }

    /// Set LevelControl cluster (cluster 8) attribute. Stage 1: returns
    /// OperationalMdnsMissing.
    pub async fn set_level(&self, node_id: u64, level: u8) -> Result<(), DirectError> {
        let _ = level;
        // Stage 2: same as set_on_off but cluster 8 command 0
        // (MoveToLevel) with level + transition_time.
        Err(DirectError::OperationalMdnsMissing { node_id })
    }

    /// Read OnOff state. Stage 1: returns OperationalMdnsMissing.
    pub async fn read_on_off(&self, node_id: u64) -> Result<bool, DirectError> {
        // Stage 2: CASE + IM read cluster 6 attribute 0.
        Err(DirectError::OperationalMdnsMissing { node_id })
    }
}

impl Default for MatterDirectClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a paired node, returned by list_nodes().
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeSummary {
    pub node_id: u64,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub label: Option<String>,
    pub on_off: Option<bool>,
    pub level: Option<u8>,
    pub address: Option<String>,
}

/// Compile-time smoke: instantiate a few rs-matter types we'll need in
/// Stage 2. If this builds, the dep wiring works. If it doesn't, the
/// rev pin needs bumping or feature flags adjusting.
#[cfg(test)]
mod compile_smoke {
    #[test]
    fn rs_matter_types_resolve() {
        // We don't instantiate (most rs-matter types need a full Matter
        // setup), but we DO want to verify the public paths exist at
        // the pinned rev. If any of these `use` statements fail at
        // compile time, that's the signal to bump or pivot.
        #[allow(unused_imports)]
        use rs_matter::error::Error as MatterError;
        #[allow(unused_imports)]
        use rs_matter::dm::clusters::on_off as _;
        #[allow(unused_imports)]
        use rs_matter::dm::clusters::level_control as _;
    }
}
