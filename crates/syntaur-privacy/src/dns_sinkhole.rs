//! DNS sinkhole — the runtime hot path.
//!
//! Intercepts DNS queries on a configurable UDP+TCP port. For each
//! query:
//!
//! 1. Look up the name in the cloud-domain registry.
//! 2. If matched, return NXDOMAIN immediately and emit a `DnsBlocked`
//!    event on the bus.
//! 3. If not matched, forward to the configured upstream resolver
//!    and return whatever it returns (passthrough).
//!
//! Decision logic is split out as a pure function (`decide`) so unit
//! tests don't need to bind a port. The `serve` entry point is the
//! actual server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hickory_proto::op::{Header, HeaderCounts, Metadata, ResponseCode};
use hickory_proto::rr::{Name, Record, RecordType};
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::lookup::Lookup;
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use hickory_server::Server;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use tokio::net::{TcpListener, UdpSocket};

use crate::error::{Error, Result};
use crate::monitor::{Event, EventBus};
use crate::registry::CloudDomainRegistry;

/// Decision the matcher emits per query — pure, allocation-free,
/// trivially testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Return NXDOMAIN. Vendor id, if any, is captured for event logging.
    Block { vendor_id: Option<String> },
    /// Forward to upstream resolver.
    Forward,
}

/// Pure decision function. Public so it's testable without binding a port.
pub fn decide(registry: &CloudDomainRegistry, name: &str) -> Decision {
    let hits = registry.matches_for(name);
    match hits.first() {
        Some(entry) => Decision::Block {
            vendor_id: Some(entry.vendor_id.clone()),
        },
        None => Decision::Forward,
    }
}

/// Resolves a client IP to a Syntaur device id, when the IP is recognized.
/// Implemented by the gateway's smart_home_devices module; here just a
/// trait so this crate doesn't depend on the gateway.
#[async_trait::async_trait]
pub trait DeviceLookup: Send + Sync + 'static {
    async fn device_for_ip(&self, ip: std::net::IpAddr) -> Option<String>;
}

/// No-op device lookup — useful before the gateway wires its real
/// resolver in, and for tests.
pub struct NullDeviceLookup;

#[async_trait::async_trait]
impl DeviceLookup for NullDeviceLookup {
    async fn device_for_ip(&self, _ip: std::net::IpAddr) -> Option<String> {
        None
    }
}

/// Runtime config for the sinkhole.
#[derive(Debug, Clone)]
pub struct SinkholeConfig {
    /// UDP+TCP bind address. Use `127.0.0.1:5353` or similar for
    /// non-privileged testing; `0.0.0.0:53` once stable.
    pub bind: SocketAddr,
    /// Optional upstream resolver. None = use system default.
    pub upstream: Option<ResolverConfig>,
    pub upstream_opts: ResolverOpts,
    /// TTL applied to NXDOMAIN responses (caching client retries).
    /// Default 300s.
    pub block_ttl_secs: u32,
}

impl Default for SinkholeConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:5353".parse().unwrap(),
            upstream: None,
            upstream_opts: ResolverOpts::default(),
            block_ttl_secs: 300,
        }
    }
}

/// The hickory-server request handler that glues registry + upstream.
pub struct PrivacyDnsHandler {
    registry: Arc<CloudDomainRegistry>,
    upstream: Arc<TokioResolver>,
    bus: EventBus,
    device_lookup: Arc<dyn DeviceLookup>,
}

impl PrivacyDnsHandler {
    pub fn new(
        registry: Arc<CloudDomainRegistry>,
        upstream: Arc<TokioResolver>,
        bus: EventBus,
        device_lookup: Arc<dyn DeviceLookup>,
    ) -> Self {
        Self {
            registry,
            upstream,
            bus,
            device_lookup,
        }
    }
}

// hickory 0.26 made the RequestHandler trait generic over a `Time` type
// in addition to `ResponseHandler`. The Time generic is unused in this
// handler's body but required by the trait signature.
#[async_trait::async_trait]
impl RequestHandler for PrivacyDnsHandler {
    async fn handle_request<R: ResponseHandler, T: hickory_server::net::runtime::Time>(
        &self,
        request: &Request,
        response_handle: R,
    ) -> ResponseInfo {
        // RequestInfo gives us the metadata + the single LowerQuery in one
        // typed access. If the request has 0 or 2+ queries we fall back to
        // ServFail rather than panic — same shape as ResponseInfo::serve_failed.
        let info = match request.request_info() {
            Ok(i) => i,
            Err(_) => {
                let mut metadata = Metadata::response_from_request(&request.metadata);
                metadata.response_code = ResponseCode::ServFail;
                return send_empty(response_handle, request, metadata).await;
            }
        };

        let mut metadata = Metadata::response_from_request(info.metadata);
        metadata.authoritative = false;
        metadata.recursion_available = true;

        let qname = info.query.name().to_string();
        let qname_lc = qname.to_ascii_lowercase();
        let qtype = info.query.query_type();

        match decide(&self.registry, &qname_lc) {
            Decision::Block { vendor_id } => {
                let device_id = self
                    .device_lookup
                    .device_for_ip(request.src().ip())
                    .await;
                self.bus.emit(Event::DnsBlocked {
                    ts: SystemTime::now(),
                    client_ip: request.src().ip().to_string(),
                    device_id,
                    query_name: qname_lc.trim_end_matches('.').to_string(),
                    vendor_id,
                });
                metadata.response_code = ResponseCode::NXDomain;
                send_empty(response_handle, request, metadata).await
            }
            Decision::Forward => match forward(&self.upstream, &qname, qtype).await {
                Ok(records) => send_records(response_handle, request, metadata, records).await,
                Err(e) => {
                    log::debug!("upstream resolve failed for {qname_lc}: {e}");
                    metadata.response_code = ResponseCode::ServFail;
                    send_empty(response_handle, request, metadata).await
                }
            },
        }
    }
}

