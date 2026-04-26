//! BTP (Bluetooth Transport Protocol) session over GATT — Phase 4.
//!
//! Central-role implementation. Opens a GATT connection to a Matter
//! device advertising the Matter service UUID, runs the BTP handshake
//! against `C1` (write) + `C2` (indications), and exposes framed
//! send/recv of Matter SDUs for the PASE + commissioning layers above.
//!
//! ## Layering
//!
//! ```text
//!   syntaur_matter::Commissioner           ← 8-step state machine
//!          │
//!          ▼
//!   BleCommissionExchange                  ← CommissionExchange impl
//!          │   (PASE init + IM invoke over BTP — follow-on work)
//!          ▼
//!   BtpSession                             ← this file
//!          │   (handshake, fragmentation, ack, seq windowing)
//!          ▼
//!   btleplug::Peripheral                   ← cross-platform BLE central
//!          │
//!          ▼
//!   HCI adapter  (Linux: BlueZ; macOS: CoreBluetooth; Windows: WinRT)
//! ```
//!
//! ## What this implements
//!
//! - BTP header codec (Matter Core §4.17.1): flags byte with optional
//!   opcode, ack_num, seq_num, msg_len fields. Self-contained — doesn't
//!   depend on rs-matter's internal `session::packet` module (which is
//!   private).
//! - BTP handshake per Matter Core §4.17.3: `MANAGEMENT | HANDSHAKE`
//!   frame with opcode `0x6C`, 7-byte handshake payload (supported-
//!   versions bitmap + MTU + window). Response parsed into negotiated
//!   MTU + window.
//! - BTP fragmentation: first segment sets `BEGINNING_SEGMENT` + 16-bit
//!   `msg_len`; last segment sets `ENDING_SEGMENT`; middles set
//!   `CONTINUE`.
//! - Reassembly over C2 indications: fragments concatenated into the
//!   SDU buffer until `ENDING_SEGMENT` arrives.
//! - Ack policy: every received data frame acks the previous sequence
//!   number; piggybacked on the next outgoing frame when one is ready,
//!   or flushed as a standalone `ACK`-only frame when idle.
//!
//! ## What still needs hooking up
//!
//! The `CommissionExchange` impl currently returns a "not implemented"
//! error from `invoke()`. Driving PASE (`rs_matter::sc::pase::
//! PaseInitiator`) and IM (`rs_matter::im::client::ImClient::
//! invoke_single_cmd`) over the BTP transport requires wiring
//! `&BtpSession` into a rs-matter `Matter::run` network pair. That's
//! the next ~200 LoC on top.
//!
//! See vault/projects/path_c_plan.md for the broader roadmap.

use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{
    CharPropFlags, Central, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    ValueNotification, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::scan::{CommissionableDevice, MATTER_SERVICE_UUID};

// ── Matter BTP characteristic UUIDs (Core §5.4.1) ────────────────────
const BTP_CHAR_C1: Uuid = Uuid::from_u128(0x18EE2EF5_263D_4559_959F_4F9C429F9D11);
const BTP_CHAR_C2: Uuid = Uuid::from_u128(0x18EE2EF5_263D_4559_959F_4F9C429F9D12);
const BTP_CHAR_C3: Uuid = Uuid::from_u128(0x64630238_8772_45F2_B87D_748A83218F04);

// BTP management opcode for handshake (Matter Core §4.17.3).
const BTP_OPCODE_HANDSHAKE: u8 = 0x6C;

// Preferred session parameters. The peer may downgrade in its handshake
// response; we always accept the peer's choice as the session floor.
// Using Matter spec minimums (MTU=23, window=1) for the initial
// handshake — some devices silent-drop larger requests. Post-handshake
// we could renegotiate upward if the peer allows.
const BTP_PREF_MTU: u16 = 247;
const BTP_PREF_WINDOW: u8 = 4;

// Spec minimums.
const BTP_MIN_MTU: u16 = 23;
const BTP_MIN_WINDOW: u8 = 1;

// BTP segment payload = GATT MTU - 3-byte GATT ATT header.
const GATT_ATT_HEADER: u16 = 3;

// ── BTP header wire format (Matter Core §4.17.1) ─────────────────────
//
// A BTP packet is a 1-byte flag byte followed by optional fields in
// this order (presence determined by flag bits):
//
//   Flags (1 byte, always)
//   Opcode (1 byte, if MANAGEMENT set)
//   Ack Number (1 byte, if ACK set)
//   Sequence Number (1 byte, if HANDSHAKE not set)
//   Message Length (2 bytes LE, if BEGINNING_SEGMENT set and not HANDSHAKE)
//   Payload (remainder)

mod flags {
    pub const HANDSHAKE: u8 = 0x40;
    pub const MANAGEMENT: u8 = 0x20;
    pub const ACK: u8 = 0x08;
    pub const ENDING_SEGMENT: u8 = 0x04;
    pub const CONTINUE: u8 = 0x02;
    pub const BEGINNING_SEGMENT: u8 = 0x01;
}

#[derive(Debug, Default, Clone, Copy)]
struct BtpHdr {
    flags: u8,
    opcode: u8,
    ack_num: u8,
    seq_num: u8,
    msg_len: u16,
}

impl BtpHdr {
    fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.flags);
        if self.has(flags::MANAGEMENT) {
            out.push(self.opcode);
        }
        if self.has(flags::ACK) {
            out.push(self.ack_num);
        }
        if !self.has(flags::HANDSHAKE) {
            out.push(self.seq_num);
        }
        if self.has(flags::BEGINNING_SEGMENT) && !self.has(flags::HANDSHAKE) {
            out.extend_from_slice(&self.msg_len.to_le_bytes());
        }
    }
    /// Decode a header from `bytes`, returning the header + number of
    /// bytes consumed. Remaining bytes are the segment payload.
    fn decode(bytes: &[u8]) -> Result<(Self, usize), BtpError> {
        let mut i = 0;
        if bytes.is_empty() {
            return Err(BtpError::Protocol("empty frame"));
        }
        let mut h = BtpHdr::default();
        h.flags = bytes[i];
        i += 1;
        if h.has(flags::MANAGEMENT) {
            if i >= bytes.len() {
                return Err(BtpError::Protocol("truncated: opcode"));
            }
            h.opcode = bytes[i];
            i += 1;
        }
        if h.has(flags::ACK) {
            if i >= bytes.len() {
                return Err(BtpError::Protocol("truncated: ack_num"));
            }
            h.ack_num = bytes[i];
            i += 1;
        }
        if !h.has(flags::HANDSHAKE) {
            if i >= bytes.len() {
                return Err(BtpError::Protocol("truncated: seq_num"));
            }
            h.seq_num = bytes[i];
            i += 1;
        }
        if h.has(flags::BEGINNING_SEGMENT) && !h.has(flags::HANDSHAKE) {
            if i + 2 > bytes.len() {
                return Err(BtpError::Protocol("truncated: msg_len"));
            }
            h.msg_len = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
            i += 2;
        }
        Ok((h, i))
    }
}

