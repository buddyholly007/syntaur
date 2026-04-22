//! IP-side Matter commissioner for multi-admin fabric joins.
//!
//! Counterpart to [`syntaur_matter_ble::BleCommissionExchange`]. Where BLE
//! commissions fresh devices, IP commissioning takes an existing device
//! that another admin (HA, Apple Home, Google Home) has asked to open a
//! commissioning window — and joins it as a second admin on Syntaur's
//! fabric. No BLE involved; runs entirely over UDP against the device's
//! `_matterc._udp` commissionable listener.
//!
//! ## Flow
//!
//! ```text
//!   HA admin (existing fabric)
//!           │
//!           │  AdministratorCommissioning.OpenCommissioningWindow
//!           ▼
//!    ┌───────────────────────────────────┐
//!    │  Device enters commissioning mode │
//!    │  Broadcasts _matterc._udp mDNS    │
//!    │  Accepts PASE with new passcode   │
//!    └───────────────────────────────────┘
//!           ▲
//!           │  PASE(passcode) + 8-step Commissioner over UDP
//!           │  (ArmFailSafe → CSRRequest → AddTrustedRoot → AddNOC
//!           │   → AddOrUpdateWiFiNetwork → ConnectNetwork → CommissioningComplete)
//!           │
//!   Syntaur commissioner (this crate)
//! ```
//!
//! ## Why per-invoke PASE
//!
//! Matter spec prefers a single PASE session spanning all 8 commissioning
//! steps. The current MVP uses a fresh PASE for each step (matching the
//! BLE implementation) — devices tolerate this during OCW windows, and
//! this keeps the transport lifetime simple. If a specific device rejects
//! mid-sequence, upgrade to held-session PASE.
//!
//! See `crates/syntaur-matter-ble/src/btp.rs` for the BLE counterpart
//! and the rs-matter `Matter::run` bridging rationale.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use syntaur_matter::commission::CommissionExchange;
use syntaur_matter::error::MatterFabricError;

/// IP-side `CommissionExchange` — uses `_matterc._udp` listener as
/// commissioning transport. Each `invoke()` opens a fresh UDP PASE
/// session and runs one `ImClient::invoke_single_cmd`.
pub struct IpCommissionExchange {
    /// Pre-resolved device address (via mDNS after OCW).
    pub peer_addr: SocketAddr,
    /// Setup pin code from the OCW response.
    pub passcode: u32,
}

impl IpCommissionExchange {
    /// Construct from a known peer address + passcode. Use the `mdns`
    /// module to discover the address from a freshly-OCW'd device.
    pub fn new(peer_addr: SocketAddr, passcode: u32) -> Self {
        Self { peer_addr, passcode }
    }

    /// Core primitive: spin up rs-matter's `Matter::run` on a blocking
    /// thread, establish PASE over UDP with `passcode`, then run `op`
    /// against the authenticated exchange. Mirrors
    /// `tools/matter_direct::with_pase_op` — the stage2b primitive that
    /// has been field-tested reading BasicInformation attributes.
    pub async fn with_pase_op<R>(
        peer_addr: SocketAddr,
        passcode: u32,
        op: impl for<'e> FnOnce(
                &'e mut rs_matter::transport::exchange::Exchange<'_>,
            )
                -> Pin<Box<dyn std::future::Future<Output = Result<R, MatterFabricError>> + 'e>>
            + Send
            + 'static,
    ) -> Result<R, MatterFabricError>
    where
        R: Send + 'static,
    {
        use std::net::UdpSocket;
        use std::time::Duration;

        tokio::task::spawn_blocking(move || -> Result<R, MatterFabricError> {
            futures_lite::future::block_on(async move {
                use rs_matter::crypto::test_only_crypto;
                use rs_matter::dm::devices::test::{TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET};
                use rs_matter::sc::pase::PaseInitiator;
                use rs_matter::transport::exchange::Exchange;
                use rs_matter::transport::network::{Address, NoNetwork};
                use rs_matter::utils::epoch::sys_epoch;
                use rs_matter::Matter;

                let crypto = test_only_crypto();
                let matter =
                    Matter::new(&TEST_DEV_DET, TEST_DEV_COMM, &TEST_DEV_ATT, sys_epoch, 0);
                matter.initialize_transport_buffers().map_err(|e| {
                    MatterFabricError::Matter(format!("initialize_transport_buffers: {e:?}"))
                })?;

                let socket = async_io::Async::<UdpSocket>::bind(([0u8, 0, 0, 0], 0u16))
                    .map_err(|e| MatterFabricError::Matter(format!("udp bind: {e}")))?;

                let transport_fut = async {
                    let tres = matter.run(&crypto, &socket, &socket, NoNetwork).await;
                    Err::<R, MatterFabricError>(MatterFabricError::Matter(format!(
                        "transport exited prematurely: {tres:?}"
                    )))
                };

                let op_fut = async {
                    let mut ex = Exchange::initiate_unsecured(
                        &matter,
                        &crypto,
                        Address::Udp(peer_addr),
                    )
                    .await
                    .map_err(|e| {
                        MatterFabricError::Matter(format!(
                            "unsecured exchange (pre-PASE) to {peer_addr}: {e:?}"
                        ))
                    })?;
                    PaseInitiator::initiate(&mut ex, &crypto, passcode)
                        .await
                        .map_err(|e| {
                            MatterFabricError::Matter(format!(
                                "PASE handshake to {peer_addr} with passcode {passcode}: {e:?}"
                            ))
                        })?;
                    op(&mut ex).await
                };

                futures_lite::future::or(transport_fut, op_fut).await
            })
        })
        .await
        .map_err(|e| MatterFabricError::Matter(format!("spawn_blocking join: {e}")))?
    }
}

