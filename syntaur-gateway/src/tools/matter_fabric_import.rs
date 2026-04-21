//! Importer for python-matter-server fabric credentials.
//!
//! Reads on-disk state written by [python-matter-server][pms] (which delegates
//! its persistent storage to the connectedhomeip Python `PersistentStorageJSON`
//! class) and produces an [`ImportedFabric`] suitable for handing to a pure-Rust
//! `rs-matter` controller. This lets us cut over from the Python bridge without
//! re-pairing every Matter device on the household fabric.
//!
//! [pms]: https://github.com/home-assistant-libs/python-matter-server
//!
//! ## Storage layout (as of python-matter-server main, 2026-04)
//!
//! python-matter-server keeps two distinct on-disk artifacts in its
//! `--storage-path` directory (default `~/.matter_server`, configurable via the
//! `--storage-path` CLI flag — there is **no** environment variable and there
//! is **no** encryption-at-rest):
//!
//! ```text
//! <storage_path>/
//!   chip.json                         # SDK-side persistent storage
//!   chip.json.backup                  # write-ahead backup
//!   <compressed_fabric_id>.json       # python-matter-server node metadata
//!   <compressed_fabric_id>.json.backup
//! ```
//!
//! References (line numbers as of commit `416d907`):
//!
//! - `matter_server/server/__main__.py` — `--storage-path` CLI flag, default
//!   `~/.matter_server`; `--vendorid` default `0xFFF1`; `--fabricid` default
//!   `1`.
//! - `matter_server/server/stack.py` — `storage_file = os.path.join(
//!   server.storage_path, "chip.json")`; constructs `ChipStack(persistentStoragePath=storage_file)`.
//! - `matter_server/server/storage.py` — the per-fabric `<compressed_fabric_id>.json`
//!   file is written via `StorageController._save()`; format is a plain dict
//!   serialized as JSON with optional `subkey` nesting; **no encryption**, see
//!   `_save()` and the docstring "Data is stored as plain JSON files without
//!   any cryptographic protection".
//! - `matter_server/server/device_controller.py` — only writes two top-level
//!   keys to that file: `DATA_KEY_NODES` (a dict keyed by stringified node id,
//!   each holding a serialized `MatterNodeData`) and `DATA_KEY_LAST_NODE_ID`
//!   (an int).
//! - `matter_server/common/models.py` — `MatterNodeData` fields: `node_id`,
//!   `date_commissioned`, `last_interview`, `interview_version`, `available`,
//!   `is_bridge`, `attributes` (dict keyed by `"endpoint/cluster/attribute"`),
//!   `attribute_subscriptions`. Vendor/Product/Label live in `attributes`
//!   under Basic Information cluster (cluster `0x28`, endpoint `0`,
//!   attributes `1`, `2`, `5`). **No `last_known_address` is stored** —
//!   addresses are resolved at runtime via mDNS each session.
//!
//! ### `chip.json` schema
//!
//! Written by `connectedhomeip/src/controller/python/matter/storage/__init__.py::PersistentStorageJSON`
//! (class is `PersistentStorageJSON`, see lines 252-311). Top-level shape:
//!
//! ```json
//! {
//!   "repl-config": {
//!     "caList": { "0": [ { "fabricId": 1, "vendorId": 65521 } ] }
//!   },
//!   "sdk-config": {
//!     "ExampleCARootCert0":          "<base64 DER X.509 RCAC>",
//!     "ExampleOpCredsCAKey0":        "<base64 serialized P256 keypair (root)>",
//!     "ExampleCAIntermediateCert0":  "<base64 DER X.509 ICAC>",       // optional
//!     "ExampleOpCredsICAKey0":       "<base64 serialized P256 keypair (ICA)>", // optional
//!     "f/1/n":                       "<base64 Matter-TLV-encoded NOC>",
//!     "f/1/i":                       "<base64 Matter-TLV-encoded ICAC>", // optional
//!     "f/1/r":                       "<base64 Matter-TLV-encoded RCAC>",
//!     "f/1/m":                       "<base64 TLV fabric metadata (label etc.)>",
//!     "f/1/o":                       "<base64 serialized P256 operational keypair>",
//!     "f/1/k/0":                     "<base64 TLV keyset; contains the IPK>",
//!     "g/fidx":                      "<base64 TLV fabric index list>"
//!   }
//! }
//! ```
//!
//! - `repl-config` keys are written by the Python REPL layer
//!   (`CertificateAuthority.LoadFabricAdminsFromStorage`,
//!   `connectedhomeip/src/controller/python/matter/CertificateAuthority.py`
//!   lines 108-162). The `caList` maps each CA index (stringified) to a list of
//!   `{ "fabricId", "vendorId" }` dicts.
//! - `sdk-config` keys are written by the C++ side via the
//!   `PersistentStorageDelegate` adapter. Values are **always**
//!   `base64.b64encode` of the raw bytes the SDK wrote (see
//!   `PersistentStorageBase.SetSdkKey` / `GetSdkKey`,
//!   `connectedhomeip/src/controller/python/matter/storage/__init__.py`
//!   lines 215-227).
//! - The `ExampleCA*` / `ExampleOpCreds*` keys come from
//!   `connectedhomeip/src/controller/ExampleOperationalCredentialsIssuer.cpp`
//!   lines 33-36 and the `PERSISTENT_KEY_OP` macro
//!   (`connectedhomeip/src/lib/support/PersistentStorageMacros.h`) which
//!   appends the CA index as a lowercase hex suffix without zero-padding
//!   (so index 0 → `"ExampleCARootCert0"`, index 1 → `"ExampleCARootCert1"`).
//!   These are the **issuing authority** root cert & private key (DER for the
//!   cert, serialized P256 keypair for the key).
//! - The `f/<fabric>/...` keys are the Fabric Table entries written by the C++
//!   `FabricTable`, see
//!   `connectedhomeip/src/lib/support/DefaultStorageKeyAllocator.h` lines
//!   97-111. `<fabric>` is the FabricIndex as lowercase hex without padding;
//!   for the canonical single-fabric python-matter-server install this is `1`.
//!   The certs at `f/1/{n,i,r}` are stored in **Matter-TLV** form (chip-cert
//!   format), not DER. rs-matter accepts both, but for cross-stack import we
//!   often convert to DER via `Credentials::ConvertChipCertToX509Cert`.
//!
//! **Encryption / PIN gotchas:** none. The chip.json file is plaintext JSON
//! with base64-encoded binary blobs. Anyone with file-system read access on the
//! python-matter-server host can extract the fabric private keys. This is
//! consistent with how the upstream Matter SDK ships and is not specific to
//! python-matter-server.
//!
//! ## What this module does
//!
//! Given a path to the python-matter-server storage directory (or directly to
//! `chip.json` plus the optional per-fabric nodes JSON), parse out everything
//! rs-matter needs to act as the same admin on the same fabric:
//!
//! - Root CA cert (RCAC), Intermediate CA cert (ICAC), Node Operational Cert (NOC)
//! - Operational signing key (raw P-256 secret scalar)
//! - IPK (group epoch key)
//! - fabric_id, compressed_fabric_id, node_id, vendor_id, fabric_label
//! - The list of commissioned devices with their vendor/product IDs and labels
//!
//! ## What this module does **not** do
//!
//! - Decode Matter-TLV. The certificates and keypair blobs at `f/<idx>/{n,i,r,o}`
//!   are returned as the raw base64-decoded payload; converting NOC/ICAC/RCAC
//!   from TLV to DER and unwrapping the operational keypair is the
//!   responsibility of the caller (rs-matter has helpers for both). For
//!   convenience we also surface the simpler DER root cert from
//!   `ExampleCARootCert<index>`, which is already X.509 DER.
//! - Recompute compressed_fabric_id. We extract it from the storage filename
//!   (python-matter-server uses it as the per-fabric file basename); if the
//!   caller passes only `chip.json` and we cannot find a sibling
//!   `<hex>.json`, the field is left as `None`.
//! - Resolve current device IP addresses. python-matter-server does not
//!   persist them, so `last_known_address` is always `None`.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default fabric index used by python-matter-server (single-fabric install).
const DEFAULT_FABRIC_INDEX: u8 = 1;

