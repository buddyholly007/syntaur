//! CASE handshake helper + concrete CASE-secured operations over UDP/IPv6.
//!
//! Two layers:
//!   - `with_case_op<F, R>` (line ~50): generic. Opens a fresh Matter+UDP transport,
//!     runs CASE Sigma1/2/3 to `peer_addr`, then hands a CASE-secured
//!     `Exchange` to your closure. Closure does its thing (invoke a command,
//!     read an attribute, etc.) and returns. Three-way race with transport
//!     and a 30s timeout.
//!   - `case_and_commissioning_complete_via_udp` (line ~210): the
//!     post-ConnectNetwork CommissioningComplete, built on top of `with_case_op`.
//!   - `discover_operational` (line ~30): mDNS browse `_matter._tcp` filtered
//!     by `-<NODE_ID_HEX16>`, prefer global IPv6 over fe80 link-local.
//!
//! Pattern mirrors `syntaur-gateway::tools::matter_direct::with_matter_op`:
//!   - `tokio::task::spawn_blocking` + `futures_lite::block_on` (rs-matter holds RefCells)
//!   - fresh `Matter::new` + `initialize_transport_buffers`
//!   - bind UDP socket via `async_io`
//!   - register fabric in `matter.state.fabrics`
//!   - `matter.run` as transport_fut
//!   - `CaseInitiator::initiate` then the user-supplied op closure

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::pin::Pin;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

use syntaur_matter::error::MatterFabricError;

/// Browse `_matter._tcp.local.` for an entry whose instance name ends
/// with `-<NODE_ID_HEX16>`. Returns the first resolvable IPv6 socket
/// address (preferring global/ULA over link-local). If only IPv4 is
/// present we fall back to that. Returns Err if nothing matches in
/// `timeout`.
pub fn discover_operational(
    node_id: u64,
    timeout: Duration,
) -> Result<SocketAddr, MatterFabricError> {
    let suffix = format!("-{:016X}", node_id);
    let mdns = ServiceDaemon::new()
        .map_err(|e| MatterFabricError::Matter(format!("mdns daemon: {e}")))?;
    let rx = mdns
        .browse("_matter._tcp.local.")
        .map_err(|e| MatterFabricError::Matter(format!("mdns browse: {e}")))?;
    let deadline = std::time::Instant::now() + timeout;

    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        let evt = match rx.recv_timeout(remaining) {
            Ok(e) => e,
            Err(_) => break,
        };
        if let ServiceEvent::ServiceResolved(info) = evt {
            let name = info.get_fullname().to_string().to_uppercase();
            log::info!("[case-udp] mDNS saw {name} (port={})", info.get_port());
            if !name.contains(&suffix) {
                continue;
            }
            let port = info.get_port();
            let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
            // Prefer IPv4 (works with matter_direct's IPv4-ANY binding pattern,
            // and most LAN-attached Matter devices have it). Then global IPv6
            // (Thread mesh OMR prefix), then any IPv6, then anything.
            let pick = addrs
                .iter()
                .find(|a| matches!(a, IpAddr::V4(_)))
                .or_else(|| addrs.iter().find(|a| matches!(a, IpAddr::V6(v6) if !v6.is_loopback() && !v6.is_unspecified()
                    && v6.segments()[0] != 0xfe80)))
                .or_else(|| addrs.iter().find(|a| matches!(a, IpAddr::V6(_))))
                .or_else(|| addrs.iter().next())
                .cloned();
            let _ = mdns.shutdown();
            return pick
                .map(|ip| SocketAddr::new(ip, port))
                .ok_or_else(|| {
                    MatterFabricError::Matter(format!(
                        "mdns: matched {name} but no addresses resolved"
                    ))
                });
        }
    }
    let _ = mdns.shutdown();
    Err(MatterFabricError::Matter(format!(
        "mdns: no _matter._tcp record with node {node_id:#x} ({suffix}) within {:?}",
        timeout
    )))
}