impl CommissionExchange for IpCommissionExchange {
    fn invoke<'a>(
        &'a mut self,
        cluster: u32,
        command: u32,
        payload: Vec<u8>,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<u8>, MatterFabricError>>
                + Send
                + 'a,
        >,
    > {
        let peer_addr = self.peer_addr;
        let passcode = self.passcode;
        Box::pin(async move {
            Self::with_pase_op::<Vec<u8>>(peer_addr, passcode, move |ex| {
                Box::pin(async move {
                    use rs_matter::im::client::ImClient;
                    use rs_matter::im::CmdResp;
                    use rs_matter::tlv::TLVElement;

                    let tlv_payload = TLVElement::new(&payload);
                    let resp = ImClient::invoke_single_cmd(
                        ex,
                        0, // endpoint 0 for commissioning cluster commands
                        cluster,
                        command,
                        tlv_payload,
                        None,
                    )
                    .await
                    .map_err(|e| {
                        MatterFabricError::Matter(format!(
                            "invoke_single_cmd cluster={cluster:#x} cmd={command:#x}: {e:?}"
                        ))
                    })?;
                    match resp {
                        CmdResp::Cmd(data) => Ok(data.data.raw_data().to_vec()),
                        CmdResp::Status(s) => {
                            if s.status.status == rs_matter::im::IMStatusCode::Success {
                                Ok(Vec::new())
                            } else {
                                Err(MatterFabricError::Matter(format!(
                                    "IM status {:?} (cluster={cluster:#x} cmd={command:#x})",
                                    s.status
                                )))
                            }
                        }
                    }
                })
            })
            .await
        })
    }
}

// ── mDNS discovery for _matterc._udp ──────────────────────────────────

pub mod mdns {
    //! Discover a freshly OCW'd Matter device by its short discriminator.
    //!
    //! Matter devices in commissioning mode advertise `_matterc._udp` with
    //! a service name like `<SHORT_DISCRIMINATOR>._matterc._udp.local` and
    //! TXT records containing `D=<discriminator>`, `CM=<commissioning_mode>`,
    //! `VP=<vendor_id>+<product_id>`. The SRV record gives the ephemeral
    //! UDP port the device is listening on (NOT 5540 — that's operational).

    use std::net::{IpAddr, SocketAddr};
    use std::time::Duration;

    use mdns_sd::{ServiceDaemon, ServiceEvent};

    /// Block up to `timeout` waiting for a Matter commissionable device to
    /// appear in mDNS. Returns the first matching device's socket address.
    /// If `want_discriminator` is Some, match exactly (full 12-bit);
    /// otherwise return the first commissionable device seen.
    pub fn discover(
        want_discriminator: Option<u16>,
        timeout: Duration,
    ) -> Result<SocketAddr, String> {
        let mdns = ServiceDaemon::new().map_err(|e| format!("mdns daemon: {e}"))?;
        let receiver = mdns
            .browse("_matterc._udp.local.")
            .map_err(|e| format!("mdns browse: {e}"))?;

        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| "mDNS discovery timed out".to_string())?;
            let evt = receiver
                .recv_timeout(remaining)
                .map_err(|e| format!("mdns recv: {e}"))?;
            if let ServiceEvent::ServiceResolved(info) = evt {
                let discriminator_match = match want_discriminator {
                    None => true,
                    Some(want) => info
                        .get_property_val_str("D")
                        .and_then(|s| s.parse::<u16>().ok())
                        .map(|d| d == want)
                        .unwrap_or(false),
                };
                if !discriminator_match {
                    continue;
                }
                let port = info.get_port();
                if let Some(ip) = info.get_addresses().iter().find_map(|a| match a {
                    IpAddr::V4(v4) => Some(IpAddr::V4(*v4)),
                    IpAddr::V6(_) => None, // prefer IPv4 for simplicity
                }) {
                    return Ok(SocketAddr::new(ip, port));
                }
            }
        }
    }
}
