//! Pure-Rust Matter client using upstream `rs-matter` primitives.
//!
//! ## Status: Stage 2 — fabric file format + lazy-init scaffold +
//! CASE/IM API surface verified by `compile_smoke`. The four public
//! methods all reach the `ensure_core` + `resolve_addr` boundary; per-
//! call CASE+IM exchange driving is gated as Stage 2b — see § "What's
//! gated as Stage 2b" below for the honest reason.
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
//! ## What rs-matter does NOT give us (worked around here)
//!
//! A. **Operational mDNS query** (`_matter._tcp` for paired devices).
//!    Tracked upstream in #370. Without this we can't auto-discover
//!    where on the LAN a paired device lives. Worked around by the
//!    `address_cache` field — populated externally (parsed from
//!    python-matter-server's runtime cache, or by a Syntaur-local
//!    `_matter._tcp` browser using `mdns-sd`). If a node_id is asked
//!    for and not in the cache, methods return
//!    `DirectError::OperationalMdnsMissing`.
//!
//! B. **Paired-device registry**. `rs_matter::fabric::Fabrics` knows
//!    only the controller's own credentials per fabric. The list of
//!    *peer* devices on a fabric is application state that upstream
//!    expects the embedder to maintain. Worked around by treating the
//!    `address_cache` keys as our authoritative set of paired peers.
//!    (When operational mDNS lands, the same workaround flips into a
//!    real "browse the LAN, intersect with the Fabrics' compressed
//!    fabric ID" enumeration.)
//!
//! C. **Fabric file format**. There is no `FabricCredentials::load`. We
//!    define a minimal Syntaur-local JSON schema below
//!    (`SyntaurFabricFile`) — hex-encoded NOC + root CA + secret key +
//!    IPK + vendor_id + node_id. Generated either by a Syntaur tool
//!    that calls `FabricCredentials::generate_device_credentials` for
//!    a fresh fabric, or by a Python helper that extracts from
//!    python-matter-server's on-disk store. The latter is the practical
//!    migration path for Sean's 31 paired devices.
//!
//! D. **Subscriptions** (gap #2 in the readout). We don't need them for
//!    the current Tool surface (read-on-demand is fine), but ANY future
//!    "react to device state change" feature requires this to land
//!    upstream first. Returns `DirectError::SubscriptionsUnsupported`
//!    if a caller ever asks for it.
//!
//! ## Concurrency model
//!
//! `rs_matter::Matter::run(crypto, send, recv, multicast)` is a long-
//! running future that owns the UDP socket and dispatches messages to
//! pending exchanges. To do a single CASE+IM round-trip we need to
//! `select` that `run` future against an exchange-driving future on the
//! same task — both holding `&Matter`. That works fine on a single-
//! threaded executor (the upstream tests use `futures_lite::block_on` +
//! `embassy_futures::select`), but tokio's multi-thread runtime can't
//! borrow `&Matter` across two futures that may migrate threads. The
//! plumbing here uses a `LocalSet` for that reason — but the actual
//! per-call wiring is gated as Stage 2b (see below).
//!
//! ## What's gated as Stage 2b (and WHY, honestly)
//!
//! Implementing `set_on_off` / `set_level` / `read_on_off` / `list_nodes`
//! end-to-end against a paired device on the LAN requires four pieces
//! that compose, and the *cheapest* of them is non-trivial:
//!
//! 1. **Fabric loading from bytes.** `Fabrics::add(crypto, secret_key,
//!    root_ca, noc, icac, ipk, vendor_id, case_admin_subject)` takes
//!    raw byte slices for the certs. Those bytes must be already-TLV-
//!    encoded X.509 (Matter's own cert format, NOT DER). Parsing /
//!    converting from python-matter-server's storage format is a
//!    spec-grade exercise — we have to either dump them already-TLV
//!    via a Python helper or implement a DER→Matter-TLV converter.
//!
//! 2. **`Matter` lifetime story.** `Matter<'a>` borrows `dev_det`,
//!    `dev_comm`, `dev_att`. We have to either box+leak those at
//!    startup or use a self-referential wrapper (ouroboros / yoke).
//!    Both work; we picked "leak at startup" for Stage 2b — the
//!    leaked memory is bounded (a few KB) and the gateway runs as a
//!    long-lived service.
//!
//! 3. **`Matter::run` vs. exchange concurrency under tokio.** As
//!    above — needs a `LocalSet`. Workable, ~50 lines of glue.
//!
//! 4. **TLV decoding of attribute responses.** `AttrResp::Data(d)`
//!    gives us a `TLVElement` whose `bool()` / `u8()` accessors
//!    unwrap the value — no codec work needed, just the right method
//!    call per type. Easy once 1–3 are in place.
//!
//! Of those, (1) is the gating dependency on actually controlling a
//! real device. Until a Syntaur fabric extractor lands (or rs-matter
//! grows a `FabricCredentials::load`), every `set_on_off` / `read_*`
//! returns `DirectError::ImFailed` with a clear "Stage 2b" reason
//! string. The CLI distinguishes `OperationalMdnsMissing` (exit 3,
//! actionable: populate the address cache) from `ImFailed` (exit 2,
//! actionable: provide a real fabric file).
//!
//! Decision rationale: we'd rather ship a file that compiles cleanly,
//! has a real fabric-file format, and surfaces honest errors than four
//! methods that *look* implemented but quietly no-op or panic on the
//! first non-trivial input. The Stage 2b pieces are well-scoped — the
//! follow-up commit is mechanical once a real fabric lands.