/// Generic CASE-secured op runner. Builds a fresh Matter+UDP transport,
/// runs CASE Sigma1/2/3, calls `op(&mut secured_exchange)`. 30s timeout.
///
/// The closure returns `Result<R, MatterFabricError>`; success bubbles up,
/// errors bubble up. Use this for OnOff invoke, attribute reads, etc.
///
/// Caller passes raw key material from the persisted fabric handle so we
/// avoid coupling this helper to FabricHandle's on-disk format.
/// `with_case_op_persisted` — preferred entry point. Caller passes a
/// pre-signed controller NOC + secret key (loaded from the persisted
/// fabric handle). The same controller identity is used every call,
/// which is what Eve / Meross / Aqara devices expect after the first
/// CASE handshake at CommissioningComplete time. If you don't have a
/// persisted controller NOC, use [`with_case_op`] which mints one
/// fresh per call (only valid the first time the device sees us).
pub async fn with_case_op_persisted<F, R>(
    eve_addr: SocketAddr,
    fabric_id: u64,
    peer_node_id: u64,
    rcac: Vec<u8>,
    controller_noc: Vec<u8>,
    controller_secret_key_scalar: [u8; 32],
    ipk: [u8; 16],
    vendor_id: u16,
    op: F,
) -> Result<R, MatterFabricError>
where
    F: for<'e> FnOnce(
            &'e mut rs_matter::transport::exchange::Exchange<'_>,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<R, MatterFabricError>> + 'e>>
        + Send
        + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || -> Result<R, MatterFabricError> {
        futures_lite::future::block_on(async move {
            use rs_matter::crypto::{
                default_crypto, CanonAeadKey, CanonPkcSecretKey, AEAD_CANON_KEY_LEN,
            };
            use rs_matter::dm::devices::test::{
                DAC_PRIVKEY, TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET,
            };
            use rs_matter::sc::case::CaseInitiator;
            use rs_matter::transport::exchange::Exchange;
            use rs_matter::transport::network::{Address, NoNetwork};
            use rs_matter::utils::epoch::sys_epoch;
            use rs_matter::Matter;
            use rand_core::OsRng;
            use std::num::NonZeroU8;

            // OsRng — not test_only_crypto's deterministic xor-shift.
            // Eve/Meross silently drop the second CASE Sigma1 if the
            // InitiatorRandom + ECDH ephemeral key are bit-identical to
            // the first (replay protection).
            let crypto = default_crypto(OsRng, DAC_PRIVKEY);
            let matter = Matter::new(&TEST_DEV_DET, TEST_DEV_COMM, &TEST_DEV_ATT, sys_epoch, 0);
            matter
                .initialize_transport_buffers()
                .map_err(|e| MatterFabricError::Matter(format!("initialize_transport_buffers: {e:?}")))?;

            // Reuse the persisted controller NOC + secret. Loading the
            // signing key from the pre-supplied scalar means our
            // controller identity (NOC bytes + ECDSA pubkey) is stable
            // across binary invocations — Eve/Meross/Aqara accept any
            // CASE handshake from this same identity after the first.
            let controller_secret_canon = CanonPkcSecretKey::from(&controller_secret_key_scalar);

            let mut ipk_canon = CanonAeadKey::new();
            let mut ipk_arr = [0u8; AEAD_CANON_KEY_LEN];
            let copy_n = ipk_arr.len().min(ipk.len());
            ipk_arr[..copy_n].copy_from_slice(&ipk[..copy_n]);
            ipk_canon.load_from_array(&ipk_arr);

            let fab_idx: NonZeroU8 = matter
                .with_state(|state| {
                    state
                        .fabrics
                        .add(
                            &crypto,
                            controller_secret_canon.reference(),
                            &rcac,
                            &controller_noc,
                            &[],
                            Some(ipk_canon.reference()),
                            vendor_id,
                            1,
                        )
                        .map(|f| f.fab_idx())
                        .map_err(|e| MatterFabricError::Matter(format!("fabrics.add: {e:?}")))
                })?;

            let bind_local: SocketAddr = match eve_addr {
                SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
                SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            };
            let socket = async_io::Async::<UdpSocket>::bind(bind_local).map_err(|e| {
                MatterFabricError::Matter(format!("udp bind {bind_local}: {e}"))
            })?;

            let transport_fut = async {
                let tres = matter.run(&crypto, &socket, &socket, NoNetwork).await;
                Err::<R, MatterFabricError>(MatterFabricError::Matter(format!(
                    "transport exited prematurely: {tres:?}"
                )))
            };

            let op_fut = async {
                let mut ex_unsec = Exchange::initiate_unsecured(
                    &matter,
                    &crypto,
                    Address::Udp(eve_addr),
                )
                .await
                .map_err(|e| MatterFabricError::Matter(format!("unsecured ex (CASE/UDP): {e:?}")))?;

                CaseInitiator::initiate(&mut ex_unsec, &crypto, fab_idx, peer_node_id)
                    .await
                    .map_err(|e| MatterFabricError::Matter(format!("CASE/UDP handshake: {e:?}")))?;
                drop(ex_unsec);

                let mut ex_case = Exchange::initiate(&matter, fab_idx.get(), peer_node_id, true)
                    .await
                    .map_err(|e| {
                        MatterFabricError::Matter(format!("CASE/UDP secured ex: {e:?}"))
                    })?;
                op(&mut ex_case).await
            };

            let timeout_fut = async {
                async_io::Timer::after(Duration::from_secs(30)).await;
                Err::<R, MatterFabricError>(MatterFabricError::Matter(
                    "CASE/UDP timed out after 30s".into(),
                ))
            };

            let op_or_timeout = futures_lite::future::or(op_fut, timeout_fut);
            futures_lite::future::or(transport_fut, op_or_timeout).await
        })
    })
    .await
    .map_err(|e| MatterFabricError::Matter(format!("spawn_blocking join: {e}")))?
}