async fn forward(
    upstream: &TokioResolver,
    qname: &str,
    qtype: RecordType,
) -> std::result::Result<Vec<Record>, NetError> {
    let name = Name::from_ascii(qname)?;
    let lookup: Lookup = upstream.lookup(name, qtype).await?;
    Ok(lookup.answers().to_vec())
}

async fn send_empty<R: ResponseHandler>(
    mut response_handle: R,
    request: &Request,
    metadata: Metadata,
) -> ResponseInfo {
    let builder = MessageResponseBuilder::from_message_request(request);
    let resp = builder.build_no_records(metadata);
    response_handle
        .send_response(resp)
        .await
        .unwrap_or_else(|_| ResponseInfo::from(Header { metadata, counts: HeaderCounts::default() }))
}

async fn send_records<R: ResponseHandler>(
    mut response_handle: R,
    request: &Request,
    metadata: Metadata,
    records: Vec<Record>,
) -> ResponseInfo {
    let builder = MessageResponseBuilder::from_message_request(request);
    let resp = builder.build(
        metadata,
        records.iter(),
        std::iter::empty(),
        std::iter::empty(),
        std::iter::empty(),
    );
    response_handle
        .send_response(resp)
        .await
        .unwrap_or_else(|_| ResponseInfo::from(Header { metadata, counts: HeaderCounts::default() }))
}

/// Bind UDP+TCP listeners on `cfg.bind` and serve until the future
/// is dropped. Cancel by dropping the returned `Server`.
pub async fn serve(
    cfg: SinkholeConfig,
    registry: Arc<CloudDomainRegistry>,
    bus: EventBus,
    device_lookup: Arc<dyn DeviceLookup>,
) -> Result<Server<PrivacyDnsHandler>> {
    let upstream = build_resolver(&cfg)?;
    let handler = PrivacyDnsHandler::new(registry, Arc::new(upstream), bus, device_lookup);
    let mut server = Server::new(handler);

    let udp = UdpSocket::bind(cfg.bind)
        .await
        .map_err(|e| Error::DnsServer(format!("udp bind {}: {e}", cfg.bind)))?;
    server.register_socket(udp);

    let tcp = TcpListener::bind(cfg.bind)
        .await
        .map_err(|e| Error::DnsServer(format!("tcp bind {}: {e}", cfg.bind)))?;
    // hickory 0.26 added a response_buffer_size param. 65535 = max UDP
    // payload — generous for a forwarder, no real downside on TCP.
    server.register_listener(tcp, Duration::from_secs(5), 65535);

    log::info!("syntaur-privacy DNS sinkhole listening on {}", cfg.bind);
    Ok(server)
}

fn build_resolver(cfg: &SinkholeConfig) -> Result<TokioResolver> {
    let resolver = match &cfg.upstream {
        Some(rc) => {
            let mut builder = TokioResolver::builder_with_config(rc.clone(), TokioRuntimeProvider::default());
            *builder.options_mut() = cfg.upstream_opts.clone();
            builder
                .build()
                .map_err(|e| Error::DnsServer(format!("resolver build (explicit): {e}")))?
        }
        None => {
            // Honor the host's /etc/resolv.conf rather than blindly using
            // Google DNS. `builder()` reads system_conf internally.
            match TokioResolver::builder(TokioRuntimeProvider::default()) {
                Ok(b) => b
                    .build()
                    .map_err(|e| Error::DnsServer(format!("resolver build (system): {e}")))?,
                Err(e) => {
                    log::warn!("read_system_conf failed ({e}); falling back to default config");
                    let mut builder = TokioResolver::builder_with_config(
                        ResolverConfig::default(),
                        TokioRuntimeProvider::default(),
                    );
                    *builder.options_mut() = cfg.upstream_opts.clone();
                    builder
                        .build()
                        .map_err(|e| Error::DnsServer(format!("resolver build (fallback): {e}")))?
                }
            }
        }
    };
    Ok(resolver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::CloudDomainRegistry;

    fn registry() -> CloudDomainRegistry {
        CloudDomainRegistry::embedded().expect("embedded registry")
    }

    #[test]
    fn decide_blocks_known_vendor() {
        let r = registry();
        let d = decide(&r, "iot.meross.com");
        match d {
            Decision::Block { vendor_id } => assert_eq!(vendor_id.as_deref(), Some("meross")),
            _ => panic!("expected Block, got {:?}", d),
        }
    }

    #[test]
    fn decide_forwards_unknown() {
        let r = registry();
        let d = decide(&r, "github.com");
        assert_eq!(d, Decision::Forward);
    }

    #[test]
    fn decide_handles_fqdn_trailing_dot() {
        let r = registry();
        let d = decide(&r, "iot.meross.com.");
        assert!(matches!(d, Decision::Block { .. }));
    }

    #[test]
    fn decide_case_insensitive() {
        let r = registry();
        let d = decide(&r, "IoT.Meross.COM");
        assert!(matches!(d, Decision::Block { .. }));
    }

    #[tokio::test]
    async fn null_device_lookup_returns_none() {
        let l = NullDeviceLookup;
        let r = l.device_for_ip("192.168.1.1".parse().unwrap()).await;
        assert!(r.is_none());
    }
}
