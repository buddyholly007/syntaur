//! Z-Wave Controller — owns the link layer and exposes typed request/
//! response pairs for the Serial API functions we need at init time.
//!
//! Every method here follows the same shape:
//!   1. Build a Request `Frame` with the function id + typed payload.
//!   2. Hand it to `LinkLayer::send_frame` — link layer owns ACK/NAK/CAN.
//!   3. Receive the Response via `LinkLayer::recv_frame`.
//!   4. Verify the function id matches and decode the payload.
//!
//! Week 2 (Track D) covers the five init-time calls the driver needs
//! before it can speak to nodes: version, capabilities, home/node id,
//! API capabilities bitmask, and the init-data snapshot (node list +
//! controller role). Command-class fan-out lands weeks 5-9.

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::frame::{Frame, FrameKind};
use crate::serial::{LinkError, LinkLayer};

// ── Serial API function constants (subset — more land per week) ─────────

pub const FUNC_SERIAL_API_GET_INIT_DATA: u8 = 0x02;
pub const FUNC_GET_CONTROLLER_CAPABILITIES: u8 = 0x05;
pub const FUNC_SERIAL_API_GET_CAPABILITIES: u8 = 0x07;
pub const FUNC_APPLICATION_COMMAND_HANDLER: u8 = 0x04;
pub const FUNC_SEND_DATA: u8 = 0x13;
pub const FUNC_GET_VERSION: u8 = 0x15;
pub const FUNC_MEMORY_GET_ID: u8 = 0x20;
pub const FUNC_GET_NODE_PROTOCOL_INFO: u8 = 0x41;

/// Default TX options for SendData: TRANSMIT_OPTION_ACK (0x01) +
/// TRANSMIT_OPTION_AUTO_ROUTE (0x04) + TRANSMIT_OPTION_EXPLORE (0x20).
/// Matches what zwave-js sends for standard frames.
pub const TX_OPTIONS_DEFAULT: u8 = 0x25;

/// TX status bytes reported by the Send-Data completion callback.
/// See spec §TransmitComplete*. Expose as constants rather than an
/// enum so upstream can pattern-match the raw byte.
pub const TX_STATUS_OK: u8 = 0x00;
pub const TX_STATUS_NO_ACK: u8 = 0x01;
pub const TX_STATUS_FAIL: u8 = 0x02;
pub const TX_STATUS_ROUTING_NOT_IDLE: u8 = 0x03;
pub const TX_STATUS_NOROUTE: u8 = 0x04;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("link error: {0}")]
    Link(#[from] LinkError),
    #[error(
        "unexpected function in response — sent {expected:#04x}, got {got:#04x}"
    )]
    FunctionMismatch { expected: u8, got: u8 },
    #[error("expected Response frame, got Request")]
    UnexpectedKind,
    #[error("payload too short for {0} — need {1} bytes, got {2}")]
    ShortPayload(&'static str, usize, usize),
    #[error("stick rejected SendData (status byte {0:#04x})")]
    SendDataRejected(u8),
    #[error("SendData transmission failed at RF level (tx_status {0:#04x})")]
    TransmitFailed(u8),
    #[error(
        "SendData callback mismatch — expected callback id {expected}, got {got}"
    )]
    CallbackIdMismatch { expected: u8, got: u8 },
}

// ── Typed responses ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    /// Null-terminated ASCII version string, e.g.
    /// "Z-Wave 6.07\0" or "Z-Wave 7.17.2\0". Trailing NUL stripped here.
    pub version: String,
    /// Library type byte. `1 = Static Controller`, `2 = Controller`,
    /// `6 = Bridge Controller`, etc. per Serial API docs.
    pub library_type: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerCapabilities {
    /// Raw capability byte. Bit flags per spec:
    ///   0x01 = secondary controller
    ///   0x02 = on other network
    ///   0x04 = SIS is present
    ///   0x08 = real primary
    ///   0x10 = SUC
    pub raw: u8,
}

