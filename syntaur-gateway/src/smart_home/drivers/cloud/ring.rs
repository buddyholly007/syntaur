//! Ring doorbells / cameras — **stub pending OAuth wiring**.
//!
//! Ring's public API is the `ring-client-api`-style Node library's
//! server, not a documented REST surface. Community reverse-
//! engineered it as `oauth.ring.com` with device-flow refresh-token
//! auth. To ship real integration we need:
//!
//!   1. A /settings/ring page that runs the OAuth device flow
//!      (POST username/password → 2FA → refresh token).
//!   2. Encrypted refresh-token storage in `smart_home_credentials`
//!      (already supported via `crypto.rs`).
//!   3. An access-token refresh loop (Ring access tokens expire ~1h).
//!   4. A minimal REST client around the discovered endpoints
//!      (`/clients_api/ring_devices`, `/clients_api/doorbots/{id}/history`).
//!
//! None of those ship in v1. File stays as scaffolding so the cloud
//! adapter module tree compiles + the eventual implementation has a
//! known home.

#![allow(dead_code)]

pub const ADAPTER_NAME: &str = "ring";
