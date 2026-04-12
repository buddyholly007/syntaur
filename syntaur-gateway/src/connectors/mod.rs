//! Connector framework for ingesting external sources into the document index.
//!
//! Onyx-style contracts:
//!   - LoadConnector::load_full() — bulk snapshot
//!   - PollConnector::poll_range() — incremental updates by time range
//!   - SlimConnector::list_ids() — cheap ID scan for prune detection
//!
//! v1 ships with one connector (`workspace_files`) and a simple background
//! scheduler that runs Load + Poll on a fixed interval. Per-connector
//! refresh/prune schedules and an event-driven mode are deferred.

mod scheduler;
pub mod sources;

pub use scheduler::ConnectorScheduler;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::index::ExternalDoc;

/// Identifier emitted by `SlimConnector::list_ids` for prune detection.
#[derive(Debug, Clone)]
pub struct DocIdOnly {
    pub external_id: String,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Common base for all connector kinds. Defines `name()` exactly once so
/// trait-object methods don't collide when a connector implements multiple
/// kinds.
pub trait Connector: Send + Sync {
    fn name(&self) -> &str;
}

/// Bulk-snapshot connector. Called once at startup and on manual refresh.
#[async_trait]
pub trait LoadConnector: Connector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String>;
}

/// Incremental connector. Called periodically with a time window.
#[async_trait]
#[allow(dead_code)]
pub trait PollConnector: Connector {
    async fn poll_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<ExternalDoc>, String>;
}

/// Existence-scan connector. Returns just IDs so we can detect deletions.
#[async_trait]
#[allow(dead_code)]
pub trait SlimConnector: Connector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String>;
}

/// A connector that supports all three operations. Most file-based connectors
/// will implement this since the operations are cheap.
#[async_trait]
pub trait FullConnector: LoadConnector + SlimConnector + Send + Sync {}

impl<T: LoadConnector + SlimConnector + Send + Sync> FullConnector for T {}

/// Wraps a connector along with its run schedule.
pub struct ConnectorEntry {
    pub connector: Arc<dyn FullConnector>,
    pub refresh_secs: u64,
    pub prune_secs: u64,
}