impl ControllerCapabilities {
    pub fn is_secondary(self) -> bool {
        self.raw & 0x01 != 0
    }
    pub fn is_sis_present(self) -> bool {
        self.raw & 0x04 != 0
    }
    pub fn is_real_primary(self) -> bool {
        self.raw & 0x08 != 0
    }
    pub fn is_suc(self) -> bool {
        self.raw & 0x10 != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeId {
    pub home_id: u32,
    /// This controller's own node_id within that home_id.
    pub node_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiCapabilities {
    pub serial_api_version: (u8, u8),
    pub manufacturer_id: u16,
    pub product_type: u16,
    pub product_id: u16,
    /// Raw bitmask (32 bytes) of which FUNC_* calls the stick supports.
    /// Function id `f` is supported iff `bitmap[(f-1)/8] >> ((f-1)%8) & 1 == 1`.
    pub function_bitmap: Vec<u8>,
}

impl ApiCapabilities {
    pub fn supports(&self, func: u8) -> bool {
        if func == 0 {
            return false;
        }
        let idx = ((func - 1) / 8) as usize;
        let bit = (func - 1) % 8;
        self.function_bitmap
            .get(idx)
            .map(|b| (b >> bit) & 1 == 1)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitData {
    /// Serial API version echoed back.
    pub version: u8,
    /// Capabilities bitmask for the Serial API itself (slave/secondary/etc).
    pub capabilities: u8,
    /// Node bitmask — 29 bytes covering nodes 1..=232. Node N is present
    /// iff `node_bitmap[(N-1)/8] >> ((N-1)%8) & 1 == 1`.
    pub node_bitmap: Vec<u8>,
    pub chip_type: u8,
    pub chip_version: u8,
}

impl InitData {
    /// All included node_ids (1..=232) present in `node_bitmap`.
    pub fn included_nodes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (byte_idx, byte) in self.node_bitmap.iter().enumerate() {
            for bit in 0..8u8 {
                if byte >> bit & 1 == 1 {
                    let node_id = (byte_idx as u16) * 8 + bit as u16 + 1;
                    if node_id <= 232 {
                        out.push(node_id as u8);
                    }
                }
            }
        }
        out
    }
}

/// Outcome of a `FUNC_ZW_SEND_DATA` call. Only returned on the
/// successful (`tx_status == TX_STATUS_OK`) path. `meta` is whatever
/// trailing bytes the stick attached to the completion callback —
/// firmware versions pack RSSI / TX repeats here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendDataReport {
    pub callback_id: u8,
    pub tx_status: u8,
    pub meta: Vec<u8>,
}

/// Per-node protocol info (returned by `FUNC_GET_NODE_PROTOCOL_INFO`).
/// Six bytes on the wire, surfaced here as typed flags + device classes
/// so callers don't have to re-parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// Raw capability byte. Bit 0x80 = listening, 0x40 = routing,
    /// 0x20 = beam-capability, 0x08 = security flag (pre-S0 indicator).
    pub capability: u8,
    /// Raw security byte. Bit 0x01 = sensor-250ms FLIRS, 0x02 = sensor-
    /// 1000ms FLIRS, 0x10 = specific device class valid, etc.
    pub security: u8,
    pub basic_device_class: u8,
    pub generic_device_class: u8,
    pub specific_device_class: u8,
}

impl NodeInfo {
    pub fn is_listening(&self) -> bool {
        self.capability & 0x80 != 0
    }
    pub fn is_routing(&self) -> bool {
        self.capability & 0x40 != 0
    }
    pub fn is_beam_capable(&self) -> bool {
        self.capability & 0x20 != 0
    }
    /// FLIRS = Frequently Listening Routing Slave — battery-powered
    /// device that wakes on a 250ms or 1000ms beam.
    pub fn is_flirs(&self) -> bool {
        self.security & 0x03 != 0
    }
}

/// Minimal cache so callers don't re-issue `GetNodeProtocolInfo` for
/// every SendData. Populated lazily by `Controller::node_info(...)` and
/// seeded by `init_data()` iteration in higher layers.
#[derive(Debug, Default, Clone)]
pub struct NodeInfoCache {
    by_node: std::collections::HashMap<u8, NodeInfo>,
}

impl NodeInfoCache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, node_id: u8) -> Option<&NodeInfo> {
        self.by_node.get(&node_id)
    }
    pub fn insert(&mut self, node_id: u8, info: NodeInfo) {
        self.by_node.insert(node_id, info);
    }
    pub fn contains(&self, node_id: u8) -> bool {
        self.by_node.contains_key(&node_id)
    }
    pub fn len(&self) -> usize {
        self.by_node.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_node.is_empty()
    }
}

// ── Controller driver ───────────────────────────────────────────────────

pub struct Controller<T> {
    link: LinkLayer<T>,
    /// Rolling callback id for SendData. Values 1..=255 cycle; 0 is
    /// reserved by the spec for "no callback requested."
    next_callback_id: u8,
    /// Per-node protocol info cache so we don't round-trip every call.
    pub node_info: NodeInfoCache,
}