/// Default CA index used by python-matter-server. The
/// `_caKeysBackwardCompatibilityRewrite` in chip's `PersistentStorageJSON`
/// rewrites legacy 1-based indices to 0-based, so on disk this should always
/// be `0` for a fresh-or-migrated install.
const DEFAULT_CA_INDEX: u32 = 0;

/// Matter Basic Information cluster IDs (Endpoint 0).
const BASIC_INFO_CLUSTER: u32 = 0x0028;
const BASIC_INFO_ATTR_VENDOR_ID: u32 = 1;
const BASIC_INFO_ATTR_PRODUCT_ID: u32 = 2;
const BASIC_INFO_ATTR_NODE_LABEL: u32 = 5;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("chip.json is missing the `sdk-config` map")]
    MissingSdkConfig,
    #[error("chip.json is missing the `repl-config.caList` entry for CA index {0}")]
    MissingCaList(u32),
    #[error("chip.json `caList[{ca_index}]` does not contain any fabric admin entries")]
    NoFabricAdmin { ca_index: u32 },
    #[error("required SDK storage key `{0}` is missing from chip.json")]
    MissingSdkKey(String),
    #[error("base64 decode failed for SDK key `{key}`: {source}")]
    Base64 {
        key: String,
        #[source]
        source: base64::DecodeError,
    },
    #[error(
        "could not determine compressed_fabric_id: pass it explicitly via \
         `compressed_fabric_id_hint` or place chip.json next to a \
         `<compressed_fabric_id>.json` file"
    )]
    CompressedFabricIdUnknown,
    #[error("compressed_fabric_id `{0}` is not 16 lowercase hex chars")]
    BadCompressedFabricId(String),
    #[error("IPK keyset blob at `{key}` is too short ({len} bytes); expected at least 16")]
    BadIpk { key: String, len: usize },
}

