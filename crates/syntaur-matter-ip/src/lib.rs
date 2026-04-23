//! IP-side Matter commissioner for multi-admin fabric joins.
//!
//! Held-session PASE design: `connect()` spawns a dedicated spawn_blocking
//! task that establishes ONE PASE session and processes invoke commands
//! over flume channels. The Exchange is held alive across all 8
//! Commissioner state-machine steps, matching Matter spec.
//!
//! ## Architecture
//!
//! ```text
//!    ┌──────── caller (tokio runtime) ────────┐
//!    │  IpCommissionExchange::invoke(...)     │
//!    │    → cmd_tx.send(Invoke{...}).await    │
//!    │    → resp_rx.recv().await              │
//!    └────────┬───────────────────────────────┘
//!             │ flume channels
//!    ┌────────▼── spawn_blocking thread ────────┐
//!    │  futures_lite::block_on(async {          │
//!    │    let matter = Matter::new(...);        │
//!    │    let socket = async_io Udp;            │
//!    │    transport_fut = matter.run(...);      │
//!    │    driver_fut = async {                  │
//!    │      let mut ex = Exchange::initiate();  │
//!    │      PaseInitiator::initiate(passcode);  │
//!    │      loop {                              │
//!    │        match cmd_rx.recv().await {       │
//!    │          Invoke => im.invoke_single_cmd  │
//!    │            → resp_tx.send();             │
//!    │          Shutdown => break;              │
//!    │        }                                 │
//!    │      }                                   │
//!    │    };                                    │
//!    │    futures_lite::or(transport, driver)   │
//!    │  });                                     │
//!    └──────────────────────────────────────────┘
//! ```

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use syntaur_matter::commission::CommissionExchange;
use syntaur_matter::error::MatterFabricError;

/// Command sent from the caller's invoke() to the driver task.
enum DriverCmd {
    Invoke {
        cluster: u32,
        command: u32,
        payload: Vec<u8>,
    },
    Shutdown,
}

type InvokeResult = Result<Vec<u8>, MatterFabricError>;

/// Held-session IP-side commissioner. `connect()` opens PASE once; all
/// subsequent `invoke()` calls reuse that same exchange.
pub struct IpCommissionExchange {
    cmd_tx: flume::Sender<DriverCmd>,
    resp_rx: flume::Receiver<InvokeResult>,
    #[allow(dead_code)]
    cancel: Arc<AtomicBool>,
    #[allow(dead_code)]
    driver: Option<tokio::task::JoinHandle<Result<(), MatterFabricError>>>,
}