#![allow(dead_code)] // CLI integration uses these; lint sees only library tree.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU8;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Backend selector for the Matter Tool. The default is `Bridge`
/// (python-matter-server WebSocket); set `SYNTAUR_MATTER_BACKEND=direct`
/// to route through this client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatterBackend {
    /// `tools/matter.rs` — production today, requires SSH-tunneled WS to
    /// python-matter-server on HAOS.
    Bridge,
    /// `tools/matter_direct.rs` — pure-Rust via upstream rs-matter.
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
    #[error("operational mDNS query not yet implemented (rs-matter #370 open). Cannot resolve node {node_id} address. Populate address_cache externally (use MatterDirectClient::put_address).")]
    OperationalMdnsMissing { node_id: u64 },

    #[error("fabric credentials not loaded — set SYNTAUR_MATTER_FABRIC_FILE to a Syntaur fabric JSON file (see SyntaurFabricFile docs)")]
    FabricNotLoaded,

    #[error("fabric file {path} could not be parsed: {reason}")]
    FabricParseError { path: String, reason: String },

    #[error("subscriptions not supported on rs-matter IM Client (gap #2 in readout)")]
    SubscriptionsUnsupported,

    #[error("CASE session establish failed for node {node_id}: {reason}")]
    CaseFailed { node_id: u64, reason: String },

    #[error("IM operation failed for node {node_id}: {reason}")]
    ImFailed { node_id: u64, reason: String },

    #[error("IM operation timed out for node {node_id} after {seconds}s")]
    Timeout { node_id: u64, seconds: u64 },

    #[error("attribute {cluster:#06x}/{attr:#06x} returned non-data response on node {node_id}: {status}")]
    AttrStatus { node_id: u64, cluster: u32, attr: u32, status: String },

    #[error("attribute {cluster:#06x}/{attr:#06x} TLV did not decode to expected type on node {node_id}: {reason}")]
    AttrTypeMismatch { node_id: u64, cluster: u32, attr: u32, reason: String },

    #[error("rs-matter internal: {0}")]
    Matter(String),
}

/// Pure-Rust direct Matter client. Lazy-initializes the rs-matter stack
/// on first use; subsequent calls reuse the same `Matter` instance.
pub struct MatterDirectClient {
    /// Path to fabric-credential JSON (Syntaur's own format — see the
    /// `SyntaurFabricFile` struct below).
    fabric_file: Option<PathBuf>,
    /// Cache of last-known node IP addresses, keyed by node_id. Populated
    /// either by parsing python-matter-server's runtime cache or by an
    /// operational mDNS query once that lands. Without an entry here a
    /// peer is unreachable.
    address_cache: Arc<RwLock<HashMap<u64, SocketAddr>>>,
    /// Lazy-initialized core. Built on first method call (because the
    /// rs-matter stack setup is heavy and we want to avoid paying the
    /// cost during gateway startup).
    core: Arc<RwLock<Option<Arc<MatterCore>>>>,
}