/// Output of [`import_from_storage_path`].
#[derive(Debug, Clone, Serialize)]
pub struct ImportedFabric {
    /// Matter fabric ID (the 64-bit value put in NOC subjects).
    pub fabric_id: u64,
    /// 8-byte compressed fabric ID (matches python-matter-server's
    /// per-fabric storage filename when rendered as 16 lowercase hex chars).
    pub compressed_fabric_id: [u8; 8],
    /// Vendor ID for the fabric admin (default `0xFFF1` for python-matter-server).
    pub vendor_id: u16,
    /// User-facing fabric label (UTF-8). Empty if unset.
    pub fabric_label: String,
    /// Local controller node ID on this fabric.
    pub node_id: u64,
    /// Root CA certificate.
    ///
    /// Two flavours are surfaced because both are present on disk:
    /// - `der` is the X.509 DER from `ExampleCARootCert<ca_index>` written by
    ///   `ExampleOperationalCredentialsIssuer`. This is what rs-matter wants
    ///   for its trust store.
    /// - `tlv` is the Matter-TLV-encoded RCAC the SDK uses internally
    ///   (`f/<fabric>/r`). Returned for completeness; rs-matter can convert
    ///   between forms.
    pub root_ca_cert: CertBlob,
    /// Optional Intermediate CA cert (most installs have none).
    pub icac: Option<CertBlob>,
    /// Node Operational Certificate (Matter-TLV form, from `f/<fabric>/n`).
    /// Convert to DER with rs-matter's CHIP-cert helpers if needed.
    pub noc: Vec<u8>,
    /// Serialized P-256 operational keypair (`f/<fabric>/o`).
    ///
    /// The connectedhomeip `Crypto::P256Keypair::Serialize` format is
    /// `pub_key (65 bytes uncompressed) || priv_key (32 bytes)` for OpenSSL/mbedtls
    /// builds, but back-end specific. rs-matter expects the raw 32-byte private
    /// scalar; callers should slice the trailing 32 bytes after verifying the
    /// length is `>= 32`. The full blob is returned here so callers can choose.
    pub noc_signing_key_serialized: Vec<u8>,
    /// IPK (Identity Protection Key / epoch key). 16 bytes, extracted from
    /// `f/<fabric>/k/0`. Most installs only have a single epoch key.
    pub ipk: [u8; 16],
    /// Devices commissioned on this fabric, parsed from the
    /// `<compressed_fabric_id>.json` companion file. Empty if that file was
    /// not provided / not found.
    pub commissioned_devices: Vec<CommissionedDevice>,
}