// ── BTP handshake payloads (Matter Core §4.17.3) ─────────────────────
//
// Handshake request payload (7 bytes after the flag+opcode header):
//   Bytes 0..=3: supported-versions bitmap (little-endian u32,
//                but spec really stores 4 nibbles for up to 8 versions;
//                we only care about version 4 which is bit index 4).
//   Bytes 4..=5: client MTU (LE u16)
//   Byte 6:      client window size
//
// Handshake response payload (4 bytes after the flag+opcode header):
//   Byte 0:      selected version (nibble-packed per spec — we accept
//                as a single byte since versions are small integers).
//   Bytes 1..=2: selected MTU (LE u16)
//   Byte 3:      selected window size

fn encode_handshake_request(versions_bitmap: u32, mtu: u16, window: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    // Matter Core §4.17.3.1: handshake request flags MUST be H|M|B|E —
    // it's a complete single-segment management frame. Eve firmware
    // enforces this strictly; earlier flags=0x60 (H|M only) was
    // silently dropped without responding on C2.
    let hdr = BtpHdr {
        flags: flags::HANDSHAKE
            | flags::MANAGEMENT
            | flags::BEGINNING_SEGMENT
            | flags::ENDING_SEGMENT,
        opcode: BTP_OPCODE_HANDSHAKE,
        ..Default::default()
    };
    hdr.encode(&mut out);
    out.extend_from_slice(&versions_bitmap.to_le_bytes());
    out.extend_from_slice(&mtu.to_le_bytes());
    out.push(window);
    out
}

#[derive(Debug, Clone, Copy)]
struct HandshakeResp {
    version: u8,
    mtu: u16,
    window: u8,
}

fn decode_handshake_response(payload: &[u8]) -> Result<HandshakeResp, BtpError> {
    if payload.len() < 4 {
        return Err(BtpError::Protocol("handshake response too short"));
    }
    Ok(HandshakeResp {
        version: payload[0] & 0x0F,
        mtu: u16::from_le_bytes([payload[1], payload[2]]),
        window: payload[3],
    })
}

// ── BtpSession ────────────────────────────────────────────────────────

/// Open BTP session over GATT. Owns the btleplug peripheral, the
/// characteristic handles, the C2 notification stream, and the framing
/// state. Dropping the session disconnects the peripheral.
pub struct BtpSession {
    peripheral: Peripheral,
    /// Peer BLE address parsed into the 6-byte form rs-matter expects.
    peer_btaddr: [u8; 6],
    c1: Characteristic,
    #[allow(dead_code)] // kept for teardown bookkeeping + future resubscribe
    c2: Characteristic,
    /// Incoming C2 indications.
    notif_rx: Mutex<mpsc::UnboundedReceiver<ValueNotification>>,
    /// Handler task for C2 indications — aborted on close.
    notif_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Negotiated session parameters (set after handshake).
    state: Mutex<BtpState>,
}

#[derive(Debug, Clone, Copy)]
struct BtpState {
    mtu: u16,
    window_size: u8,
    tx_seq: u8,
    /// Highest sequence number we've observed from the peer. Sent as
    /// the `ack_num` on our next outgoing frame.
    pending_ack: Option<u8>,
}

impl Default for BtpState {
    fn default() -> Self {
        Self {
            mtu: BTP_MIN_MTU,
            window_size: BTP_MIN_WINDOW,
            tx_seq: 0,
            pending_ack: None,
        }
    }
}

