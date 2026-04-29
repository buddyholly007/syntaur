//! Per-device privacy policy.
//!
//! The Privacy tier (this crate, fully built) supports `Open` and
//! `Privacy` modes. `Lockdown` exists in the enum so the rest of
//! Syntaur can model the eventual state, but trying to *apply*
//! Lockdown today returns `Error::DnsServer("lockdown not yet
//! implemented — gated on dedicated-hardware deploy")`. The UI must
//! gate the Lockdown choice on a "gateway hardware connected" check.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Default: no DNS interception, no firewall enforcement.
    Open,
    /// DNS sinkhole blocks vendor-cloud lookups for this device.
    /// Falls back to upstream resolver for everything else.
    Privacy,
    /// Full L3 enforcement (nft policy + DHCP + per-device allowlist).
    /// NOT YET IMPLEMENTED — see `projects/syntaur_privacy_mode` Phase 5.
    Lockdown,
}

impl Mode {
    /// True iff the mode is implemented in the Privacy tier (this crate).
    pub fn is_supported(&self) -> bool {
        matches!(self, Mode::Open | Mode::Privacy)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Mode::Open => "Open",
            Mode::Privacy => "Privacy",
            Mode::Lockdown => "Lockdown",
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Open
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePolicy {
    /// Stable device identifier from Syntaur's smart_home_devices DB row.
    pub device_id: String,
    pub mode: Mode,
    /// Optional per-device override list of additional domains to sinkhole
    /// (e.g. for a specific firmware that phones home to a different host).
    #[serde(default)]
    pub extra_blocked_domains: Vec<String>,
    /// Optional temporary allow window — when set, `mode` is treated as
    /// `Open` until this timestamp passes. UI surfaces a countdown.
    #[serde(default)]
    pub temporary_allow_until: Option<SystemTime>,
    /// Last time the user (or autopilot) changed `mode`. Used for
    /// audit + UI ordering.
    #[serde(default)]
    pub mode_set_at: Option<SystemTime>,
}

impl DevicePolicy {
    pub fn new(device_id: impl Into<String>, mode: Mode) -> Self {
        Self {
            device_id: device_id.into(),
            mode,
            extra_blocked_domains: Vec::new(),
            temporary_allow_until: None,
            mode_set_at: Some(SystemTime::now()),
        }
    }

    /// True if the device's policy currently *enforces* (i.e. the
    /// temporary-allow window has not expired and mode is not Open).
    pub fn is_enforcing(&self, now: SystemTime) -> bool {
        if matches!(self.mode, Mode::Open) {
            return false;
        }
        match self.temporary_allow_until {
            Some(until) if now < until => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn default_mode_is_open() {
        assert_eq!(Mode::default(), Mode::Open);
    }

    #[test]
    fn open_and_privacy_supported_lockdown_not() {
        assert!(Mode::Open.is_supported());
        assert!(Mode::Privacy.is_supported());
        assert!(!Mode::Lockdown.is_supported());
    }

    #[test]
    fn open_mode_never_enforces() {
        let p = DevicePolicy::new("dev-1", Mode::Open);
        assert!(!p.is_enforcing(SystemTime::now()));
    }

    #[test]
    fn privacy_mode_enforces_when_no_temp_allow() {
        let p = DevicePolicy::new("dev-1", Mode::Privacy);
        assert!(p.is_enforcing(SystemTime::now()));
    }

    #[test]
    fn temporary_allow_window_suspends_enforcement() {
        let mut p = DevicePolicy::new("dev-1", Mode::Privacy);
        let now = SystemTime::now();
        p.temporary_allow_until = Some(now + Duration::from_secs(60));
        assert!(!p.is_enforcing(now));
    }

    #[test]
    fn temporary_allow_expires() {
        let mut p = DevicePolicy::new("dev-1", Mode::Privacy);
        let now = SystemTime::now();
        p.temporary_allow_until = Some(now - Duration::from_secs(1));
        assert!(p.is_enforcing(now));
    }

    #[test]
    fn json_round_trip() {
        let p = DevicePolicy::new("dev-1", Mode::Privacy);
        let j = serde_json::to_string(&p).unwrap();
        let p2: DevicePolicy = serde_json::from_str(&j).unwrap();
        assert_eq!(p2.device_id, p.device_id);
        assert_eq!(p2.mode, p.mode);
    }
}