/// `with_case_op` — backward-compat wrapper that mints a fresh controller
/// NOC inside the spawn_blocking task and forwards to
/// [`with_case_op_persisted`]. Use this only when the device hasn't yet
/// recorded a controller identity (i.e. during initial commissioning's
/// CommissioningComplete step). After that, use the `_persisted` variant.
pub async fn with_case_op<F, R>(
    eve_addr: SocketAddr,
    fabric_id: u64,
    peer_node_id: u64,
    rcac: Vec<u8>,
    ca_secret_key_scalar: [u8; 32],
    ipk: [u8; 16],
    vendor_id: u16,
    op: F,
) -> Result<R, MatterFabricError>
where
    F: for<'e> FnOnce(
            &'e mut rs_matter::transport::exchange::Exchange<'_>,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<R, MatterFabricError>> + 'e>>
        + Send
        + 'static,
    R: Send + 'static,
{
    // Mint controller NOC + secret in a sub-scope so the non-Send rs-matter
    // SecretKey/Crypto types don't cross the .await on with_case_op_persisted.
    let (controller_noc, controller_secret_scalar) = mint_fresh_controller_noc(
        &rcac,
        &ca_secret_key_scalar,
        fabric_id,
    )?;

    with_case_op_persisted(
        eve_addr,
        fabric_id,
        peer_node_id,
        rcac,
        controller_noc,
        controller_secret_scalar,
        ipk,
        vendor_id,
        op,
    )
    .await
}

/// Helper for the backward-compat path: mints a controller NOC inside a
/// scope so the (non-Send) rs-matter crypto types are dropped before any
/// `.await`. Returns owned NOC bytes + scalar.
fn mint_fresh_controller_noc(
    rcac: &[u8],
    ca_secret_key_scalar: &[u8; 32],
    fabric_id: u64,
) -> Result<(Vec<u8>, [u8; 32]), MatterFabricError> {
    use rs_matter::commissioner::NocGenerator;
    use rs_matter::crypto::Crypto;
    use rs_matter::crypto::{default_crypto, CanonPkcSecretKey, SecretKey, SigningSecretKey};
    use rs_matter::dm::devices::test::DAC_PRIVKEY;
    use rand_core::OsRng;

    let crypto = default_crypto(OsRng, DAC_PRIVKEY);
    let controller_secret_key = crypto
        .generate_secret_key()
        .map_err(|e| MatterFabricError::Matter(format!("generate controller key: {e:?}")))?;
    let mut controller_csr_buf = [0u8; 256];
    let controller_csr = controller_secret_key
        .csr(&mut controller_csr_buf)
        .map_err(|e| MatterFabricError::Matter(format!("controller csr: {e:?}")))?;
    let mut controller_secret_canon = CanonPkcSecretKey::new();
    controller_secret_key
        .write_canon(&mut controller_secret_canon)
        .map_err(|e| MatterFabricError::Matter(format!("write canon: {e:?}")))?;
    let mut controller_secret_scalar = [0u8; 32];
    controller_secret_scalar.copy_from_slice(controller_secret_canon.access());

    let ca_secret = CanonPkcSecretKey::from(ca_secret_key_scalar);
    let mut gen = NocGenerator::from_root_ca(&crypto, ca_secret, rcac, fabric_id, 1)
        .map_err(|e| MatterFabricError::Matter(format!("NocGenerator::from_root_ca: {e:?}")))?;
    let controller_creds = gen
        .generate_noc(&crypto, controller_csr, 1, &[])
        .map_err(|e| MatterFabricError::Matter(format!("generate controller noc: {e:?}")))?;

    Ok((controller_creds.noc.to_vec(), controller_secret_scalar))
}