impl BtpSession {
    /// Connect, discover characteristics, subscribe, handshake.
    pub async fn open(device: CommissionableDevice) -> Result<Self, BtpError> {
        log::debug!("[btp] open: Manager::new");
        let manager = Manager::new().await?;
        log::debug!("[btp] open: adapters");
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .ok_or(BtpError::NoAdapter)?;

        log::debug!("[btp] open: start_scan (refresh cache for {})", device.address);
        adapter
            .start_scan(ScanFilter {
                services: vec![MATTER_SERVICE_UUID],
            })
            .await?;
        let peripheral = {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                let peripherals = adapter.peripherals().await?;
                let found = peripherals.into_iter().find(|p| {
                    p.address()
                        .to_string()
                        .eq_ignore_ascii_case(&device.address)
                });
                if let Some(p) = found {
                    break p;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(BtpError::DeviceNotFound(device.address.clone()));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        };
        log::debug!("[btp] open: found peripheral in cache");
        let _ = adapter.stop_scan().await;
        log::debug!("[btp] open: stop_scan done");

        let pre_connected = peripheral.is_connected().await.unwrap_or(false);
        log::debug!("[btp] open: pre_connected={pre_connected}");
        if pre_connected {
            let _ = peripheral.disconnect().await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        log::debug!("[btp] open: connect()");
        match tokio::time::timeout(Duration::from_secs(15), peripheral.connect()).await {
            Ok(Ok(())) => log::debug!("[btp] open: connect OK"),
            Ok(Err(e)) => {
                log::warn!("[btp] open: connect returned btleplug error: {e:?}");
                return Err(e.into());
            }
            Err(_) => {
                log::warn!("[btp] open: connect() hung past 15s — giving up");
                return Err(BtpError::Protocol("connect() timed out at 15s"));
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        log::debug!("[btp] open: discover_services");
        match tokio::time::timeout(Duration::from_secs(10), peripheral.discover_services()).await {
            Ok(Ok(())) => log::debug!("[btp] open: discover_services OK"),
            Ok(Err(e)) => {
                log::warn!("[btp] open: discover_services error: {e:?}");
                return Err(e.into());
            }
            Err(_) => {
                log::warn!("[btp] open: discover_services hung past 10s");
                return Err(BtpError::Protocol("discover_services timed out"));
            }
        }

        let characteristics = peripheral.characteristics();
        log::debug!("[btp] open: discovered characteristics:");
        for ch in characteristics.iter() {
            log::debug!("[btp]   {} props={:?}", ch.uuid, ch.properties);
        }
        let c1 = characteristics
            .iter()
            .find(|c| c.uuid == BTP_CHAR_C1)
            .cloned()
            .ok_or(BtpError::MissingCharacteristic("C1"))?;
        let c2 = characteristics
            .iter()
            .find(|c| c.uuid == BTP_CHAR_C2)
            .cloned()
            .ok_or(BtpError::MissingCharacteristic("C2"))?;
        log::info!(
            "[btp] C1 props={:?}, C2 props={:?}",
            c1.properties,
            c2.properties
        );

        if !c1.properties.contains(CharPropFlags::WRITE)
            && !c1.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        {
            return Err(BtpError::BadCharacteristic("C1 not writable"));
        }
        if !c2.properties.contains(CharPropFlags::INDICATE)
            && !c2.properties.contains(CharPropFlags::NOTIFY)
        {
            return Err(BtpError::BadCharacteristic("C2 not indicatable"));
        }

        // Get the notification stream BEFORE subscribing so BlueZ-backed
        // indications arriving in the subscribe-ack window are not dropped.
        // Per Eve quirk (verified via M3 BTP trace 2026-04-25): subscribe()
        // is intentionally deferred until AFTER the BTP Handshake Request
        // is written — Eve only releases the queued Handshake Response
        // indication after the CCCD-enable that follows the request. See
        // do_handshake() below.
        let mut stream = peripheral.notifications().await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let c2_uuid = c2.uuid;
        let notif_task = tokio::spawn(async move {
            while let Some(n) = stream.next().await {
                if n.uuid == c2_uuid && tx.send(n).is_err() {
                    break; // receiver dropped — session closed
                }
            }
        });

        let peer_btaddr = parse_ble_address(&device.address)?;

        let session = Self {
            peripheral,
            peer_btaddr,
            c1,
            c2,
            notif_rx: Mutex::new(rx),
            notif_task: Mutex::new(Some(notif_task)),
            state: Mutex::new(BtpState::default()),
        };
        session.do_handshake().await?;
        Ok(session)
    }

    /// Run the BTP handshake. Sends a HANDSHAKE|MANAGEMENT frame with
    /// our preferred MTU/window, waits for the response on C2, and
    /// stores the negotiated minimums in `self.state`.
    async fn do_handshake(&self) -> Result<(), BtpError> {
        // Give CCCD enable time to land on the peer side — some firmware
        // (Eve, AiDot) doesn't fully register the subscription before
        // processing BTP frames. 5s is overkill but eliminates the class.
        tokio::time::sleep(Duration::from_millis(200)).await; // patched 2026-04-25: was 5000, M3 trace shows no such delay
        // Supported Versions is a list of up to 8 nibble-packed version
        // numbers, NOT a bitmap. chip-sdk sends 0x04 meaning "first
        // supported version = 4, no others". LE u32 = 0x00000004.
        let frame = encode_handshake_request(0x00000004u32, BTP_PREF_MTU, BTP_PREF_WINDOW);
        log::debug!("[btp] handshake TX {} bytes: {:02x?}", frame.len(), &frame);
        // Matter Core §4.17.3.2: writes to C1 for the Connect Handshake
        // MUST use ATT Write Request (opcode 0x12), NOT Write Command
        // (0x52). project-chip/connectedhomeip issue #3160 + PR #3161
        // fixed this exact silent-drop bug on Android CHIPTool. Eve /
        // AiDot firmware silently drops Write Command handshakes even
        // though the bytes match spec.
        self.peripheral
            .write(&self.c1, &frame, WriteType::WithResponse)
            .await?;
        log::debug!("[btp] handshake Write Request sent (per spec §4.17.3.2 / CHIP PR #3161)");

        // M3-order subscribe: enable C2 indications AFTER the request is on
        // the wire. Eve queues the Handshake Response internally and only
        // releases it once the CCCD on C2 is written 0x0002. Putting
        // subscribe() before the request — the canonical BLE pattern —
        // results in silent-drop on Eve. See nrf52840 sniffer capture
        // 2026-04-25 (eve_via_aqara_v3 frames 23772/23780/23785 = working
        // M3 order; eve_via_rust_patched frames 564/576/no-response =
        // broken pre-fix order).
        self.peripheral.subscribe(&self.c2).await?;
        log::debug!("[btp] subscribe(C2) called AFTER handshake request (Eve order quirk)");

        let resp_frame = self.recv_raw_frame(Duration::from_secs(10)).await?;
        let (hdr, consumed) = BtpHdr::decode(&resp_frame)?;
        if !hdr.has(flags::HANDSHAKE) {
            return Err(BtpError::Protocol("expected handshake response"));
        }
        let resp = decode_handshake_response(&resp_frame[consumed..])?;

        let negotiated_mtu = resp.mtu.max(BTP_MIN_MTU);
        let negotiated_window = resp.window.max(BTP_MIN_WINDOW);

        let mut state = self.state.lock().await;
        state.mtu = negotiated_mtu;
        state.window_size = negotiated_window;
        state.tx_seq = 0;
        state.pending_ack = None;
        log::info!(
            "[btp] handshake ok — version={} mtu={} window={}",
            resp.version,
            negotiated_mtu,
            negotiated_window
        );
        Ok(())
    }

    /// Send a Matter SDU — splits into BTP fragments, each with its
    /// own header, writing each to C1 in order. The first fragment
    /// carries `BEGINNING_SEGMENT + msg_len`; the last sets
    /// `ENDING_SEGMENT`; middles set `CONTINUE`.
    pub async fn send_sdu(&self, sdu: &[u8]) -> Result<(), BtpError> {
        let sdu_len: u16 = sdu
            .len()
            .try_into()
            .map_err(|_| BtpError::Protocol("SDU exceeds 65535 bytes"))?;

        let payload_cap = {
            let state = self.state.lock().await;
            // Max BTP payload = negotiated MTU - GATT ATT header - BTP header.
            // Worst-case BTP header = 5 bytes (flags + ack + seq + msg_len).
            state
                .mtu
                .saturating_sub(GATT_ATT_HEADER)
                .saturating_sub(5) as usize
        };
        if payload_cap == 0 {
            return Err(BtpError::Protocol("negotiated MTU too small"));
        }

        let mut offset = 0;
        let mut first = true;
        while offset < sdu.len() {
            let chunk_end = (offset + payload_cap).min(sdu.len());
            let is_last = chunk_end == sdu.len();

            let pending_ack = {
                let mut state = self.state.lock().await;
                state.pending_ack.take()
            };

            let seq = {
                let mut state = self.state.lock().await;
                let s = state.tx_seq;
                state.tx_seq = state.tx_seq.wrapping_add(1);
                s
            };

            let mut hdr_flags = 0u8;
            if first {
                hdr_flags |= flags::BEGINNING_SEGMENT;
            } else if !is_last {
                hdr_flags |= flags::CONTINUE;
            }
            if is_last {
                hdr_flags |= flags::ENDING_SEGMENT;
            }
            if pending_ack.is_some() {
                hdr_flags |= flags::ACK;
            }

            let hdr = BtpHdr {
                flags: hdr_flags,
                opcode: 0,
                ack_num: pending_ack.unwrap_or(0),
                seq_num: seq,
                msg_len: if first { sdu_len } else { 0 },
            };
            let mut frame = Vec::with_capacity(5 + (chunk_end - offset));
            hdr.encode(&mut frame);
            frame.extend_from_slice(&sdu[offset..chunk_end]);

            // Matter §4.17.3.2: C1 writes use ATT Write Request.
            self.peripheral
                .write(&self.c1, &frame, WriteType::WithResponse)
                .await?;

            offset = chunk_end;
            first = false;
        }
        Ok(())
    }

    /// Receive a Matter SDU — reads C2 indications until `ENDING_SEGMENT`,
    /// reassembles, and returns the body. Updates `pending_ack` so the
    /// next outgoing frame piggybacks it.
    pub async fn recv_sdu(&self, timeout: Duration) -> Result<Vec<u8>, BtpError> {
        let mut sdu: Vec<u8> = Vec::new();
        let mut expected_len: Option<u16> = None;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or(BtpError::Timeout)?;
            let frame = self.recv_raw_frame(remaining).await?;
            let (hdr, consumed) = BtpHdr::decode(&frame)?;
            if hdr.has(flags::HANDSHAKE) {
                return Err(BtpError::Protocol(
                    "unexpected handshake frame during SDU recv",
                ));
            }
            {
                let mut state = self.state.lock().await;
                state.pending_ack = Some(hdr.seq_num);
            }
            if hdr.has(flags::BEGINNING_SEGMENT) {
                expected_len = Some(hdr.msg_len);
                sdu.clear();
            }
            sdu.extend_from_slice(&frame[consumed..]);
            if hdr.has(flags::ENDING_SEGMENT) {
                if let Some(len) = expected_len {
                    if sdu.len() != usize::from(len) {
                        return Err(BtpError::Protocol(
                            "reassembled SDU length mismatch vs msg_len",
                        ));
                    }
                }
                return Ok(sdu);
            }
        }
    }

    /// Flush any pending standalone ack. Call when idle so the peer
    /// doesn't stall on its window.
    pub async fn flush_ack(&self) -> Result<(), BtpError> {
        let pending = self.state.lock().await.pending_ack.take();
        let Some(ack) = pending else { return Ok(()) };

        let seq = {
            let mut state = self.state.lock().await;
            let s = state.tx_seq;
            state.tx_seq = state.tx_seq.wrapping_add(1);
            s
        };
        let hdr = BtpHdr {
            flags: flags::ACK,
            opcode: 0,
            ack_num: ack,
            seq_num: seq,
            msg_len: 0,
        };
        let mut frame = Vec::with_capacity(4);
        hdr.encode(&mut frame);
        // Matter §4.17.3.2: C1 writes use ATT Write Request.
        self.peripheral
            .write(&self.c1, &frame, WriteType::WithResponse)
            .await?;
        Ok(())
    }

    async fn recv_raw_frame(&self, timeout: Duration) -> Result<Vec<u8>, BtpError> {
        let mut rx = self.notif_rx.lock().await;
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(n)) => {
                log::debug!("[btp] RX {} bytes: {:02x?}", n.value.len(), &n.value);
                Ok(n.value)
            }
            Ok(None) => Err(BtpError::NotificationChannelClosed),
            Err(_) => Err(BtpError::Timeout),
        }
    }

    /// Disconnect cleanly. Aborts the notification task and drops the
    /// BLE link.
    pub async fn close(self) -> Result<(), BtpError> {
        if let Some(h) = self.notif_task.lock().await.take() {
            h.abort();
        }
        let _ = self.peripheral.disconnect().await;
        Ok(())
    }
}

// ── CommissionExchange ────────────────────────────────────────────────
//
// `BleCommissionExchange` is the object the `syntaur_matter::Commissioner`
// state machine drives. It owns the BTP session; `invoke()` runs one
// cluster-command round-trip over BTP using rs-matter's PASE + IM
// machinery.
//
// Current status: the plumbing to drive rs-matter's `Matter::run` over
// a BTP-backed `NetworkSend`/`NetworkReceive` pair is follow-on work.
// The trait impl below returns a clear "not implemented" so the rest
// of the stack (fabric management, QR parsing, bridge route wiring)
// builds green end-to-end and the remaining work is isolated.

pub struct BleCommissionExchange {
    session: Arc<BtpSession>,
    #[allow(dead_code)]
    passcode: u32,
    pub(crate) pase_handle: tokio::sync::Mutex<Option<PaseHandle>>,
}

impl BleCommissionExchange {
    /// Open BLE → BTP session. `passcode` is the device's setup pin
    /// code from the QR / manual code; stored for the PASE stage that
    /// runs inside `invoke()` once the Matter::run plumbing lands.
    pub async fn connect(
        device: CommissionableDevice,
        passcode: u32,
    ) -> Result<Self, BtpError> {
        let session = Arc::new(BtpSession::open(device).await?);
        Ok(Self { session, passcode, pase_handle: tokio::sync::Mutex::new(None) })
    }