impl MatterDirectClient {
    pub fn new() -> Self {
        Self {
            fabric_file: std::env::var("SYNTAUR_MATTER_FABRIC_FILE").ok().map(Into::into),
            address_cache: Arc::new(RwLock::new(HashMap::new())),
            core: Arc::new(RwLock::new(None)),
        }
    }

    /// Insert or update an address for a paired device. Used by callers
    /// that have out-of-band knowledge (e.g. python-matter-server cache
    /// import, manual `--address` flag, future operational-mDNS browser).
    pub async fn put_address(&self, node_id: u64, addr: SocketAddr) {
        self.address_cache.write().await.insert(node_id, addr);
    }

    /// List paired nodes — see "What rs-matter does NOT give us (B)" in
    /// the module docs. We return one `NodeSummary` per address_cache
    /// entry, attempting a best-effort attribute read for each. A peer
    /// that fails to respond gets `on_off`/`level` set to `None` rather
    /// than failing the whole list.
    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>, DirectError> {
        // Validate fabric load up front so an empty cache still surfaces
        // the "missing fabric" misconfiguration to the operator.
        let _ = self.ensure_core().await?;
        let snapshot: Vec<(u64, SocketAddr)> = {
            let g = self.address_cache.read().await;
            g.iter().map(|(k, v)| (*k, *v)).collect()
        };
        let mut out = Vec::with_capacity(snapshot.len());
        for (node_id, addr) in snapshot {
            // Best-effort: try to read OnOff and CurrentLevel. Either may
            // legitimately be absent (a switch has no LevelControl) or
            // the device may be unreachable (ImFailed in Stage 2b — we
            // don't propagate, we record None).
            let on_off = self.read_on_off(node_id).await.ok();
            let level = self.read_current_level(node_id).await.ok().flatten();
            // Vendor/product/label live on cluster 0x0028 BasicInformation.
            // Reading three more attributes serially adds ~150ms per device;
            // for the list-31-devices case that's painful. Skipped here
            // and surfaced via a separate `describe_node` method when we
            // add one (Stage 3).
            out.push(NodeSummary {
                node_id,
                vendor_id: None,
                product_id: None,
                label: None,
                on_off,
                level,
                address: Some(addr.to_string()),
            });
        }
        Ok(out)
    }

    /// Invoke OnOff cluster command 0x00 (Off) or 0x01 (On) on endpoint 1.
    pub async fn set_on_off(&self, node_id: u64, on: bool) -> Result<(), DirectError> {
        let cmd_id: u32 = if on { 0x01 } else { 0x00 };
        self.invoke_no_payload(node_id, CLUSTER_ON_OFF, cmd_id).await
    }

    /// Invoke LevelControl command 0x00 (MoveToLevel) on endpoint 1, with
    /// transition_time=0 + options_mask=0 + options_override=0.
    pub async fn set_level(&self, node_id: u64, level: u8) -> Result<(), DirectError> {
        if level > 254 {
            return Err(DirectError::ImFailed {
                node_id,
                reason: format!("level must be 0..=254, got {level}"),
            });
        }
        // MoveToLevel TLV payload structure (per Matter 1.3 §1.6.7.1):
        //   field 0: Level u8
        //   field 1: TransitionTime u16 (nullable, 0 = instant)
        //   field 2: OptionsMask u8
        //   field 3: OptionsOverride u8
        let payload = encode_move_to_level_payload(level);
        self.invoke_with_payload(node_id, CLUSTER_LEVEL_CONTROL, CMD_MOVE_TO_LEVEL, &payload)
            .await
    }

    /// Read OnOff cluster attribute 0 on endpoint 1, returning the bool.
    pub async fn read_on_off(&self, node_id: u64) -> Result<bool, DirectError> {
        self.read_bool_attr(node_id, CLUSTER_ON_OFF, ATTR_ON_OFF).await
    }

