//! Opt-in cloud (and cloud-adjacent local-API) adapters.
//!
//! Unlike the always-on drivers in `drivers/*`, these require explicit
//! per-user configuration (OAuth grants, gateway IPs, API tokens) and
//! are off by default. Each adapter owns its own config surface and
//! exposes a thin async API so energy ingestion + automation can
//! invoke it without caring about the vendor specifics.
//!
//! v1 ships the Tesla Powerwall **local** adapter as the one concrete
//! implementation — it's a pure HTTP client against the gateway on the
//! user's LAN, no cloud round-trip and no OAuth. Ring / Nest / Ecobee
//! ship as honest stubs that document exactly what's missing (all
//! three require OAuth2 device-flow grants + per-tenant secrets),
//! landing as real integrations once their config UI pages exist.

pub mod ecobee;
pub mod nest;
pub mod ring;
pub mod tesla_local;
