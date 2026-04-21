//! Zigbee 3.0 driver — **deferred to v1.x**.
//!
//! 2026-04-21 ecosystem survey found no production-ready Rust Zigbee
//! coordinator stack. Every candidate crate is either archived
//! (`zigbee-rs`, garro95/zigbee → thebino) or explicit WIP (`ezsp`
//! frame codec only, `deconz-sp` "do not use in production", `ziggurat`
//! ~19 commits one-hop). Pure-Rust EZSP from scratch = 6-10 weeks of
//! ASHv2 + NWK formation + trust center + ZDO + ZCL work.
//!
//! Sean's call 2026-04-21: pure Rust only, no Zigbee2MQTT bridge. So
//! v1 ships without native Zigbee entirely; the `crates/syntaur-zigbee`
//! crate lands as a named v1.x milestone alongside the Matter Controller
//! (see task #17 + plan file).
//!
//! This file stays as a placeholder so the drivers module tree compiles;
//! `scan::run()` does not call into it, and no scan candidate will ever
//! carry `driver = "zigbee"` in v1.
