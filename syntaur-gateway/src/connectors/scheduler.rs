//! Background scheduler that runs each connector on its own interval.
//!
//! v1 strategy: spawn one tokio task per connector. Each task does
//!   loop { load_full → indexer.put_document for each → sleep refresh_secs }
//! and a separate prune timer that runs slim → indexer.prune.
//!
//! There is no work queue, no priority, no cross-connector concurrency
//! limit. Single-user, ~10K-doc scale doesn't need any of that. If a
//! connector takes longer than its interval to run, the next tick is
//! skipped (not queued).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use log::{error, info, warn};
use tokio::sync::watch;

use super::ConnectorEntry;
use crate::index::Indexer;

pub struct ConnectorScheduler {
    indexer: Arc<Indexer>,
    entries: Vec<ConnectorEntry>,
}

impl ConnectorScheduler {
    pub fn new(indexer: Arc<Indexer>) -> Self {
        Self {
            indexer,
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, entry: ConnectorEntry) {
        info!(
            "[connector] registered '{}' (refresh {}s, prune {}s)",
            entry.connector.name(),
            entry.refresh_secs,
            entry.prune_secs
        );
        self.entries.push(entry);
    }

    /// Run an initial full load of every connector synchronously, then
    /// spawn the periodic refresh tasks. Initial load is awaited so the
    /// index is warm by the time HTTP/Telegram start serving.
    pub async fn warm_up_then_spawn(self, mut shutdown_rx: watch::Receiver<bool>) {
        let indexer = self.indexer.clone();
        let entries = self.entries;

        // Initial sync — sequential so logs are readable
        for entry in &entries {
            let name = entry.connector.name().to_string();
            info!("[connector:{}] initial load starting", name);
            match entry.connector.load_full().await {
                Ok(docs) => {
                    let n = docs.len();
                    let mut errors = 0;
                    for doc in docs {
                        if let Err(e) = indexer.put_document(doc).await {
                            errors += 1;
                            warn!("[connector:{}] put failed: {}", name, e);
                        }
                    }
                    let stats = indexer.stats(None).await;
                    info!(
                        "[connector:{}] initial load done: {} docs, {} errors. Index: {} docs / {} chunks total",
                        name, n, errors, stats.documents, stats.chunks
                    );

                    // Initial prune (slim scan)
                    match entry.connector.list_ids().await {
                        Ok(ids) => {
                            let keep: Vec<String> =
                                ids.into_iter().map(|d| d.external_id).collect();
                            match indexer.prune(&name, keep).await {
                                Ok(removed) => {
                                    if removed > 0 {
                                        info!(
                                            "[connector:{}] pruned {} stale docs",
                                            name, removed
                                        );
                                    }
                                }
                                Err(e) => warn!("[connector:{}] prune failed: {}", name, e),
                            }
                        }
                        Err(e) => warn!("[connector:{}] list_ids failed: {}", name, e),
                    }

                    let _ = indexer
                        .set_connector_cursor(
                            &name,
                            &serde_json::json!({
                                "last_full_load": Utc::now().to_rfc3339(),
                            })
                            .to_string(),
                        )
                        .await;
                }
                Err(e) => {
                    error!("[connector:{}] initial load failed: {}", name, e);
                }
            }
        }

        // Spawn periodic refresh tasks
        for entry in entries {
            let indexer = indexer.clone();
            let mut rx = shutdown_rx.clone();
            let connector = entry.connector;
            let refresh_secs = entry.refresh_secs;
            let prune_secs = entry.prune_secs;
            tokio::spawn(async move {
                let name = connector.name().to_string();
                let mut prune_counter: u64 = 0;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(refresh_secs)) => {}
                        _ = rx.changed() => {
                            info!("[connector:{}] shutting down", name);
                            return;
                        }
                    }

                    // Refresh: full load (cheap for file-based connectors)
                    match connector.load_full().await {
                        Ok(docs) => {
                            let n = docs.len();
                            for doc in docs {
                                if let Err(e) = indexer.put_document(doc).await {
                                    warn!("[connector:{}] refresh put failed: {}", name, e);
                                }
                            }
                            info!("[connector:{}] refresh: {} docs", name, n);
                        }
                        Err(e) => warn!("[connector:{}] refresh failed: {}", name, e),
                    }

                    // Prune every Nth refresh cycle
                    prune_counter += refresh_secs;
                    if prune_counter >= prune_secs {
                        prune_counter = 0;
                        if let Ok(ids) = connector.list_ids().await {
                            let keep: Vec<String> =
                                ids.into_iter().map(|d| d.external_id).collect();
                            if let Ok(removed) = indexer.prune(&name, keep).await {
                                if removed > 0 {
                                    info!("[connector:{}] pruned {} stale", name, removed);
                                }
                            }
                        }
                    }

                    // Update cursor
                    let _ = indexer
                        .set_connector_cursor(
                            &name,
                            &serde_json::json!({"last_refresh": Utc::now().to_rfc3339()}).to_string(),
                        )
                        .await;
                }
            });
        }

        // Park until shutdown
        let _ = shutdown_rx.changed().await;
    }
}
