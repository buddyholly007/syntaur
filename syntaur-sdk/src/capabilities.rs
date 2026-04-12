//! Per-tool capability metadata for rate limiting, circuit breaking,
//! and approval gating.

/// Operational metadata for a tool. Not surfaced to the LLM — purely
/// used by the gateway's dispatch funnel.
#[derive(Debug, Clone)]
pub struct ToolCapabilities {
    /// Tool reads but never writes. Default true.
    pub read_only: bool,
    /// Tool's writes are destructive (overwrite/delete). Default false.
    pub destructive: bool,
    /// Repeated calls with the same args produce the same result.
    pub idempotent: bool,
    /// Tool makes outbound network requests.
    pub network: bool,
    /// Override the approval gate. `Some(true)` forces approval,
    /// `Some(false)` skips it, `None` uses the registry default.
    pub requires_approval: Option<bool>,
    /// Circuit breaker group key. Tools sharing state should share
    /// a breaker name.
    pub circuit_name: Option<&'static str>,
    /// Explicit rate limit as `(capacity, per_seconds)`.
    pub rate_limit: Option<(u32, u64)>,
}

impl Default for ToolCapabilities {
    fn default() -> Self {
        Self {
            read_only: true,
            destructive: false,
            idempotent: true,
            network: false,
            requires_approval: None,
            circuit_name: None,
            rate_limit: None,
        }
    }
}

impl ToolCapabilities {
    /// Read-only network tools (web_search, fetch, etc).
    pub fn read_network() -> Self {
        Self {
            read_only: true,
            network: true,
            ..Self::default()
        }
    }

    /// Tools that mutate the local filesystem.
    pub fn write_local() -> Self {
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
            ..Self::default()
        }
    }

    /// Tools that mutate external state (post, send email, create account).
    pub fn write_external(circuit: &'static str) -> Self {
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
            network: true,
            circuit_name: Some(circuit),
            ..Self::default()
        }
    }
}
