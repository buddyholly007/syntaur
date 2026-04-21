//! Google / Nest devices — **stub pending Device Access OAuth wiring**.
//!
//! Google's Smart Device Management (SDM) API is what Nest devices
//! expose now. It requires:
//!
//!   1. A $5 one-time Device Access registration fee + a GCP
//!      project the user creates in their own account.
//!   2. OAuth 2.0 with PKCE (device flow is not supported).
//!   3. A per-structure authorization flow — users grant access to
//!      a specific "structure" (house) rather than individual devices.
//!   4. SDM REST endpoints: `enterprises/{id}/devices`,
//!      `enterprises/{id}/structures`.
//!
//! Shipping real Nest integration means owning enough config UI to
//! walk users through the Device Access console + project setup.
//! v1 scope is too tight for that; the stub lives here so adapter
//! tree compiles and this file is the concrete home for the Week
//! 11 v1.x implementation.

#![allow(dead_code)]

pub const ADAPTER_NAME: &str = "nest";