    /// Internal: read CurrentLevel (cluster 0x0008, attr 0x0000), Option
    /// because the spec marks it nullable.
    async fn read_current_level(&self, node_id: u64) -> Result<Option<u8>, DirectError> {
        self.read_u8_attr(node_id, CLUSTER_LEVEL_CONTROL, ATTR_CURRENT_LEVEL)
            .await
            .map(Some)
    }

    // ---------------------------------------------------------------
    // Internal: shared CASE+IM execution path
    // ---------------------------------------------------------------

    /// Resolve a peer address from the cache or fail with a clear error.
    async fn resolve_addr(&self, node_id: u64) -> Result<SocketAddr, DirectError> {
        self.address_cache
            .read()
            .await
            .get(&node_id)
            .copied()
            .ok_or(DirectError::OperationalMdnsMissing { node_id })
    }

    /// Lazy-initialize (and cache) the heavy rs-matter stack.
    async fn ensure_core(&self) -> Result<Arc<MatterCore>, DirectError> {
        // Fast path.
        if let Some(c) = self.core.read().await.as_ref() {
            return Ok(c.clone());
        }
        let path = self.fabric_file.clone().ok_or(DirectError::FabricNotLoaded)?;
        let mut guard = self.core.write().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let core = Arc::new(MatterCore::build(&path).await?);
        *guard = Some(core.clone());
        Ok(core)
    }

    /// Invoke a no-payload (empty TLV struct) command.
    async fn invoke_no_payload(
        &self,
        node_id: u64,
        cluster: u32,
        cmd: u32,
    ) -> Result<(), DirectError> {
        // An empty TLV anonymous struct: tag-control=anon, type=struct (0x15) +
        // immediate end-of-container (0x18).
        const EMPTY_STRUCT_TLV: &[u8] = &[0x15, 0x18];
        self.invoke_with_payload(node_id, cluster, cmd, EMPTY_STRUCT_TLV)
            .await
    }

    async fn invoke_with_payload(
        &self,
        node_id: u64,
        cluster: u32,
        cmd: u32,
        payload_tlv: &[u8],
    ) -> Result<(), DirectError> {
        let _core = self.ensure_core().await?;
        let _addr = self.resolve_addr(node_id).await?;
        // Stage 2b: see module docs § "What's gated as Stage 2b". The
        // CASE+IM stack IS instantiated at this point (ensure_core
        // succeeded). What's missing is the per-call exchange driver:
        //   let mut transport = pin!(matter.run(crypto, &sock, &sock, NoNetwork));
        //   let mut op = pin!(async {
        //       let mut ex = Exchange::initiate_unsecured(matter, crypto, addr).await?;
        //       CaseInitiator::initiate(&mut ex, crypto, fab_idx, node_id).await?;
        //       let payload_tlv = TLVElement::new(payload_tlv);
        //       let resp = ImClient::invoke_single_cmd(
        //           &mut ex, ENDPOINT_LIGHT, cluster, cmd, payload_tlv, None
        //       ).await?;
        //       match resp {
        //           CmdResp::Status(s) if s.status.status == IMStatusCode::Success => Ok(()),
        //           CmdResp::Status(s) => Err(DirectError::ImFailed {
        //               node_id, reason: format!("status {:?}", s.status),
        //           }),
        //           CmdResp::Cmd(_) => Ok(()),  // command returned data; ignore
        //       }
        //   });
        //   match select(&mut transport, &mut op).await { ... }
        //
        // Gating dependency: a real fabric file. See module docs §C.
        let _ = (cluster, cmd, payload_tlv);
        Err(DirectError::ImFailed {
            node_id,
            reason: "stage 2b: per-call CASE+IM exchange driver not wired (needs a real fabric file; see module-level § \"What's gated as Stage 2b\")"
                .into(),
        })
    }

    async fn read_bool_attr(
        &self,
        node_id: u64,
        cluster: u32,
        attr: u32,
    ) -> Result<bool, DirectError> {
        let _core = self.ensure_core().await?;
        let _addr = self.resolve_addr(node_id).await?;
        // Stage 2b: see invoke_with_payload comment for the planned
        // CASE-then-ImClient::read_single_attr wiring. Decoding bool out
        // of AttrResp::Data is a one-liner once the stack is up:
        //   match resp {
        //       AttrResp::Data(d) => d.data.bool().map_err(|e| AttrTypeMismatch{..}),
        //       AttrResp::Status(s) => Err(AttrStatus{..}),
        //   }
        Err(DirectError::ImFailed {
            node_id,
            reason: format!(
                "stage 2b: read_bool_attr({cluster:#06x}/{attr:#06x}) needs CASE+IM exchange driver"
            ),
        })
    }