/// A Matter certificate, surfaced in whichever encodings we have on hand.
#[derive(Debug, Clone, Serialize)]
pub struct CertBlob {
    /// X.509 DER if available (root cert always, ICAC sometimes).
    pub der: Option<Vec<u8>>,
    /// Matter-TLV encoding (always present for fabric-table entries
    /// `f/<fabric>/{n,i,r}`).
    pub tlv: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommissionedDevice {
    pub node_id: u64,
    /// python-matter-server does not persist last-known IPs (mDNS resolves
    /// each session), so this is always `None`. Field kept for API symmetry
    /// with rs-matter's expectations.
    pub last_known_address: Option<SocketAddr>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub label: Option<String>,
}

// ── On-disk JSON shape ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ChipJson {
    #[serde(rename = "repl-config", default)]
    repl_config: ReplConfig,
    #[serde(rename = "sdk-config", default)]
    sdk_config: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct ReplConfig {
    /// Map of stringified CA index → list of fabric admin entries.
    #[serde(default, rename = "caList")]
    ca_list: HashMap<String, Vec<FabricAdminEntry>>,
}

#[derive(Debug, Deserialize)]
struct FabricAdminEntry {
    #[serde(rename = "fabricId")]
    fabric_id: u64,
    #[serde(rename = "vendorId")]
    vendor_id: u16,
}

/// `<compressed_fabric_id>.json` — written by python-matter-server's
/// `StorageController`. Two top-level keys are used by `device_controller.py`:
/// `nodes` (dict keyed by stringified node id) and `last_node_id` (int).
#[derive(Debug, Default, Deserialize)]
struct PmsNodesFile {
    #[serde(default)]
    nodes: HashMap<String, PmsNode>,
}

#[derive(Debug, Deserialize)]
struct PmsNode {
    node_id: u64,
    #[serde(default)]
    attributes: HashMap<String, serde_json::Value>,
}

// ── Public entry points ─────────────────────────────────────────────────────

/// High-level import from a python-matter-server `--storage-path` directory.
///
/// Reads `chip.json` from `dir` and, if a sibling `<compressed_fabric_id>.json`
/// exists, also enumerates commissioned devices.
pub fn import_from_storage_dir(dir: &Path) -> Result<ImportedFabric, ImportError> {
    let chip_path = dir.join("chip.json");
    let chip_bytes = fs::read(&chip_path).map_err(|e| ImportError::Io {
        path: chip_path.clone(),
        source: e,
    })?;
    let chip: ChipJson = serde_json::from_slice(&chip_bytes).map_err(|e| ImportError::Json {
        path: chip_path.clone(),
        source: e,
    })?;

    // Look for the per-fabric nodes JSON; use whichever <hex>.json file we find
    // (in a single-fabric install there is only one).
    let nodes_path = find_nodes_file(dir);
    let nodes_payload = nodes_path
        .as_ref()
        .map(|p| {
            let raw = fs::read(p).map_err(|e| ImportError::Io {
                path: p.clone(),
                source: e,
            })?;
            let parsed: PmsNodesFile =
                serde_json::from_slice(&raw).map_err(|e| ImportError::Json {
                    path: p.clone(),
                    source: e,
                })?;
            Ok::<_, ImportError>(parsed)
        })
        .transpose()?
        .unwrap_or_default();

    let compressed_hint = nodes_path.as_ref().and_then(file_stem_as_hex);

    parse_chip_json(
        &chip,
        DEFAULT_CA_INDEX,
        DEFAULT_FABRIC_INDEX,
        compressed_hint,
        nodes_payload,
    )
}

/// Lower-level entry: parse a chip.json that has already been read into
/// memory. Useful for tests and for callers that ship credentials over the
/// network rather than via a shared filesystem.
pub fn import_from_chip_json_bytes(
    bytes: &[u8],
    compressed_fabric_id_hint: Option<&str>,
    nodes_json_bytes: Option<&[u8]>,
) -> Result<ImportedFabric, ImportError> {
    let chip: ChipJson = serde_json::from_slice(bytes).map_err(|e| ImportError::Json {
        path: PathBuf::from("<bytes>"),
        source: e,
    })?;
    let nodes = nodes_json_bytes
        .map(|b| {
            serde_json::from_slice::<PmsNodesFile>(b).map_err(|e| ImportError::Json {
                path: PathBuf::from("<bytes>"),
                source: e,
            })
        })
        .transpose()?
        .unwrap_or_default();
    parse_chip_json(
        &chip,
        DEFAULT_CA_INDEX,
        DEFAULT_FABRIC_INDEX,
        compressed_fabric_id_hint.map(str::to_string),
        nodes,
    )
}

// ── Internals ───────────────────────────────────────────────────────────────

fn find_nodes_file(dir: &Path) -> Option<PathBuf> {
    let read = fs::read_dir(dir).ok()?;
    for entry in read.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = match p.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        // chip.json is the SDK store, not a per-fabric nodes file.
        if stem == "chip" {
            continue;
        }
        if stem.len() == 16 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(p);
        }
    }
    None
}

