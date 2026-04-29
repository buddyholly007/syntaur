//! Vendor-cloud-domain registry.
//!
//! Loads a YAML file describing each vendor's cloud-control domains and
//! exposes a glob-based matcher. The DNS sinkhole consults the matcher
//! once per query; matches return NXDOMAIN.
//!
//! The default registry is shipped at `data/cloud_domains.yaml` next to
//! this crate; the host can ship its own copy at
//! `~/.syntaur/privacy/cloud_domains.yaml`. If both exist, the host copy
//! is preferred.

use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorEntry {
    pub vendor_id: String,
    pub vendor_name: String,
    pub cloud_domains: Vec<String>,
    #[serde(default)]
    pub hardcodes_ips: bool,
    #[serde(default)]
    pub notes: String,
}

/// Embedded fallback registry, baked into the binary at compile time.
/// Used when no on-disk copy is available.
const EMBEDDED: &str = include_str!("../data/cloud_domains.yaml");

#[derive(Debug, Clone)]
pub struct CloudDomainRegistry {
    entries: Vec<VendorEntry>,
    matcher: GlobSet,
    /// Parallel array: matcher.matches(name)[i] → entries[matcher_owner[i]].
    matcher_owner: Vec<usize>,
}

impl CloudDomainRegistry {
    /// Load from a YAML file. Returns the parsed registry with a
    /// pre-compiled glob matcher.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::load_from_str(&text)
    }

    /// Load from a YAML string.
    pub fn load_from_str(text: &str) -> Result<Self> {
        let entries: Vec<VendorEntry> = serde_yaml::from_str(text)?;
        Self::from_entries(entries)
    }

    /// Load the registry baked into this crate at compile time.
    pub fn embedded() -> Result<Self> {
        Self::load_from_str(EMBEDDED)
    }

    fn from_entries(entries: Vec<VendorEntry>) -> Result<Self> {
        let mut builder = GlobSetBuilder::new();
        let mut owner = Vec::new();
        for (idx, entry) in entries.iter().enumerate() {
            for pattern in &entry.cloud_domains {
                // Case-insensitive: DNS labels are case-insensitive per RFC 1035,
                // and we don't want the registry to silently miss a host that
                // happens to advertise itself in mixed case.
                let glob = GlobBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|source| Error::InvalidGlob {
                        pattern: pattern.clone(),
                        source,
                    })?;
                builder.add(glob);
                owner.push(idx);
            }
        }
        let matcher = builder
            .build()
            .map_err(|e| Error::RegistryParse(format!("globset build: {e}")))?;
        Ok(Self {
            entries,
            matcher,
            matcher_owner: owner,
        })
    }

    /// True if `name` matches any vendor's cloud-domain pattern.
    /// `name` may include a trailing dot (FQDN form from the DNS wire);
    /// it is stripped before matching.
    /// Hot path: optimized boolean check, no allocation.
    pub fn is_blocked(&self, name: &str) -> bool {
        let name = name.trim_end_matches('.');
        self.matcher.is_match(name)
    }

    /// Return all vendor entries whose patterns match `name`.
    /// Trailing dot accepted (FQDN form) and stripped before matching.
    /// Allocates; not for the DNS hot path — call `is_blocked` for
    /// boolean checks.
    pub fn matches_for(&self, name: &str) -> Vec<&VendorEntry> {
        let name = name.trim_end_matches('.');
        let hits = self.matcher.matches(name);
        let mut owners: Vec<usize> = hits.into_iter().map(|i| self.matcher_owner[i]).collect();
        owners.sort_unstable();
        owners.dedup();
        owners.into_iter().map(|i| &self.entries[i]).collect()
    }

    pub fn entries(&self) -> &[VendorEntry] {
        &self.entries
    }

    pub fn vendor_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> CloudDomainRegistry {
        CloudDomainRegistry::embedded().expect("embedded registry must parse")
    }

    #[test]
    fn embedded_registry_parses() {
        let r = fresh();
        assert!(r.vendor_count() >= 8, "expected at least 8 vendors, got {}", r.vendor_count());
    }

    #[test]
    fn meross_root_domain_matches() {
        let r = fresh();
        assert!(r.is_blocked("iot.meross.com"));
        assert!(r.is_blocked("openapi.meross.com"));
    }

    #[test]
    fn meross_iotx_wildcard_matches() {
        let r = fresh();
        // iotx-*.meross.com — the '-' AFTER the wildcard means single-label.
        assert!(r.is_blocked("iotx-eu.meross.com"));
    }

    #[test]
    fn unrelated_domains_pass_through() {
        let r = fresh();
        assert!(!r.is_blocked("syntaur.io"));
        assert!(!r.is_blocked("github.com"));
        assert!(!r.is_blocked("home.arpa"));
    }

    #[test]
    fn match_returns_vendor_entry() {
        let r = fresh();
        let hits = r.matches_for("iot.tplinkcloud.com");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].vendor_id, "tplink");
    }

    #[test]
    fn hardcodes_ips_flag_round_trips() {
        // Use controlled input so the test isn't coupled to the embedded
        // YAML's specific vendor list.
        let yaml = r#"
- vendor_id: hardcoder
  vendor_name: Hardcoded Cloud
  cloud_domains: ["api.hardcoder.com"]
  hardcodes_ips: true
- vendor_id: nicevendor
  vendor_name: Nice Vendor
  cloud_domains: ["api.nicevendor.com"]
  hardcodes_ips: false
"#;
        let r = CloudDomainRegistry::load_from_str(yaml).unwrap();
        let h = r.entries().iter().find(|e| e.vendor_id == "hardcoder").unwrap();
        let n = r.entries().iter().find(|e| e.vendor_id == "nicevendor").unwrap();
        assert!(h.hardcodes_ips);
        assert!(!n.hardcodes_ips);
    }

    #[test]
    fn trailing_dot_fqdn_form_matches() {
        let r = fresh();
        // hickory passes names with trailing dots (FQDN form). We strip.
        assert!(r.is_blocked("iot.meross.com."));
        assert_eq!(r.matches_for("iot.meross.com.").len(), 1);
    }

    #[test]
    fn case_insensitive_match() {
        // DNS is case-insensitive per RFC 1035; the registry must follow.
        let yaml = r#"
- vendor_id: x
  vendor_name: X
  cloud_domains: ["*.example.com"]
"#;
        let r = CloudDomainRegistry::load_from_str(yaml).unwrap();
        assert!(r.is_blocked("api.example.com"));
        assert!(r.is_blocked("API.EXAMPLE.COM"));
        assert!(r.is_blocked("Api.Example.Com"));
    }

    #[test]
    fn invalid_glob_yields_clear_error() {
        // Crafted YAML with an obviously-broken pattern; assert we surface
        // the offending pattern in the error chain.
        let bad = "- vendor_id: x\n  vendor_name: x\n  cloud_domains:\n    - \"[unclosed\"\n";
        let err = CloudDomainRegistry::load_from_str(bad).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("[unclosed"), "error did not mention pattern: {s}");
    }
}