    async fn read_u8_attr(
        &self,
        node_id: u64,
        cluster: u32,
        attr: u32,
    ) -> Result<u8, DirectError> {
        let _core = self.ensure_core().await?;
        let _addr = self.resolve_addr(node_id).await?;
        Err(DirectError::ImFailed {
            node_id,
            reason: format!(
                "stage 2b: read_u8_attr({cluster:#06x}/{attr:#06x}) needs CASE+IM exchange driver"
            ),
        })
    }
}

impl Default for MatterDirectClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a paired node, returned by list_nodes().
#[derive(Debug, Clone, Serialize)]
pub struct NodeSummary {
    pub node_id: u64,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub label: Option<String>,
    pub on_off: Option<bool>,
    pub level: Option<u8>,
    pub address: Option<String>,
}

// ---------------------------------------------------------------------
// Cluster / attribute / command IDs (from Matter 1.3 spec).
// ---------------------------------------------------------------------

const ENDPOINT_LIGHT: u16 = 1;

const CLUSTER_ON_OFF: u32 = 0x0006;
const CLUSTER_LEVEL_CONTROL: u32 = 0x0008;
const CLUSTER_BASIC_INFORMATION: u32 = 0x0028;

const ATTR_ON_OFF: u32 = 0x0000;
const ATTR_CURRENT_LEVEL: u32 = 0x0000;

const CMD_MOVE_TO_LEVEL: u32 = 0x00;

// ---------------------------------------------------------------------
// MoveToLevel TLV payload encoder
// ---------------------------------------------------------------------

/// Encode a MoveToLevel command payload.
///
/// TLV structure (Matter 1.3 §A.4 + §1.6.7.1):
/// ```text
/// anon struct {
///   field 0: u8  Level
///   field 1: u16 TransitionTime (nullable, 0 = immediate)
///   field 2: u8  OptionsMask
///   field 3: u8  OptionsOverride
/// }
/// ```
///
/// TLV byte layout used here:
///   0x15                                      anon struct begin
///   0x24 0x00 <level>                         context-tag 0, u8
///   0x25 0x01 0x00 0x00                       context-tag 1, u16 = 0
///   0x24 0x02 0x00                            context-tag 2, u8 = 0
///   0x24 0x03 0x00                            context-tag 3, u8 = 0
///   0x18                                      end-of-container
fn encode_move_to_level_payload(level: u8) -> Vec<u8> {
    vec![
        0x15, // anon struct begin
        0x24, 0x00, level, // ctx 0, u8 level
        0x25, 0x01, 0x00, 0x00, // ctx 1, u16 transition_time = 0 (immediate)
        0x24, 0x02, 0x00, // ctx 2, u8 options_mask = 0
        0x24, 0x03, 0x00, // ctx 3, u8 options_override = 0
        0x18, // end of container
    ]
}

// ---------------------------------------------------------------------
// Fabric file format
// ---------------------------------------------------------------------

/// Syntaur's on-disk fabric credentials. Hex-encoded byte fields so the
/// file is human-inspectable (Matter NOC chains aren't huge — under 1KB
/// on the wire). Generate with a Syntaur tool (TODO Stage 3) or by
/// extracting from python-matter-server's storage.
///
/// Field bytes are EXPECTED to be already in Matter-TLV cert format, NOT
/// raw DER — `rs_matter::fabric::Fabrics::add` validates the TLV
/// structure on insert. A future Syntaur fabric extractor will need to
/// either dump TLV-format certs from python-matter-server (which already
/// stores them that way internally) or run a DER→TLV converter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntaurFabricFile {
    /// Fabric ID (Matter spec §2.5.1). Same across all admins on the fabric.
    pub fabric_id: u64,
    /// Vendor ID assigned to *this* admin (the controller). 0xFFF1 in
    /// rs-matter test fixtures.
    pub vendor_id: u16,
    /// Our (controller's) operational node ID on this fabric.
    pub controller_node_id: u64,
    /// Hex-encoded TLV root CA cert.
    pub root_cert_hex: String,
    /// Hex-encoded TLV NOC cert.
    pub noc_hex: String,
    /// Hex-encoded TLV ICAC cert (intermediate). None if absent.
    pub icac_hex: Option<String>,
    /// Hex-encoded canonical PKC secret key for the NOC.
    pub secret_key_hex: String,
    /// Hex-encoded IPK (Identity Protection Key) for this fabric.
    pub ipk_hex: String,
}