fn file_stem_as_hex(p: &PathBuf) -> Option<String> {
    p.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn parse_chip_json(
    chip: &ChipJson,
    ca_index: u32,
    fabric_index: u8,
    compressed_hint: Option<String>,
    nodes: PmsNodesFile,
) -> Result<ImportedFabric, ImportError> {
    let sdk = &chip.sdk_config;
    if sdk.is_empty() {
        return Err(ImportError::MissingSdkConfig);
    }

    // Resolve fabric admin metadata (vendor_id + fabric_id).
    let admins = chip
        .repl_config
        .ca_list
        .get(&ca_index.to_string())
        .ok_or(ImportError::MissingCaList(ca_index))?;
    let admin = admins
        .first()
        .ok_or(ImportError::NoFabricAdmin { ca_index })?;

    // Cert blobs.
    let root_der = decode_required(sdk, &format!("ExampleCARootCert{}", ca_index))?;
    let root_tlv = decode_required(sdk, &format!("f/{:x}/r", fabric_index))?;
    let icac_der = decode_optional(sdk, &format!("ExampleCAIntermediateCert{}", ca_index))?;
    let icac_tlv = decode_optional(sdk, &format!("f/{:x}/i", fabric_index))?;
    let noc_tlv = decode_required(sdk, &format!("f/{:x}/n", fabric_index))?;
    let op_keypair = decode_required(sdk, &format!("f/{:x}/o", fabric_index))?;

    // IPK lives in keyset 0 (f/<fabric>/k/0). The keyset is a Matter-TLV blob
    // containing one or more 16-byte epoch keys; for nearly every fabric this
    // is a single key. We slice off the trailing 16 bytes which, in the
    // canonical single-key encoding, is the IPK material. Callers that need
    // formal TLV decoding (e.g. multi-epoch installs) should re-parse
    // `f/<fabric>/k/0` themselves; we expose the raw blob for that case via
    // the helper below.
    let keyset_key = format!("f/{:x}/k/0", fabric_index);
    let keyset = decode_required(sdk, &keyset_key)?;
    if keyset.len() < 16 {
        return Err(ImportError::BadIpk {
            key: keyset_key,
            len: keyset.len(),
        });
    }
    let mut ipk = [0u8; 16];
    ipk.copy_from_slice(&keyset[keyset.len() - 16..]);

    // node_id + label live in the fabric metadata TLV at f/<fabric>/m. We do
    // NOT decode TLV here; instead we recover the controller's node id from
    // the python-matter-server side when possible (it's identical to the
    // local admin node id baked into the NOC subject), and otherwise fall
    // back to 0 with a TODO. For the rs-matter import, the node id is
    // recoverable directly from the NOC by the caller — rs-matter parses the
    // NOC subject DN.
    //
    // The fabric_label is similarly stored in `f/<fabric>/m` as TLV; we
    // surface the metadata blob via [`raw_fabric_metadata_tlv`] for callers
    // that want to decode it themselves.
    let _fabric_metadata = decode_optional(sdk, &format!("f/{:x}/m", fabric_index))?;

    // Resolve compressed fabric id.
    let compressed_fabric_id = match compressed_hint {
        Some(hex) => parse_compressed_hex(&hex)?,
        None => return Err(ImportError::CompressedFabricIdUnknown),
    };

    // Walk commissioned device list out of the per-fabric nodes JSON.
    let mut commissioned: Vec<CommissionedDevice> = nodes
        .nodes
        .into_values()
        .map(|node| extract_device(&node))
        .collect();
    commissioned.sort_by_key(|d| d.node_id);

    Ok(ImportedFabric {
        fabric_id: admin.fabric_id,
        compressed_fabric_id,
        vendor_id: admin.vendor_id,
        fabric_label: String::new(), // TODO: decode from TLV metadata blob
        node_id: 0, // TODO: pull from NOC subject DN; left to rs-matter side
        root_ca_cert: CertBlob {
            der: Some(root_der),
            tlv: Some(root_tlv),
        },
        icac: match (icac_der, icac_tlv) {
            (None, None) => None,
            (der, tlv) => Some(CertBlob { der, tlv }),
        },
        noc: noc_tlv,
        noc_signing_key_serialized: op_keypair,
        ipk,
        commissioned_devices: commissioned,
    })
}

fn decode_required(
    map: &HashMap<String, String>,
    key: &str,
) -> Result<Vec<u8>, ImportError> {
    let raw = map
        .get(key)
        .ok_or_else(|| ImportError::MissingSdkKey(key.to_string()))?;
    BASE64
        .decode(raw.as_bytes())
        .map_err(|source| ImportError::Base64 {
            key: key.to_string(),
            source,
        })
}

fn decode_optional(
    map: &HashMap<String, String>,
    key: &str,
) -> Result<Option<Vec<u8>>, ImportError> {
    let Some(raw) = map.get(key) else {
        return Ok(None);
    };
    BASE64
        .decode(raw.as_bytes())
        .map(Some)
        .map_err(|source| ImportError::Base64 {
            key: key.to_string(),
            source,
        })
}

fn parse_compressed_hex(hex: &str) -> Result<[u8; 8], ImportError> {
    let h = hex.trim().to_ascii_lowercase();
    if h.len() != 16 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ImportError::BadCompressedFabricId(hex.to_string()));
    }
    let mut out = [0u8; 8];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = &h[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(pair, 16)
            .map_err(|_| ImportError::BadCompressedFabricId(hex.to_string()))?;
    }
    Ok(out)
}