impl IpCommissionExchange {
    /// Open PASE to `peer_addr` with `passcode`, hold the session, and
    /// return an exchange ready for `Commissioner::commission` to drive.
    pub async fn connect(
        peer_addr: SocketAddr,
        passcode: u32,
    ) -> Result<Self, MatterFabricError> {
        let (cmd_tx, cmd_rx) = flume::bounded::<DriverCmd>(4);
        let (resp_tx, resp_rx) = flume::bounded::<InvokeResult>(4);
        let (ready_tx, ready_rx) = flume::bounded::<Result<(), MatterFabricError>>(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_drv = Arc::clone(&cancel);

        let driver = tokio::task::spawn_blocking(move || -> Result<(), MatterFabricError> {
            futures_lite::future::block_on(async move {
                use std::net::UdpSocket;
                use rs_matter::crypto::test_only_crypto;
                use rs_matter::dm::devices::test::{TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET};
                use rs_matter::im::client::ImClient;
                use rs_matter::im::CmdResp;
                use rs_matter::sc::pase::PaseInitiator;
                use rs_matter::tlv::TLVElement;
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
                    Err::<(), MatterFabricError>(MatterFabricError::Matter(format!(
                        "transport exited prematurely: {tres:?}"
                    )))
                };

                let driver_fut = async {
                    // 1. Establish PASE once.
                    let mut ex = match Exchange::initiate_unsecured(
                        &matter,
                        &crypto,
                        Address::Udp(peer_addr),
                    )
                    .await
                    {
                        Ok(e) => e,
                        Err(e) => {
                            let _ = ready_tx
                                .send_async(Err(MatterFabricError::Matter(format!(
                                    "unsecured exchange (pre-PASE) to {peer_addr}: {e:?}"
                                ))))
                                .await;
                            return Err::<(), MatterFabricError>(MatterFabricError::Matter(
                                format!("exchange setup failed"),
                            ));
                        }
                    };

                    if let Err(e) = PaseInitiator::initiate(&mut ex, &crypto, passcode).await {
                        let _ = ready_tx
                            .send_async(Err(MatterFabricError::Matter(format!(
                                "PASE handshake to {peer_addr} with passcode {passcode}: {e:?}"
                            ))))
                            .await;
                        return Err(MatterFabricError::Matter("PASE failed".into()));
                    }

                    // 2. Signal ready to caller.
                    let _ = ready_tx.send_async(Ok(())).await;
                    log::info!("[ip-commission] PASE established, entering command loop");

                    // 3. Process invoke commands until shutdown.
                    loop {
                        if cancel_drv.load(Ordering::Relaxed) {
                            break;
                        }
                        match cmd_rx.recv_async().await {
                            Ok(DriverCmd::Invoke {
                                cluster,
                                command,
                                payload,
                            }) => {
                                let tlv = TLVElement::new(&payload);
                                let r: InvokeResult = match ImClient::invoke_single_cmd(
                                    &mut ex, 0, cluster, command, tlv, None,
                                )
                                .await
                                {
                                    Ok(CmdResp::Cmd(data)) => Ok(data.data.raw_data().to_vec()),
                                    Ok(CmdResp::Status(s)) => {
                                        if s.status.status
                                            == rs_matter::im::IMStatusCode::Success
                                        {
                                            Ok(Vec::new())
                                        } else {
                                            Err(MatterFabricError::Matter(format!(
                                                "IM status {:?} cluster={cluster:#x} cmd={command:#x}",
                                                s.status
                                            )))
                                        }
                                    }
                                    Err(e) => Err(MatterFabricError::Matter(format!(
                                        "invoke_single_cmd cluster={cluster:#x} cmd={command:#x}: {e:?}"
                                    ))),
                                };
                                let _ = resp_tx.send_async(r).await;
                            }
                            Ok(DriverCmd::Shutdown) => break,
                            Err(_) => break, // caller dropped
                        }
                    }
                    Ok(())
                };

                futures_lite::future::or(transport_fut, driver_fut).await
            })
        });

        // Wait for PASE to be established (or fail).
        match ready_rx.recv_async().await {
            Ok(Ok(())) => log::debug!("IP commissioner PASE ready"),
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(MatterFabricError::Matter(
                    "driver task closed before PASE ready".into(),
                ));
            }
        }

        Ok(Self {
            cmd_tx,
            resp_rx,
            cancel,
            driver: Some(driver),
        })
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
        Box::pin(async move {
            self.cmd_tx
                .send_async(DriverCmd::Invoke {
                    cluster,
                    command,
                    payload,
                })
                .await
                .map_err(|_| {
                    MatterFabricError::Matter("driver task closed".into())
                })?;
            self.resp_rx
                .recv_async()
                .await
                .map_err(|_| MatterFabricError::Matter("driver task closed mid-response".into()))?
        })
    }
}

impl Drop for IpCommissionExchange {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.try_send(DriverCmd::Shutdown);
        if let Some(h) = self.driver.take() {
            h.abort();
        }
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
    //! UDP port the device is listening on.

    use std::net::{IpAddr, SocketAddr};
    use std::time::Duration;

    use mdns_sd::{ServiceDaemon, ServiceEvent};

    /// Block up to `timeout` waiting for a Matter commissionable device to
    /// appear in mDNS. Returns the first matching device's socket address.
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
                    IpAddr::V6(_) => None,
                }) {
                    return Ok(SocketAddr::new(ip, port));
                }
            }
        }
    }
}