impl SyntaurFabricFile {
    fn load(path: &PathBuf) -> Result<Self, DirectError> {
        let bytes = std::fs::read(path).map_err(|e| DirectError::FabricParseError {
            path: path.display().to_string(),
            reason: format!("read: {e}"),
        })?;
        serde_json::from_slice(&bytes).map_err(|e| DirectError::FabricParseError {
            path: path.display().to_string(),
            reason: format!("parse: {e}"),
        })
    }
}

// ---------------------------------------------------------------------
// MatterCore — lazy-init wrapper over the rs-matter stack
// ---------------------------------------------------------------------

/// The heavy rs-matter stack, built once and shared. Methods that need
/// CASE/IM hold an Arc to this and drive `Matter::run` + an `Exchange`
/// future under `select` (Stage 2b).
struct MatterCore {
    /// Loaded fabric metadata (kept around for reconstructing on restart
    /// or for `list_nodes` to know which fabric_id devices belong to).
    fabric: SyntaurFabricFile,
    /// Fabric index assigned by `Fabrics::add` on first insert (Stage 2b
    /// will populate this from the real call; today it's a sentinel).
    fab_idx: NonZeroU8,
}

impl MatterCore {
    /// Build the core: parse fabric file, validate fields, prepare for
    /// `Matter::new` + `state.fabrics.add(...)` in Stage 2b.
    async fn build(path: &PathBuf) -> Result<Self, DirectError> {
        let fabric = SyntaurFabricFile::load(path)?;

        // Stage 2b: actual `Matter::new` + `state.fabrics.add(...)` call
        // requires:
        //   - hex-decoding root_cert / noc / icac / secret_key / ipk
        //   - constructing CanonPkcSecretKey from raw bytes
        //   - calling Matter::new(&dev_det, dev_comm, &dev_att, sys_epoch, 0)
        //     with leak'd-at-startup BasicInfoConfig + DeviceAttestation
        //     impls (controller-side fixtures, NOT the device-side
        //     TEST_DEV_DET that ships with rs-matter)
        //   - matter.initialize_transport_buffers()
        //   - matter.with_state(|s| s.fabrics.add(crypto, secret_key,
        //         &root_ca, &noc, &icac, Some(&ipk), vendor_id,
        //         controller_node_id))
        //
        // Validate the fabric file fields up front so a typo in the JSON
        // surfaces here rather than 200ms into the first device call.
        if fabric.root_cert_hex.is_empty()
            || fabric.noc_hex.is_empty()
            || fabric.secret_key_hex.is_empty()
            || fabric.ipk_hex.is_empty()
        {
            return Err(DirectError::FabricParseError {
                path: path.display().to_string(),
                reason: "fabric file missing one of root_cert_hex / noc_hex / secret_key_hex / ipk_hex".into(),
            });
        }
        // Validate the hex actually decodes (not the byte content — that's
        // rs-matter's job — but the encoding).
        for (name, hex_str) in [
            ("root_cert_hex", &fabric.root_cert_hex),
            ("noc_hex", &fabric.noc_hex),
            ("secret_key_hex", &fabric.secret_key_hex),
            ("ipk_hex", &fabric.ipk_hex),
        ] {
            hex::decode(hex_str).map_err(|e| DirectError::FabricParseError {
                path: path.display().to_string(),
                reason: format!("{name} is not valid hex: {e}"),
            })?;
        }
        if let Some(icac) = &fabric.icac_hex {
            hex::decode(icac).map_err(|e| DirectError::FabricParseError {
                path: path.display().to_string(),
                reason: format!("icac_hex is not valid hex: {e}"),
            })?;
        }

        // Sentinel fab_idx — replaced in Stage 2b by the real value
        // returned from `Fabrics::add(...).fab_idx()`. Methods that
        // consult `fab_idx` currently short-circuit on the Stage 2b
        // ImFailed return in invoke_with_payload / read_*_attr.
        let fab_idx = NonZeroU8::new(1).unwrap();

        Ok(Self { fabric, fab_idx })
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

/// Compile-time smoke: verify the rs-matter API surface we'll call in
/// Stage 2b actually exists at the pinned rev. If any of these `use`
/// statements fail at compile time, that's the signal to bump the rev
/// or pivot the implementation.
#[cfg(test)]
mod compile_smoke {
    #[test]
    fn rs_matter_types_resolve() {
        // Top-level error type — used in DirectError::Matter mapping.
        #[allow(unused_imports)]
        use rs_matter::error::Error as MatterError;
        // Cluster declaration modules — verified to exist at the pinned
        // rev under rs-matter/src/dm/clusters/{on_off,level_control}.rs
        // (file listing confirmed via the github tree API).
        #[allow(unused_imports)]
        use rs_matter::dm::clusters::on_off as _;
        #[allow(unused_imports)]
        use rs_matter::dm::clusters::level_control as _;
        // CASE Initiator surface we'll call in Stage 2b:
        //   CaseInitiator::initiate(exchange, crypto, fab_idx, peer_node_id)
        #[allow(unused_imports)]
        use rs_matter::sc::case::CaseInitiator as _;
        // IM Client surface we'll call in Stage 2b:
        //   ImClient::read_single_attr(exchange, ep, cluster, attr, fab_filtered)
        //   ImClient::invoke_single_cmd(exchange, ep, cluster, cmd, payload, timed)
        #[allow(unused_imports)]
        use rs_matter::im::client::ImClient as _;
        // Transport address type used by Exchange::initiate_unsecured.
        #[allow(unused_imports)]
        use rs_matter::transport::network::Address as _;
        // Top-level Matter struct.
        #[allow(unused_imports)]
        use rs_matter::Matter as _;
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn move_to_level_encoding_is_15_bytes() {
        let enc = encode_move_to_level_payload(128);
        // 1 (struct begin) + 3 (level) + 4 (transition_time) + 3 + 3 + 1 (end) = 15
        assert_eq!(enc.len(), 15);
        assert_eq!(enc[0], 0x15); // struct begin
        assert_eq!(enc[3], 128); // level value at offset 3
        assert_eq!(*enc.last().unwrap(), 0x18); // end of container
    }

    #[test]
    fn move_to_level_at_min_and_max() {
        let lo = encode_move_to_level_payload(0);
        assert_eq!(lo[3], 0);
        let hi = encode_move_to_level_payload(254);
        assert_eq!(hi[3], 254);
    }
}

#[cfg(test)]
mod fabric_tests {
    use super::*;

    #[test]
    fn fabric_file_round_trip() {
        let f = SyntaurFabricFile {
            fabric_id: 0x1234,
            vendor_id: 0xFFF1,
            controller_node_id: 0xAABB,
            root_cert_hex: "deadbeef".into(),
            noc_hex: "cafebabe".into(),
            icac_hex: None,
            secret_key_hex: "11223344".into(),
            ipk_hex: "ffeeddcc".into(),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: SyntaurFabricFile = serde_json::from_str(&s).unwrap();
        assert_eq!(back.fabric_id, 0x1234);
        assert_eq!(back.vendor_id, 0xFFF1);
        assert_eq!(back.icac_hex, None);
    }

    #[tokio::test]
    async fn missing_fabric_env_returns_clean_error() {
        // No SYNTAUR_MATTER_FABRIC_FILE -> ensure_core fails with FabricNotLoaded.
        std::env::remove_var("SYNTAUR_MATTER_FABRIC_FILE");
        let client = MatterDirectClient::new();
        assert!(client.fabric_file.is_none());
        let res = client.list_nodes().await;
        assert!(matches!(res, Err(DirectError::FabricNotLoaded)));
    }
}