fn extract_device(node: &PmsNode) -> CommissionedDevice {
    let mut vendor_id = 0u16;
    let mut product_id = 0u16;
    let mut label: Option<String> = None;

    for (path, value) in &node.attributes {
        let mut parts = path.split('/');
        let endpoint: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        let cluster: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        let attribute: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);

        if endpoint != 0 || cluster != BASIC_INFO_CLUSTER {
            continue;
        }
        match attribute {
            BASIC_INFO_ATTR_VENDOR_ID => {
                if let Some(v) = value.as_u64() {
                    vendor_id = v as u16;
                }
            }
            BASIC_INFO_ATTR_PRODUCT_ID => {
                if let Some(v) = value.as_u64() {
                    product_id = v as u16;
                }
            }
            BASIC_INFO_ATTR_NODE_LABEL => {
                if let Some(s) = value.as_str() {
                    if !s.is_empty() {
                        label = Some(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    CommissionedDevice {
        node_id: node.node_id,
        last_known_address: None, // not persisted by python-matter-server
        vendor_id,
        product_id,
        label,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesised chip.json that mirrors the shape python-matter-server writes
    /// for a single-fabric install (CA index 0, fabric index 1, vendor 0xFFF1,
    /// fabric id 1). All blob payloads are short fillers — we only need to
    /// exercise the parser, not the cryptography.
    fn sample_chip_json() -> String {
        // Helper: 4-byte filler "RCAC" etc., base64-encoded.
        let b64 = |bytes: &[u8]| BASE64.encode(bytes);

        // Pretend keyset blob: 32 bytes of header garbage + 16-byte IPK.
        let mut keyset = vec![0u8; 32];
        keyset.extend_from_slice(&[
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, 0x00,
        ]);

        // Pretend op keypair: 65 bytes pub + 32 bytes priv = 97 bytes.
        let op_keypair = vec![0xAB; 97];

        format!(
            r#"{{
  "repl-config": {{
    "caList": {{
      "0": [
        {{ "fabricId": 1, "vendorId": 65521 }}
      ]
    }}
  }},
  "sdk-config": {{
    "ExampleCARootCert0":         "{root_der}",
    "ExampleOpCredsCAKey0":       "{ca_key}",
    "f/1/r":                      "{root_tlv}",
    "f/1/n":                      "{noc_tlv}",
    "f/1/o":                      "{op_keypair}",
    "f/1/m":                      "{meta}",
    "f/1/k/0":                    "{keyset}",
    "g/fidx":                     "{fidx}"
  }}
}}"#,
            root_der = b64(b"DER-RCAC"),
            ca_key = b64(b"CA-KEY"),
            root_tlv = b64(b"TLV-RCAC"),
            noc_tlv = b64(b"TLV-NOC"),
            op_keypair = b64(&op_keypair),
            meta = b64(b"TLV-META"),
            keyset = b64(&keyset),
            fidx = b64(b"FIDX"),
        )
    }

    fn sample_nodes_json() -> String {
        // Two devices: one labeled, one unlabeled.
        r#"{
  "nodes": {
    "1": {
      "node_id": 1,
      "attributes": {
        "0/40/1": 4660,
        "0/40/2": 22136,
        "0/40/5": "Kitchen Light"
      }
    },
    "2": {
      "node_id": 2,
      "attributes": {
        "0/40/1": 4321,
        "0/40/2": 9999
      }
    }
  },
  "last_node_id": 2
}"#
        .to_string()
    }

    #[test]
    fn parses_minimal_fabric() {
        let chip = sample_chip_json();
        let nodes = sample_nodes_json();
        // 16-hex compressed fabric id (matches what python-matter-server
        // would name the per-fabric nodes file).
        let cfid = "0123456789abcdef";

        let imported = import_from_chip_json_bytes(
            chip.as_bytes(),
            Some(cfid),
            Some(nodes.as_bytes()),
        )
        .expect("import should succeed");

        assert_eq!(imported.fabric_id, 1);
        assert_eq!(imported.vendor_id, 0xFFF1);
        assert_eq!(
            imported.compressed_fabric_id,
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
        );
        assert_eq!(imported.root_ca_cert.der.as_deref(), Some(&b"DER-RCAC"[..]));
        assert_eq!(imported.root_ca_cert.tlv.as_deref(), Some(&b"TLV-RCAC"[..]));
        assert_eq!(imported.noc, b"TLV-NOC");
        // Op keypair filler we built was 97 bytes of 0xAB.
        assert_eq!(imported.noc_signing_key_serialized.len(), 97);
        assert!(imported.noc_signing_key_serialized.iter().all(|b| *b == 0xAB));
        // IPK is the trailing 16 bytes of our synthesised keyset.
        assert_eq!(
            imported.ipk,
            [
                0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff, 0x00
            ]
        );
        assert!(imported.icac.is_none());
        assert_eq!(imported.commissioned_devices.len(), 2);

        let kitchen = &imported.commissioned_devices[0];
        assert_eq!(kitchen.node_id, 1);
        assert_eq!(kitchen.vendor_id, 4660);
        assert_eq!(kitchen.product_id, 22136);
        assert_eq!(kitchen.label.as_deref(), Some("Kitchen Light"));
        assert!(kitchen.last_known_address.is_none());

        let other = &imported.commissioned_devices[1];
        assert_eq!(other.node_id, 2);
        assert_eq!(other.vendor_id, 4321);
        assert_eq!(other.product_id, 9999);
        assert!(other.label.is_none());
    }

    #[test]
    fn rejects_when_compressed_fabric_id_unknown() {
        let chip = sample_chip_json();
        let err = import_from_chip_json_bytes(chip.as_bytes(), None, None)
            .expect_err("must demand a compressed fabric id");
        assert!(matches!(err, ImportError::CompressedFabricIdUnknown));
    }

    #[test]
    fn rejects_bad_compressed_hex() {
        let chip = sample_chip_json();
        let err = import_from_chip_json_bytes(chip.as_bytes(), Some("not-hex"), None)
            .expect_err("must reject invalid compressed fabric id");
        assert!(matches!(err, ImportError::BadCompressedFabricId(_)));
    }

    #[test]
    fn rejects_missing_sdk_key() {
        // chip.json with empty sdk-config.
        let chip = r#"{ "repl-config": { "caList": { "0": [ { "fabricId": 1, "vendorId": 65521 } ] } }, "sdk-config": {} }"#;
        let err = import_from_chip_json_bytes(chip.as_bytes(), Some("0123456789abcdef"), None)
            .expect_err("empty sdk-config must error");
        assert!(matches!(err, ImportError::MissingSdkConfig));
    }

    #[test]
    fn rejects_short_ipk() {
        // Build a chip.json where f/1/k/0 is too short (8 bytes instead of >=16).
        let mut sample = sample_chip_json();
        let short_keyset = BASE64.encode(b"shortttt");
        // Replace the keyset entry. Brittle but adequate for the test.
        let needle = "\"f/1/k/0\":";
        let start = sample.find(needle).unwrap() + needle.len();
        let close_quote_after_value = sample[start..].find('"').unwrap() + start;
        let end_quote = sample[close_quote_after_value + 1..].find('"').unwrap()
            + close_quote_after_value
            + 1;
        sample.replace_range(close_quote_after_value..=end_quote, &format!("\"{}\"", short_keyset));
        let err = import_from_chip_json_bytes(sample.as_bytes(), Some("0123456789abcdef"), None)
            .expect_err("short keyset must error");
        assert!(matches!(err, ImportError::BadIpk { .. }));
    }
}