    /// Access the underlying BTP session (tests / diagnostics).
    pub fn session(&self) -> &BtpSession {
        &self.session
    }
}

/// Inflight command sent from `CommissionExchange::*` to the long-running
/// PASE worker. Two variants: standard IM invokes, and the post-PASE CASE
/// handshake + CommissioningComplete pair (needed because CommissioningComplete
/// must run on CASE per Matter Core §11.10.6.6).
pub enum InvokeCmd {
    Im {
        cluster: u32,
        command: u32,
        payload: Vec<u8>,
        response_tx: tokio::sync::oneshot::Sender<
            Result<Vec<u8>, syntaur_matter::error::MatterFabricError>,
        >,
    },
    CaseAndComplete {
        fabric_id: u64,
        controller_node_id: u64,
        peer_node_id: u64,
        rcac: Vec<u8>,
        ca_secret_key_scalar: [u8; 32],
        ipk: [u8; 16],
        vendor_id: u16,
        response_tx: tokio::sync::oneshot::Sender<
            Result<(), syntaur_matter::error::MatterFabricError>,
        >,
    },
}

/// Holds onto the channel used to dispatch invokes onto the cached PASE
/// session, plus the cancel flag + handles needed for orderly teardown
/// when this `BleCommissionExchange` is dropped.
pub struct PaseHandle {
    pub cmd_tx: flume::Sender<InvokeCmd>,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub _outbound: tokio::task::JoinHandle<()>,
    pub _inbound: tokio::task::JoinHandle<()>,
    pub _blocking: tokio::task::JoinHandle<Result<(), syntaur_matter::error::MatterFabricError>>,
}

impl Drop for PaseHandle {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self._outbound.abort();
        self._inbound.abort();
        // _blocking will exit when its channels close.
    }
}