/// Specific concrete op: CommissioningComplete on a CASE-secured exchange.
/// Used by the BLE commissioning flow as step 8 (after ConnectNetwork).
pub async fn case_and_commissioning_complete_via_udp(
    eve_addr: SocketAddr,
    fabric_id: u64,
    peer_node_id: u64,
    rcac: Vec<u8>,
    ca_secret_key_scalar: [u8; 32],
    ipk: [u8; 16],
    vendor_id: u16,
) -> Result<(), MatterFabricError> {
    // Auto-rescue: when the device's CASE listener flakes (Sigma1 acked,
    // no Sigma2 — observed ~1-in-5 against Meross MSS315 over WiFi), the
    // 30s timeout inside with_case_op fires and we'd previously bubble
    // up to matter-ble-commission, leaving the device half-commissioned
    // (failsafe armed, AddNOC pending rollback). Retrying with fresh
    // OsRng nonces almost always succeeds — and the user's only other
    // option was to know about `matter-op complete` and run it manually
    // inside the failsafe window. Retrying here turns that into a
    // built-in: the commissioner finalizes itself.
    let max_attempts: u32 = 3;
    for attempt in 1..=max_attempts {
        log::info!(
            "[case-udp] CommissioningComplete attempt {attempt}/{max_attempts}"
        );
        // rcac is Vec<u8> so it owns its bytes; clone per attempt so
        // with_case_op (which moves the Vec) can run again on retry.
        let rcac_for_attempt = rcac.clone();
        let result = with_case_op(
            eve_addr,
            fabric_id,
            peer_node_id,
            rcac_for_attempt,
            ca_secret_key_scalar,
            ipk,
            vendor_id,
            |ex_case| {
                Box::pin(async move {
                    use rs_matter::im::client::ImClient;
                    use rs_matter::im::CmdResp;
                    use rs_matter::tlv::TLVElement;

                    let cc_payload = syntaur_matter::tlv_build::commissioning_complete();
                    let tlv = TLVElement::new(&cc_payload);
                    let resp = ImClient::invoke_single_cmd(ex_case, 0, 0x0030, 0x04, tlv, None)
                        .await
                        .map_err(|e| {
                            MatterFabricError::Matter(format!(
                                "CommissioningComplete on CASE/UDP: {e:?}"
                            ))
                        })?;
                    match resp {
                        CmdResp::Cmd(_) => {
                            log::info!("[case-udp] CommissioningComplete InvokeResponse — DEVICE COMMISSIONED");
                            Ok(())
                        }
                        CmdResp::Status(s) => {
                            if s.status.status == rs_matter::im::IMStatusCode::Success {
                                log::info!("[case-udp] CommissioningComplete Success — DEVICE COMMISSIONED");
                                Ok(())
                            } else {
                                Err(MatterFabricError::Matter(format!(
                                    "CommissioningComplete on CASE/UDP returned status: {:?}",
                                    s.status
                                )))
                            }
                        }
                    }
                })
            },
        )
        .await;

        match result {
            Ok(()) => return Ok(()),
            Err(MatterFabricError::Matter(ref msg))
                if msg.contains("timed out") && attempt < max_attempts =>
            {
                log::warn!(
                    "[case-udp] CASE timed out on attempt {attempt}; retrying with fresh OsRng nonces in 2s"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e),
        }
    }
    // Loop only exits via Ok or non-timeout Err above; if we reach here
    // every attempt timed out.
    Err(MatterFabricError::Matter(format!(
        "CommissioningComplete on CASE/UDP timed out after {max_attempts} attempts"
    )))
}