impl<T> Controller<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(link: LinkLayer<T>) -> Self {
        Self {
            link,
            next_callback_id: 1,
            node_info: NodeInfoCache::new(),
        }
    }

    fn alloc_callback_id(&mut self) -> u8 {
        let id = self.next_callback_id;
        // Wrap 255 → 1 (never return 0; the spec treats 0 as
        // "fire-and-forget, no completion callback wanted").
        self.next_callback_id = if self.next_callback_id == 255 {
            1
        } else {
            self.next_callback_id + 1
        };
        id
    }

    /// Send a Request and wait for the matching Response.
    async fn call(&mut self, function: u8, payload: Vec<u8>) -> Result<Frame, ControllerError> {
        let req = Frame::request(function, payload);
        self.link.send_frame(&req).await?;
        let resp = self.link.recv_frame().await?;
        if resp.kind != FrameKind::Response {
            return Err(ControllerError::UnexpectedKind);
        }
        if resp.function != function {
            return Err(ControllerError::FunctionMismatch {
                expected: function,
                got: resp.function,
            });
        }
        Ok(resp)
    }

    /// `FUNC_GET_VERSION` — library version string + library type byte.
    pub async fn get_version(&mut self) -> Result<VersionInfo, ControllerError> {
        let resp = self.call(FUNC_GET_VERSION, vec![]).await?;
        if resp.payload.len() < 13 {
            return Err(ControllerError::ShortPayload(
                "GetVersion",
                13,
                resp.payload.len(),
            ));
        }
        // Bytes 0..11 are the version string (null-padded), byte 12 is
        // library type. (Older firmwares pad with 0x00, newer ones may
        // include more bytes — we only read the first 13.)
        let raw = &resp.payload[..12];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let version = String::from_utf8_lossy(&raw[..end])
            .trim_end()
            .to_string();
        Ok(VersionInfo {
            version,
            library_type: resp.payload[12],
        })
    }

    /// `FUNC_GET_CONTROLLER_CAPABILITIES` — one byte of capability bits.
    pub async fn get_capabilities(
        &mut self,
    ) -> Result<ControllerCapabilities, ControllerError> {
        let resp = self.call(FUNC_GET_CONTROLLER_CAPABILITIES, vec![]).await?;
        if resp.payload.is_empty() {
            return Err(ControllerError::ShortPayload(
                "GetControllerCapabilities",
                1,
                0,
            ));
        }
        Ok(ControllerCapabilities {
            raw: resp.payload[0],
        })
    }

    /// `FUNC_MEMORY_GET_ID` — 4-byte big-endian home_id + 1-byte node_id.
    pub async fn memory_get_id(&mut self) -> Result<HomeId, ControllerError> {
        let resp = self.call(FUNC_MEMORY_GET_ID, vec![]).await?;
        if resp.payload.len() < 5 {
            return Err(ControllerError::ShortPayload(
                "MemoryGetId",
                5,
                resp.payload.len(),
            ));
        }
        let home_id = u32::from_be_bytes([
            resp.payload[0],
            resp.payload[1],
            resp.payload[2],
            resp.payload[3],
        ]);
        let node_id = resp.payload[4];
        Ok(HomeId { home_id, node_id })
    }

    /// `FUNC_SERIAL_API_GET_CAPABILITIES` — api version, vendor ids, and
    /// a 32-byte bitmask of supported function ids.
    pub async fn api_capabilities(&mut self) -> Result<ApiCapabilities, ControllerError> {
        let resp = self.call(FUNC_SERIAL_API_GET_CAPABILITIES, vec![]).await?;
        if resp.payload.len() < 8 + 32 {
            return Err(ControllerError::ShortPayload(
                "SerialApiGetCapabilities",
                40,
                resp.payload.len(),
            ));
        }
        let p = &resp.payload;
        let version = (p[0], p[1]);
        let manufacturer_id = u16::from_be_bytes([p[2], p[3]]);
        let product_type = u16::from_be_bytes([p[4], p[5]]);
        let product_id = u16::from_be_bytes([p[6], p[7]]);
        let function_bitmap = p[8..8 + 32].to_vec();
        Ok(ApiCapabilities {
            serial_api_version: version,
            manufacturer_id,
            product_type,
            product_id,
            function_bitmap,
        })
    }

    /// `FUNC_GET_NODE_PROTOCOL_INFO` — per-node capability + device class.
    /// Populates `self.node_info` on success.
    pub async fn get_node_protocol_info(
        &mut self,
        node_id: u8,
    ) -> Result<NodeInfo, ControllerError> {
        let resp = self
            .call(FUNC_GET_NODE_PROTOCOL_INFO, vec![node_id])
            .await?;
        if resp.payload.len() < 6 {
            return Err(ControllerError::ShortPayload(
                "GetNodeProtocolInfo",
                6,
                resp.payload.len(),
            ));
        }
        let info = NodeInfo {
            capability: resp.payload[0],
            security: resp.payload[1],
            // payload[2] is reserved per spec.
            basic_device_class: resp.payload[3],
            generic_device_class: resp.payload[4],
            specific_device_class: resp.payload[5],
        };
        self.node_info.insert(node_id, info.clone());
        Ok(info)
    }

    /// `FUNC_ZW_SEND_DATA` — deliver a command class frame to a node.
    ///
    /// Two-phase: the stick first replies with a Response frame whose
    /// first payload byte is 1 (queued OK) or 0 (queue rejected). Then
    /// asynchronously the stick sends a Request frame with the same
    /// function id carrying `[callback_id, tx_status, ...rest]`. We
    /// block until that second frame arrives so callers get one typed
    /// outcome per call.
    ///
    /// `data` is the full command-class payload, e.g.
    /// `[COMMAND_CLASS_SWITCH_BINARY(0x25), SWITCH_BINARY_SET(0x01), value]`.
    pub async fn send_data(
        &mut self,
        node_id: u8,
        data: &[u8],
        tx_options: u8,
    ) -> Result<SendDataReport, ControllerError> {
        let callback_id = self.alloc_callback_id();

        let mut payload = Vec::with_capacity(data.len() + 4);
        payload.push(node_id);
        payload.push(data.len() as u8);
        payload.extend_from_slice(data);
        payload.push(tx_options);
        payload.push(callback_id);

        // Phase 1 — queued-or-rejected Response.
        let resp = self.call(FUNC_SEND_DATA, payload).await?;
        if resp.payload.is_empty() {
            return Err(ControllerError::ShortPayload("SendData", 1, 0));
        }
        let queued = resp.payload[0];
        if queued == 0 {
            return Err(ControllerError::SendDataRejected(queued));
        }

        // Phase 2 — async Request from the stick with our callback id.
        // The same physical link can bring other Requests (wake-up
        // notifications, application commands) in the meantime; skip
        // anything whose first payload byte isn't our callback id.
        let (tx_status, meta) = loop {
            let req = self.link.recv_frame().await?;
            if req.function != FUNC_SEND_DATA {
                // Not ours — the higher layers will eventually want an
                // event-bus for unsolicited traffic. For now, discard.
                log::debug!(
                    "[syntaur-zwave] discarded non-SendData async frame (func {:#04x})",
                    req.function
                );
                continue;
            }
            if req.payload.len() < 2 {
                return Err(ControllerError::ShortPayload(
                    "SendData.callback",
                    2,
                    req.payload.len(),
                ));
            }
            if req.payload[0] != callback_id {
                // Late callback from a prior SendData we already gave
                // up on — swallow quietly.
                continue;
            }
            break (req.payload[1], req.payload[2..].to_vec());
        };

        if tx_status == TX_STATUS_OK {
            Ok(SendDataReport {
                callback_id,
                tx_status,
                meta,
            })
        } else {
            Err(ControllerError::TransmitFailed(tx_status))
        }
    }

    // ── Application-layer helpers (Command Classes) ──────────────────

    /// Send `SWITCH_BINARY_SET` to a node. Thin wrapper around
    /// `send_data` + the CC encoder.
    pub async fn switch_binary_set(
        &mut self,
        node_id: u8,
        on: bool,
    ) -> Result<SendDataReport, ControllerError> {
        let data = crate::command_classes::SwitchBinaryCc::encode_set(on);
        self.send_data(node_id, &data, TX_OPTIONS_DEFAULT).await
    }

    /// Send `SWITCH_BINARY_GET` and parse the Report the node replies
    /// with. Waits for a SendData callback followed by a typed
    /// application-level Report Request frame carrying the state.
    ///
    /// Returns `None` if the node didn't send a Report within the
    /// callback window — the caller decides whether to retry.
    pub async fn switch_binary_get(
        &mut self,
        node_id: u8,
    ) -> Result<
        Option<crate::command_classes::switch_binary::SwitchBinaryReport>,
        ControllerError,
    > {
        let data = crate::command_classes::SwitchBinaryCc::encode_get();
        self.send_data(node_id, &data, TX_OPTIONS_DEFAULT).await?;
        Ok(self.await_application_report(
            node_id,
            crate::command_classes::CC_SWITCH_BINARY,
            crate::command_classes::switch_binary::CMD_REPORT,
            |p| crate::command_classes::SwitchBinaryCc::parse_report(p),
        )
        .await?)
    }

    /// Send `SWITCH_MULTILEVEL_SET`. `percent` is 0..=100 (100 saturates
    /// to the spec max of 99); passing None for `duration` emits the
    /// V1 single-byte form, Some(d) emits V2+ with the given ramp.
    pub async fn switch_multilevel_set(
        &mut self,
        node_id: u8,
        percent: u8,
        duration: Option<u8>,
    ) -> Result<SendDataReport, ControllerError> {
        let data = crate::command_classes::SwitchMultilevelCc::encode_set(percent, duration);
        self.send_data(node_id, &data, TX_OPTIONS_DEFAULT).await
    }

    /// Send `SWITCH_MULTILEVEL_GET` and parse the Report.
    pub async fn switch_multilevel_get(
        &mut self,
        node_id: u8,
    ) -> Result<
        Option<crate::command_classes::switch_multilevel::SwitchMultilevelReport>,
        ControllerError,
    > {
        let data = crate::command_classes::SwitchMultilevelCc::encode_get();
        self.send_data(node_id, &data, TX_OPTIONS_DEFAULT).await?;
        Ok(self.await_application_report(
            node_id,
            crate::command_classes::CC_SWITCH_MULTILEVEL,
            crate::command_classes::switch_multilevel::CMD_REPORT,
            |p| crate::command_classes::SwitchMultilevelCc::parse_report(p),
        )
        .await?)
    }

    /// Wait for an `APPLICATION_COMMAND_HANDLER` (0x04) Request frame
    /// from the stick that carries a matching (CC, command) from the
    /// given node, and feed its CC payload to `decode`.
    ///
    /// The stick sends these as unsolicited Request frames with shape:
    ///   [status, source_node_id, cc_payload_len, ...cc_payload]
    /// plus a trailing RSSI byte on newer firmwares (we don't rely on it).
    ///
    /// Returns `Ok(None)` if an unrelated Request arrived instead — the
    /// caller either retries or accepts the silence. Non-App-Command
    /// traffic (e.g. another node's wake-up) is skipped transparently.
    async fn await_application_report<R>(
        &mut self,
        node_id: u8,
        cc_id: u8,
        cmd_id: u8,
        decode: impl Fn(&[u8]) -> Option<R>,
    ) -> Result<Option<R>, ControllerError> {
        // Attempt up to 5 frames — protects against chatty neighbors.
        for _ in 0..5 {
            let req = self.link.recv_frame().await?;
            if req.function != FUNC_APPLICATION_COMMAND_HANDLER {
                log::debug!(
                    "[syntaur-zwave] skipped non-ApplicationCommand frame (func {:#04x}) while awaiting report",
                    req.function
                );
                continue;
            }
            if req.payload.len() < 3 {
                continue;
            }
            let src = req.payload[1];
            let len = req.payload[2] as usize;
            if src != node_id {
                continue;
            }
            let cc_start = 3;
            let cc_end = cc_start + len;
            if req.payload.len() < cc_end {
                continue;
            }
            let cc_bytes = &req.payload[cc_start..cc_end];
            if cc_bytes.len() < 2 || cc_bytes[0] != cc_id || cc_bytes[1] != cmd_id {
                // Same node sent a different CC update — not ours.
                continue;
            }
            return Ok(decode(cc_bytes));
        }
        Ok(None)
    }

    /// `FUNC_SERIAL_API_GET_INIT_DATA` — initial controller snapshot
    /// including the bitmask of currently included nodes.
    pub async fn init_data(&mut self) -> Result<InitData, ControllerError> {
        let resp = self.call(FUNC_SERIAL_API_GET_INIT_DATA, vec![]).await?;
        // Expected layout:
        //   0: version
        //   1: capabilities
        //   2: node bitmap length (usually 29 for 232 nodes)
        //   3..3+len: node bitmap
        //   3+len: chip_type
        //   4+len: chip_version
        if resp.payload.len() < 3 {
            return Err(ControllerError::ShortPayload(
                "SerialApiGetInitData",
                3,
                resp.payload.len(),
            ));
        }
        let version = resp.payload[0];
        let capabilities = resp.payload[1];
        let bitmap_len = resp.payload[2] as usize;
        let required = 3 + bitmap_len + 2;
        if resp.payload.len() < required {
            return Err(ControllerError::ShortPayload(
                "SerialApiGetInitData",
                required,
                resp.payload.len(),
            ));
        }
        let node_bitmap = resp.payload[3..3 + bitmap_len].to_vec();
        let chip_type = resp.payload[3 + bitmap_len];
        let chip_version = resp.payload[3 + bitmap_len + 1];
        Ok(InitData {
            version,
            capabilities,
            node_bitmap,
            chip_type,
            chip_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, Frame, ACK};

    /// Build the wire bytes for a Response frame with the given function
    /// + payload. Handy for composing tokio-test fixtures.
    fn response_bytes(function: u8, payload: &[u8]) -> Vec<u8> {
        Frame::response(function, payload.to_vec()).encode().to_vec()
    }

    /// Helper that returns the expected wire bytes for a Request frame
    /// with empty payload — every init call we make has no payload.
    fn request_bytes(function: u8) -> Vec<u8> {
        Frame::request(function, vec![]).encode().to_vec()
    }

    #[tokio::test]
    async fn get_version_happy_path() {
        // Simulated "Z-Wave 7.17.2" reply: 12 bytes of version string
        // (null-padded), then library_type byte = 6 (bridge controller).
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Z-Wave 7.17\0"); // 12 bytes incl NUL
        payload.push(0x06);
        let req = request_bytes(FUNC_GET_VERSION);
        let resp = response_bytes(FUNC_GET_VERSION, &payload);

        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK]) // we ACK their Response
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let v = ctl.get_version().await.expect("get_version");
        assert_eq!(v.version, "Z-Wave 7.17");
        assert_eq!(v.library_type, 0x06);
    }

    #[tokio::test]
    async fn get_capabilities_parses_flags() {
        let req = request_bytes(FUNC_GET_CONTROLLER_CAPABILITIES);
        // Capability byte 0x1C = real primary (0x08) + SIS present (0x04) + SUC (0x10).
        let resp = response_bytes(FUNC_GET_CONTROLLER_CAPABILITIES, &[0x1C]);
        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let caps = ctl.get_capabilities().await.expect("get_capabilities");
        assert!(caps.is_real_primary());
        assert!(caps.is_sis_present());
        assert!(caps.is_suc());
        assert!(!caps.is_secondary());
    }

    #[tokio::test]
    async fn memory_get_id_parses_home_and_node() {
        let req = request_bytes(FUNC_MEMORY_GET_ID);
        // home_id = 0xCAFEBABE, node_id = 0x01
        let resp = response_bytes(FUNC_MEMORY_GET_ID, &[0xCA, 0xFE, 0xBA, 0xBE, 0x01]);
        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let id = ctl.memory_get_id().await.expect("memory_get_id");
        assert_eq!(id.home_id, 0xCAFE_BABE);
        assert_eq!(id.node_id, 1);
    }

    #[tokio::test]
    async fn api_capabilities_decodes_fields_and_bitmap() {
        let req = request_bytes(FUNC_SERIAL_API_GET_CAPABILITIES);
        // Version (2,3), manufacturer 0x0086 (Aeotec), product_type
        // 0x0101, product_id 0x005A, then 32 bytes with bits set for
        // functions 5 (GetControllerCapabilities) and 0x20 (MemoryGetId).
        let mut payload = vec![0x02, 0x03, 0x00, 0x86, 0x01, 0x01, 0x00, 0x5A];
        let mut bitmap = vec![0u8; 32];
        // bit (5-1)=4 in byte 0
        bitmap[0] |= 1 << 4;
        // bit (0x20-1)=31 → byte 3, bit 7
        bitmap[3] |= 1 << 7;
        payload.extend_from_slice(&bitmap);
        let resp = response_bytes(FUNC_SERIAL_API_GET_CAPABILITIES, &payload);

        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let caps = ctl.api_capabilities().await.expect("api_capabilities");
        assert_eq!(caps.serial_api_version, (2, 3));
        assert_eq!(caps.manufacturer_id, 0x0086);
        assert_eq!(caps.product_type, 0x0101);
        assert_eq!(caps.product_id, 0x005A);
        assert!(caps.supports(FUNC_GET_CONTROLLER_CAPABILITIES));
        assert!(caps.supports(FUNC_MEMORY_GET_ID));
        assert!(!caps.supports(FUNC_SERIAL_API_GET_INIT_DATA));
    }

    #[tokio::test]
    async fn init_data_decodes_node_bitmap() {
        let req = request_bytes(FUNC_SERIAL_API_GET_INIT_DATA);
        // version=0x05, capabilities=0x08, bitmap_len=29
        let mut payload = vec![0x05, 0x08, 29];
        let mut bitmap = vec![0u8; 29];
        // Include controller itself (node 1) + a lamp node (node 5).
        bitmap[0] |= 1 << 0; // node 1
        bitmap[0] |= 1 << 4; // node 5
        payload.extend_from_slice(&bitmap);
        payload.push(0x06); // chip_type (ZM5101)
        payload.push(0x01); // chip_version
        let resp = response_bytes(FUNC_SERIAL_API_GET_INIT_DATA, &payload);

        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let init = ctl.init_data().await.expect("init_data");
        assert_eq!(init.version, 5);
        assert_eq!(init.capabilities, 0x08);
        assert_eq!(init.chip_type, 0x06);
        assert_eq!(init.chip_version, 0x01);
        assert_eq!(init.included_nodes(), vec![1, 5]);
    }

    #[tokio::test]
    async fn function_mismatch_errors_out() {
        // We send GetVersion but the peer replies with a different
        // function id — should surface as FunctionMismatch, not silent.
        let req = request_bytes(FUNC_GET_VERSION);
        // craft a Response frame with function=0x99, payload has 13 bytes so
        // even if mismatch check were missing the parser wouldn't trip
        let payload = vec![0u8; 13];
        let resp = response_bytes(0x99, &payload);
        let io = tokio_test::io::Builder::new()
            .write(&req)
            .read(&[ACK])
            .read(&resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let err = ctl.get_version().await.unwrap_err();
        assert!(matches!(
            err,
            ControllerError::FunctionMismatch { expected, got } if expected == FUNC_GET_VERSION && got == 0x99
        ));
    }

    #[test]
    fn api_capabilities_zero_func_not_supported() {
        let caps = ApiCapabilities {
            serial_api_version: (0, 0),
            manufacturer_id: 0,
            product_type: 0,
            product_id: 0,
            function_bitmap: vec![0xFF; 32],
        };
        // func 0 is never supported by convention.
        assert!(!caps.supports(0));
        // All other bits should be set.
        assert!(caps.supports(1));
        assert!(caps.supports(255));
    }

    #[tokio::test]
    async fn get_node_protocol_info_decodes_fields_and_caches() {
        // Request payload = [node_id=5]
        let req_bytes = Frame::request(FUNC_GET_NODE_PROTOCOL_INFO, vec![5])
            .encode()
            .to_vec();
        // Response payload = 6 bytes: cap=0xC0 (listening+routing),
        // security=0x00, reserved=0x00, basic=0x04, generic=0x11 (switch
        // multilevel), specific=0x01.
        let resp_bytes = Frame::response(
            FUNC_GET_NODE_PROTOCOL_INFO,
            vec![0xC0, 0x00, 0x00, 0x04, 0x11, 0x01],
        )
        .encode()
        .to_vec();
        let io = tokio_test::io::Builder::new()
            .write(&req_bytes)
            .read(&[ACK])
            .read(&resp_bytes)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let info = ctl
            .get_node_protocol_info(5)
            .await
            .expect("get_node_protocol_info");
        assert_eq!(info.basic_device_class, 0x04);
        assert_eq!(info.generic_device_class, 0x11);
        assert_eq!(info.specific_device_class, 0x01);
        assert!(info.is_listening());
        assert!(info.is_routing());
        assert!(!info.is_flirs());
        // Cache populated.
        assert!(ctl.node_info.contains(5));
    }

    #[tokio::test]
    async fn send_data_happy_path_two_phase() {
        // Issue SendData(node=5, data=[0x25, 0x01, 0xFF], tx_options=0x25, callback_id=1).
        let req_bytes = Frame::request(
            FUNC_SEND_DATA,
            vec![0x05, 0x03, 0x25, 0x01, 0xFF, 0x25, 0x01],
        )
        .encode()
        .to_vec();
        // Phase 1: immediate Response with "queued OK" = 0x01.
        let queued_resp = Frame::response(FUNC_SEND_DATA, vec![0x01]).encode().to_vec();
        // Phase 2: async Request from stick — [callback_id=1, tx_status=0x00 ok, ...meta].
        let cb_req = Frame::request(FUNC_SEND_DATA, vec![0x01, TX_STATUS_OK, 0x00, 0x00])
            .encode()
            .to_vec();

        let io = tokio_test::io::Builder::new()
            .write(&req_bytes)
            .read(&[ACK])
            .read(&queued_resp)
            .write(&[ACK]) // we ACK the Response
            .read(&cb_req)
            .write(&[ACK]) // we ACK the async Request
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let report = ctl
            .send_data(0x05, &[0x25, 0x01, 0xFF], TX_OPTIONS_DEFAULT)
            .await
            .expect("send_data");
        assert_eq!(report.callback_id, 1);
        assert_eq!(report.tx_status, TX_STATUS_OK);
        assert_eq!(report.meta, vec![0x00, 0x00]);
    }

    #[tokio::test]
    async fn send_data_queue_rejected_errors_out_before_phase_two() {
        let req_bytes = Frame::request(
            FUNC_SEND_DATA,
            vec![0x05, 0x03, 0x25, 0x01, 0xFF, 0x25, 0x01],
        )
        .encode()
        .to_vec();
        let queued_resp = Frame::response(FUNC_SEND_DATA, vec![0x00]).encode().to_vec(); // 0 = rejected

        let io = tokio_test::io::Builder::new()
            .write(&req_bytes)
            .read(&[ACK])
            .read(&queued_resp)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let err = ctl
            .send_data(0x05, &[0x25, 0x01, 0xFF], TX_OPTIONS_DEFAULT)
            .await
            .unwrap_err();
        assert!(matches!(err, ControllerError::SendDataRejected(0)), "got {:?}", err);
    }

    #[tokio::test]
    async fn send_data_transmit_failure_surfaces_tx_status() {
        let req_bytes = Frame::request(
            FUNC_SEND_DATA,
            vec![0x05, 0x03, 0x25, 0x01, 0xFF, 0x25, 0x01],
        )
        .encode()
        .to_vec();
        let queued_resp = Frame::response(FUNC_SEND_DATA, vec![0x01]).encode().to_vec();
        // Callback reports NO_ACK (node never replied at the RF layer).
        let cb_req = Frame::request(FUNC_SEND_DATA, vec![0x01, TX_STATUS_NO_ACK])
            .encode()
            .to_vec();
        let io = tokio_test::io::Builder::new()
            .write(&req_bytes)
            .read(&[ACK])
            .read(&queued_resp)
            .write(&[ACK])
            .read(&cb_req)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let err = ctl
            .send_data(0x05, &[0x25, 0x01, 0xFF], TX_OPTIONS_DEFAULT)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ControllerError::TransmitFailed(TX_STATUS_NO_ACK)),
            "got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn send_data_ignores_stale_callback_ids() {
        // First SendData allocates callback_id=1. The stick first
        // delivers a STALE callback with id=0x77 (from a previous
        // timed-out operation) which must be ignored, then the real
        // callback for id=1 arrives.
        let req_bytes = Frame::request(
            FUNC_SEND_DATA,
            vec![0x05, 0x03, 0x25, 0x01, 0xFF, 0x25, 0x01],
        )
        .encode()
        .to_vec();
        let queued_resp = Frame::response(FUNC_SEND_DATA, vec![0x01]).encode().to_vec();
        let stale_cb = Frame::request(FUNC_SEND_DATA, vec![0x77, TX_STATUS_FAIL])
            .encode()
            .to_vec();
        let real_cb = Frame::request(FUNC_SEND_DATA, vec![0x01, TX_STATUS_OK])
            .encode()
            .to_vec();

        let io = tokio_test::io::Builder::new()
            .write(&req_bytes)
            .read(&[ACK])
            .read(&queued_resp)
            .write(&[ACK])
            .read(&stale_cb)
            .write(&[ACK])
            .read(&real_cb)
            .write(&[ACK])
            .build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        let report = ctl
            .send_data(0x05, &[0x25, 0x01, 0xFF], TX_OPTIONS_DEFAULT)
            .await
            .expect("real callback accepted after stale one");
        assert_eq!(report.callback_id, 1);
    }

    #[test]
    fn node_info_cache_basic_invariants() {
        let mut c = NodeInfoCache::new();
        assert!(c.is_empty());
        c.insert(
            5,
            NodeInfo {
                capability: 0xC0,
                security: 0,
                basic_device_class: 0x04,
                generic_device_class: 0x11,
                specific_device_class: 0x01,
            },
        );
        assert_eq!(c.len(), 1);
        assert!(c.contains(5));
        assert_eq!(c.get(5).unwrap().generic_device_class, 0x11);
    }

    #[test]
    fn alloc_callback_id_wraps_255_to_1() {
        // The allocator wraps at 255 → 1, and never returns 0. Build a
        // controller over a no-op transport so we can poke it directly.
        let io = tokio_test::io::Builder::new().build();
        let mut ctl = Controller::new(LinkLayer::new(io));
        ctl.next_callback_id = 254;
        assert_eq!(ctl.alloc_callback_id(), 254);
        assert_eq!(ctl.alloc_callback_id(), 255);
        assert_eq!(ctl.alloc_callback_id(), 1); // wrapped, skipped 0
        assert_eq!(ctl.alloc_callback_id(), 2);
    }

    #[test]
    fn init_data_included_nodes_spans_multiple_bytes() {
        let init = InitData {
            version: 5,
            capabilities: 0,
            node_bitmap: {
                let mut b = vec![0u8; 29];
                b[0] = 0b0000_0001; // node 1
                b[1] = 0b0000_0010; // node 10 (byte 1 bit 1 → 8+1+1=10)
                b[28] = 0b0000_0001; // node 225 (bit 0 of byte 28 = 224+1)
                b
            },
            chip_type: 0,
            chip_version: 0,
        };
        assert_eq!(init.included_nodes(), vec![1, 10, 225]);
    }

    #[allow(unused)]
    fn smoke_frame_helper() {
        // Keep frame module reference so IDEs don't drop the import if
        // someone collapses this file during editing.
        let _ = frame::compute_checksum(&[]);
    }
}