impl syntaur_matter::commission::CommissionExchange for BleCommissionExchange {
    fn invoke<'a>(
        &'a mut self,
        cluster: u32,
        command: u32,
        payload: Vec<u8>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<u8>, syntaur_matter::error::MatterFabricError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let cmd_tx = self.ensure_pase().await?;
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            cmd_tx
                .send_async(InvokeCmd::Im {
                    cluster,
                    command,
                    payload,
                    response_tx,
                })
                .await
                .map_err(|e| {
                    syntaur_matter::error::MatterFabricError::Matter(format!(
                        "invoke cmd queue closed: {e}"
                    ))
                })?;
            response_rx.await.map_err(|e| {
                syntaur_matter::error::MatterFabricError::Matter(format!(
                    "invoke response channel dropped: {e}"
                ))
            })?
        })
    }

    fn case_and_commissioning_complete<'a>(
        &'a mut self,
        fabric: &'a syntaur_matter::FabricHandle,
        peer_node_id: u64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<(), syntaur_matter::error::MatterFabricError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            use syntaur_matter::error::MatterFabricError;
            let cmd_tx = self.ensure_pase().await?;
            let rcac = hex::decode(&fabric.root_cert_hex).map_err(|e| {
                MatterFabricError::Matter(format!("rcac hex decode: {e}"))
            })?;
            let ca_decoded = hex::decode(&fabric.ca_secret_key_hex).map_err(|e| {
                MatterFabricError::Matter(format!("ca key hex decode: {e}"))
            })?;
            if ca_decoded.len() != 32 {
                return Err(MatterFabricError::Matter(format!(
                    "ca key wrong length: {}B",
                    ca_decoded.len()
                )));
            }
            let mut ca_secret_key_scalar = [0u8; 32];
            ca_secret_key_scalar.copy_from_slice(&ca_decoded);
            let ipk_decoded = hex::decode(&fabric.ipk_hex).map_err(|e| {
                MatterFabricError::Matter(format!("ipk hex decode: {e}"))
            })?;
            if ipk_decoded.len() != 16 {
                return Err(MatterFabricError::Matter(format!(
                    "ipk wrong length: {}B",
                    ipk_decoded.len()
                )));
            }
            let mut ipk = [0u8; 16];
            ipk.copy_from_slice(&ipk_decoded);

            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            cmd_tx
                .send_async(InvokeCmd::CaseAndComplete {
                    fabric_id: fabric.fabric_id,
                    controller_node_id: fabric.controller_node_id,
                    peer_node_id,
                    rcac,
                    ca_secret_key_scalar,
                    ipk,
                    vendor_id: fabric.vendor_id,
                    response_tx,
                })
                .await
                .map_err(|e| {
                    MatterFabricError::Matter(format!("case cmd queue closed: {e}"))
                })?;
            response_rx.await.map_err(|e| {
                MatterFabricError::Matter(format!(
                    "case response channel dropped: {e}"
                ))
            })?
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BtpError {
    #[error("no BLE adapter found on this host")]
    NoAdapter,
    #[error("device {0} not visible in rescan (stale advertisement?)")]
    DeviceNotFound(String),
    #[error("required characteristic {0} not present on peer")]
    MissingCharacteristic(&'static str),
    #[error("characteristic misconfigured: {0}")]
    BadCharacteristic(&'static str),
    #[error("BTP protocol: {0}")]
    Protocol(&'static str),
    #[error("btleplug: {0}")]
    Ble(#[from] btleplug::Error),
    #[error("notification channel closed before SDU complete")]
    NotificationChannelClosed,
    #[error("timed out waiting for BTP frame")]
    Timeout,
}

// ── Phase 4b: rs-matter PASE + IM integration over BTP ────────────────
//
// `Matter::run` drives packet I/O through `NetworkSend` + `NetworkReceive`
// traits. We provide a `BtpBridge` that implements both by routing
// through flume channels to pump tasks running on the main tokio
// runtime (where btleplug lives). The rs-matter side runs on a
// `spawn_blocking` thread under `futures_lite::block_on` to sidestep
// rs-matter's !Send RefCell internals that multi-thread tokio can't
// carry across await boundaries.
//
// Bridge layout:
//
//     ┌──────────────── main tokio runtime ────────────────┐
//     │  Arc<BtpSession> (btleplug + C1/C2 I/O)            │
//     │       ▲                                            │
//     │       │                                            │
//     │  ┌────┴───────────┐      ┌────────────────────┐   │
//     │  │ outbound pump  │◄──── │ flume: to_ble_rx   │   │
//     │  │ send_sdu       │      │                    │   │
//     │  └────────────────┘      └────────────────────┘   │
//     │  ┌────────────────┐      ┌────────────────────┐   │
//     │  │ inbound pump   │────► │ flume: from_ble_tx │   │
//     │  │ recv_sdu       │      │                    │   │
//     │  └────────────────┘      └────────────────────┘   │
//     └────────────────────────────────────────────────────┘
//                                ▲                  ▲
//                                │                  │
//     ┌── spawn_blocking ────────┴──────────────────┴──────┐
//     │  BtpBridge { to_ble_tx, from_ble_rx }              │
//     │     impl NetworkSend + NetworkReceive              │
//     │           │                                        │
//     │           ▼                                        │
//     │  Matter::run(&crypto, &bridge, &bridge, NoNetwork) │
//     │  Exchange::initiate_unsecured(Address::Btp(...))   │
//     │  PaseInitiator::initiate(passcode)                 │
//     │  ImClient::invoke_single_cmd(...)                  │
//     └────────────────────────────────────────────────────┘

/// Parse a btleplug address string ("AA:BB:CC:DD:EE:FF" on Linux) into
/// the 6-byte form rs-matter's `BtAddr` wraps. Returns Protocol error
/// on non-Linux opaque UUIDs — real commissioning is currently Linux-
/// targeted (HAOS SSH add-on).
fn parse_ble_address(s: &str) -> Result<[u8; 6], BtpError> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(BtpError::Protocol(
            "peer BLE address not in MAC format (HAOS/Linux only for now)",
        ));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16)
            .map_err(|_| BtpError::Protocol("invalid hex in BLE address"))?;
    }
    Ok(out)
}

/// rs-matter NetworkSend + NetworkReceive bridge. Lives on the
/// spawn_blocking thread; talks to the main-runtime pumps over flume.
struct BtpBridge {
    peer_addr: rs_matter::transport::network::Address,
    to_ble_tx: flume::Sender<Vec<u8>>,
    from_ble_rx: flume::Receiver<Vec<u8>>,
    /// Buffered SDU drained by `wait_available` + `recv_from` pair.
    buffered: std::sync::Mutex<Option<Vec<u8>>>,
}

impl rs_matter::transport::network::NetworkSend for &BtpBridge {
    async fn send_to(
        &mut self,
        data: &[u8],
        _addr: rs_matter::transport::network::Address,
    ) -> Result<(), rs_matter::error::Error> {
        self.to_ble_tx
            .send_async(data.to_vec())
            .await
            .map_err(|_| {
                rs_matter::error::Error::new(rs_matter::error::ErrorCode::NoNetworkInterface)
            })
    }
}

impl rs_matter::transport::network::NetworkReceive for &BtpBridge {
    async fn wait_available(&mut self) -> Result<(), rs_matter::error::Error> {
        // Fast path: already buffered.
        {
            let guard = self.buffered.lock().unwrap();
            if guard.is_some() {
                return Ok(());
            }
        }
        let pkt = self.from_ble_rx.recv_async().await.map_err(|_| {
            rs_matter::error::Error::new(rs_matter::error::ErrorCode::NoNetworkInterface)
        })?;
        *self.buffered.lock().unwrap() = Some(pkt);
        Ok(())
    }

    async fn recv_from(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<(usize, rs_matter::transport::network::Address), rs_matter::error::Error> {
        let pkt = {
            let mut guard = self.buffered.lock().unwrap();
            guard.take()
        };
        let pkt = match pkt {
            Some(p) => p,
            None => self.from_ble_rx.recv_async().await.map_err(|_| {
                rs_matter::error::Error::new(rs_matter::error::ErrorCode::NoNetworkInterface)
            })?,
        };
        let n = pkt.len();
        if n > buffer.len() {
            return Err(rs_matter::error::Error::new(
                rs_matter::error::ErrorCode::BufferTooSmall,
            ));
        }
        buffer[..n].copy_from_slice(&pkt);
        Ok((n, self.peer_addr))
    }
}

impl BleCommissionExchange {
    /// Internal helper: run `op` over a PASE-established exchange,
    /// returning whatever the op returns. Mirrors
    /// `tools::matter_direct::with_pase_op` but routes through the
    /// BTP bridge rather than UDP.
    /// Ensure the cached PASE session is running. Spawns the long-lived
    /// blocking task on first call; subsequent calls return the existing
    /// channel sender. The blocking task does PASE once and then loops
    /// dispatching `InvokeCmd`s onto fresh secured exchanges (each
    /// invoke gets its own Exchange via `Exchange::initiate(_, 0, 0, true)`
    /// on the SAME PASE-secured session — required because rs-matter
    /// exchanges are single-use).
    async fn ensure_pase(
        &self,
    ) -> Result<flume::Sender<InvokeCmd>, syntaur_matter::error::MatterFabricError> {
        let mut guard = self.pase_handle.lock().await;
        if let Some(h) = guard.as_ref() {
            return Ok(h.cmd_tx.clone());
        }

        let session = Arc::clone(&self.session);
        let passcode = self.passcode;
        let peer_btaddr = session.peer_btaddr;

        // BTP bridge channels (matter <-> BTP I/O).
        let (to_ble_tx, to_ble_rx) = flume::bounded::<Vec<u8>>(8);
        let (from_ble_tx, from_ble_rx) = flume::bounded::<Vec<u8>>(8);
        // Invoke command queue (CommissionExchange::invoke -> blocking task).
        let (cmd_tx, cmd_rx) = flume::bounded::<InvokeCmd>(4);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Outbound pump.
        let pump_out_session = Arc::clone(&session);
        let pump_out_cancel = std::sync::Arc::clone(&cancel);
        let outbound = tokio::spawn(async move {
            while !pump_out_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                match to_ble_rx.recv_async().await {
                    Ok(data) => {
                        if let Err(e) = pump_out_session.send_sdu(&data).await {
                            log::warn!("[btp-bridge] outbound pump error: {e}");
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Inbound pump.
        let pump_in_session = Arc::clone(&session);
        let pump_in_cancel = std::sync::Arc::clone(&cancel);
        let inbound = tokio::spawn(async move {
            while !pump_in_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                match pump_in_session.recv_sdu(Duration::from_secs(30)).await {
                    Ok(pkt) => {
                        if from_ble_tx.send_async(pkt).await.is_err() {
                            break;
                        }
                    }
                    Err(BtpError::Timeout) => continue,
                    Err(e) => {
                        log::warn!("[btp-bridge] inbound pump error: {e}");
                        break;
                    }
                }
            }
        });

        let peer_addr = rs_matter::transport::network::Address::Btp(
            rs_matter::transport::network::BtAddr(peer_btaddr),
        );

        let blocking = tokio::task::spawn_blocking(
            move || -> Result<(), syntaur_matter::error::MatterFabricError> {
                use rs_matter::crypto::test_only_crypto;
                use rs_matter::dm::devices::test::{TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET};
                use rs_matter::im::client::ImClient;
                use rs_matter::im::CmdResp;
                use rs_matter::sc::pase::PaseInitiator;
                use rs_matter::tlv::TLVElement;
                use rs_matter::transport::exchange::Exchange;
                use rs_matter::transport::network::NoNetwork;
                use rs_matter::utils::epoch::sys_epoch;
                use rs_matter::Matter;
                use syntaur_matter::error::MatterFabricError;

                let crypto = test_only_crypto();
                let matter =
                    Matter::new(&TEST_DEV_DET, TEST_DEV_COMM, &TEST_DEV_ATT, sys_epoch, 0);
                matter
                    .initialize_transport_buffers()
                    .map_err(|e| MatterFabricError::Matter(format!(
                        "initialize_transport_buffers: {e:?}"
                    )))?;

                let bridge = BtpBridge {
                    peer_addr,
                    to_ble_tx,
                    from_ble_rx,
                    buffered: std::sync::Mutex::new(None),
                };

                futures_lite::future::block_on(async move {
                    let transport_fut = async {
                        let tres = matter.run(&crypto, &bridge, &bridge, NoNetwork).await;
                        Err::<(), MatterFabricError>(MatterFabricError::Matter(format!(
                            "transport exited prematurely: {tres:?}"
                        )))
                    };

                    let cmd_loop = async {
                        // Phase 1: PASE handshake — once, on an unsecured exchange.
                        {
                            let mut ex = Exchange::initiate_unsecured(&matter, &crypto, peer_addr)
                                .await
                                .map_err(|e| MatterFabricError::Matter(format!(
                                    "unsecured exchange (pre-PASE): {e:?}"
                                )))?;
                            PaseInitiator::initiate(&mut ex, &crypto, passcode)
                                .await
                                .map_err(|e| MatterFabricError::Matter(format!(
                                    "PASE handshake: {e:?}"
                                )))?;
                            // Drop unsecured ex so subsequent invokes get the new secured session.
                        }

                        // Phase 2: dispatch loop.
                        while let Ok(cmd) = cmd_rx.recv_async().await {
                            match cmd {
                                InvokeCmd::Im { cluster, command, payload, response_tx } => {
                                    let result = async {
                                        let mut ex2 = Exchange::initiate(&matter, 0, 0, true)
                                            .await
                                            .map_err(|e| MatterFabricError::Matter(format!(
                                                "secured exchange (post-PASE): {e:?}"
                                            )))?;
                                        let tlv_payload = TLVElement::new(&payload);
                                        let resp = ImClient::invoke_single_cmd(
                                            &mut ex2,
                                            0,
                                            cluster,
                                            command,
                                            tlv_payload,
                                            None,
                                        )
                                        .await
                                        .map_err(|e| MatterFabricError::Matter(format!(
                                            "invoke_single_cmd cluster={:#x} cmd={:#x}: {e:?}",
                                            cluster, command
                                        )))?;
                                        match resp {
                                            CmdResp::Cmd(data) => Ok(data.data.raw_data().to_vec()),
                                            CmdResp::Status(s) => {
                                                if s.status.status == rs_matter::im::IMStatusCode::Success {
                                                    Ok(Vec::new())
                                                } else {
                                                    Err(MatterFabricError::Matter(format!(
                                                        "IM status {:?} (cluster={:#x} cmd={:#x})",
                                                        s.status, cluster, command
                                                    )))
                                                }
                                            }
                                        }
                                    }
                                    .await;
                                    let _ = response_tx.send(result);
                                }
                                InvokeCmd::CaseAndComplete {
                                    fabric_id,
                                    controller_node_id,
                                    peer_node_id,
                                    rcac,
                                    ca_secret_key_scalar,
                                    ipk,
                                    vendor_id,
                                    response_tx,
                                } => {
                                    let result = case_and_commissioning_complete_impl(
                                        &matter,
                                        &crypto,
                                        peer_addr,
                                        fabric_id,
                                        controller_node_id,
                                        peer_node_id,
                                        &rcac,
                                        &ca_secret_key_scalar,
                                        &ipk,
                                        vendor_id,
                                    )
                                    .await;
                                    let _ = response_tx.send(result);
                                }
                            }
                        }

                        Ok::<(), MatterFabricError>(())
                    };

                    futures_lite::future::or(transport_fut, cmd_loop).await
                })
            },
        );

        *guard = Some(PaseHandle {
            cmd_tx: cmd_tx.clone(),
            cancel,
            _outbound: outbound,
            _inbound: inbound,
            _blocking: blocking,
        });
        Ok(cmd_tx)
    }
}


// ── CASE handshake + CommissioningComplete-on-CASE helper ──────────────────
//
// Runs inside the spawn_blocking task on futures_lite::block_on. Steps:
// 1. Generate fresh controller keypair
// 2. Sign controller NOC against the fabric's RCAC using its CA scalar
// 3. Register fabric in matter.state.fabrics
// 4. Run CaseInitiator::initiate on a fresh unsecured exchange
// 5. Open the resulting CASE-secured exchange + invoke CommissioningComplete
async fn case_and_commissioning_complete_impl<'a, C>(
    matter: &rs_matter::Matter<'_>,
    crypto: &C,
    peer_addr: rs_matter::transport::network::Address,
    fabric_id: u64,
    _controller_node_id: u64,
    peer_node_id: u64,
    rcac: &[u8],
    ca_secret_key_scalar: &[u8; 32],
    ipk: &[u8; 16],
    vendor_id: u16,
) -> Result<(), syntaur_matter::error::MatterFabricError>
where
    C: rs_matter::crypto::Crypto,
{
    use rs_matter::commissioner::NocGenerator;
    use rs_matter::crypto::{
        CanonAeadKey, CanonPkcSecretKey, SecretKey, SigningSecretKey,
        AEAD_CANON_KEY_LEN,
    };
    use rs_matter::im::client::ImClient;
    use rs_matter::im::CmdResp;
    use rs_matter::sc::case::CaseInitiator;
    use rs_matter::tlv::TLVElement;
    use rs_matter::transport::exchange::Exchange;
    use std::num::NonZeroU8;
    use syntaur_matter::error::MatterFabricError;

    log::info!("[case] step 1: generating controller keypair");
    let controller_secret_key = crypto.generate_secret_key().map_err(|e| {
        MatterFabricError::Matter(format!("generate controller key: {e:?}"))
    })?;
    let mut controller_csr_buf = [0u8; 256];
    let controller_csr = controller_secret_key
        .csr(&mut controller_csr_buf)
        .map_err(|e| MatterFabricError::Matter(format!("controller csr: {e:?}")))?;
    let mut controller_secret_canon = CanonPkcSecretKey::new();
    controller_secret_key
        .write_canon(&mut controller_secret_canon)
        .map_err(|e| MatterFabricError::Matter(format!("write canon: {e:?}")))?;

    log::info!("[case] step 2: signing controller NOC");
    let ca_secret = CanonPkcSecretKey::from(ca_secret_key_scalar);
    let mut gen = NocGenerator::from_root_ca(crypto, ca_secret, rcac, fabric_id, 1)
        .map_err(|e| MatterFabricError::Matter(format!("NocGenerator::from_root_ca: {e:?}")))?;
    let controller_creds = gen
        .generate_noc(crypto, controller_csr, 1, &[])
        .map_err(|e| MatterFabricError::Matter(format!("generate controller noc: {e:?}")))?;

    log::info!("[case] step 3: registering fabric in matter.state.fabrics");
    let mut ipk_canon = CanonAeadKey::new();
    let mut ipk_arr = [0u8; AEAD_CANON_KEY_LEN];
    let copy_n = ipk_arr.len().min(ipk.len());
    ipk_arr[..copy_n].copy_from_slice(&ipk[..copy_n]);
    ipk_canon.load_from_array(&ipk_arr);

    // Fabric::fab_idx() returns NonZeroU8 directly — no Option-unwrap needed.
    let fab_idx: NonZeroU8 = matter.with_state(|state| {
        state
            .fabrics
            .add(
                crypto,
                controller_secret_canon.reference(),
                rcac,
                &controller_creds.noc,
                &[],
                Some(ipk_canon.reference()),
                vendor_id,
                1,
            )
            .map(|f| f.fab_idx())
            .map_err(|e| MatterFabricError::Matter(format!("fabrics.add: {e:?}")))
    })?;

    log::info!("[case] step 4: opening unsecured exchange for CASE handshake");
    let mut ex_unsec = Exchange::initiate_unsecured(matter, crypto, peer_addr)
        .await
        .map_err(|e| MatterFabricError::Matter(format!("unsecured ex (CASE): {e:?}")))?;

    log::info!("[case] step 5: CaseInitiator::initiate (Sigma1/2/3)");
    CaseInitiator::initiate(&mut ex_unsec, crypto, fab_idx, peer_node_id)
        .await
        .map_err(|e| MatterFabricError::Matter(format!("CASE handshake: {e:?}")))?;
    drop(ex_unsec);

    log::info!("[case] step 6: opening CASE-secured exchange + invoking CommissioningComplete");
    // Exchange::initiate takes a u8 fabric index; unwrap the NonZero.
    let mut ex_case = Exchange::initiate(matter, fab_idx.get(), peer_node_id, true)
        .await
        .map_err(|e| MatterFabricError::Matter(format!("CASE secured ex: {e:?}")))?;

    let cc_payload = syntaur_matter::tlv_build::commissioning_complete();
    let tlv = TLVElement::new(&cc_payload);
    let resp = ImClient::invoke_single_cmd(&mut ex_case, 0, 0x0030, 0x04, tlv, None)
        .await
        .map_err(|e| MatterFabricError::Matter(format!(
            "CommissioningComplete on CASE: {e:?}"
        )))?;
    match resp {
        CmdResp::Cmd(_) => {
            log::info!("[case] CommissioningComplete returned InvokeResponse — DEVICE COMMISSIONED");
            Ok(())
        }
        CmdResp::Status(s) => {
            if s.status.status == rs_matter::im::IMStatusCode::Success {
                log::info!("[case] CommissioningComplete returned Success — DEVICE COMMISSIONED");
                Ok(())
            } else {
                Err(MatterFabricError::Matter(format!(
                    "CommissioningComplete on CASE returned status: {:?}",
                    s.status
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_request_encoding() {
        let frame = encode_handshake_request(0x00000004u32, 247, 8);
        // Matter Core §4.17.3.1: handshake = H|M|B|E = 0x65
        assert_eq!(
            frame[0],
            flags::HANDSHAKE
                | flags::MANAGEMENT
                | flags::BEGINNING_SEGMENT
                | flags::ENDING_SEGMENT
        );
        assert_eq!(frame[1], BTP_OPCODE_HANDSHAKE);
        // 4 bytes version list + 2 MTU + 1 window = 7 payload bytes
        assert_eq!(frame.len(), 2 + 7);
        // Version 4 in first nibble (low-nibble of byte[2]), rest zero.
        assert_eq!([frame[2], frame[3], frame[4], frame[5]], [0x04, 0, 0, 0]);
        assert_eq!(u16::from_le_bytes([frame[6], frame[7]]), 247);
        assert_eq!(frame[8], 8);
    }

    #[test]
    fn handshake_response_decode() {
        // Minimum response: version nibble 4, MTU 64, window 2
        let payload = [0x04u8, 64, 0, 2];
        let resp = decode_handshake_response(&payload).unwrap();
        assert_eq!(resp.version, 4);
        assert_eq!(resp.mtu, 64);
        assert_eq!(resp.window, 2);
    }

    #[test]
    fn data_header_encode_decode_roundtrip() {
        let orig = BtpHdr {
            flags: flags::BEGINNING_SEGMENT | flags::ACK,
            opcode: 0,
            ack_num: 3,
            seq_num: 7,
            msg_len: 140,
        };
        let mut buf = Vec::new();
        orig.encode(&mut buf);
        // flags + ack_num + seq_num + msg_len(2) = 5 bytes
        assert_eq!(buf.len(), 5);
        let (decoded, consumed) = BtpHdr::decode(&buf).unwrap();
        assert_eq!(consumed, 5);
        assert_eq!(decoded.flags, orig.flags);
        assert_eq!(decoded.ack_num, 3);
        assert_eq!(decoded.seq_num, 7);
        assert_eq!(decoded.msg_len, 140);
    }

    #[test]
    fn middle_fragment_has_no_msg_len() {
        let orig = BtpHdr {
            flags: flags::CONTINUE,
            opcode: 0,
            ack_num: 0,
            seq_num: 8,
            msg_len: 0,
        };
        let mut buf = Vec::new();
        orig.encode(&mut buf);
        // flags + seq_num only = 2 bytes
        assert_eq!(buf.len(), 2);
        let (decoded, consumed) = BtpHdr::decode(&buf).unwrap();
        assert_eq!(consumed, 2);
        assert_eq!(decoded.seq_num, 8);
    }
}
