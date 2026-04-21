//! Ecobee thermostats — **stub pending OAuth device-flow wiring**.
//!
//! Ecobee exposes a real public REST API (`api.ecobee.com/1/thermostat`)
//! with OAuth 2.0 device-flow auth. The integration shape is well
//! documented and much simpler than Ring or Nest — this is the
//! cloud adapter most likely to land first post-v1.
//!
//! Missing pieces:
//!   1. App-key registration on developer.ecobee.com (our app, not
//!      per-user — one API key baked into the Syntaur install).
//!   2. /settings/ecobee page that runs the device-flow pairing
//!      (displays a code + URL the user enters on ecobee.com, then
//!      polls the token endpoint until they click authorize).
//!   3. Refresh-token storage in `smart_home_credentials` (encrypted).
//!   4. REST client for `/1/thermostat?selection=...` + runtime state.
//!
//! Ecobee-specific note: the Summary interface returns all thermostats
//! on the user's account in one call, so scan discovery and state
//! refresh can be batched efficiently once auth lands.

#![allow(dead_code)]

pub const ADAPTER_NAME: &str = "ecobee";
