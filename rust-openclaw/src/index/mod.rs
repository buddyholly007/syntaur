//! Document indexing and full-text search.
//!
//! v1: SQLite + FTS5 only (BM25 keyword search). Vector embeddings via
//! sqlite-vec are deferred to v2 — FTS5 alone is sufficient for the first
//! useful slice and avoids the runtime extension-loading complexity.
//!
//! Schema lives in its own database (`~/.syntaur/index.db`) so the indexer
//! can run in WAL mode without affecting the LCM conversation store.
//!
//! Connector framework writes documents through the `Indexer` API; the
//! `internal_search` tool reads through the same API.

mod embed;
mod schema;
mod search;

pub use search::SearchHit;

/// Implemented by stores that want to be notified when an indexed document
/// changes (so they can invalidate any cached results that referenced it).
/// Defined here in `index` to avoid a circular dependency with `research`,
/// where the SessionStore implementation lives.
#[async_trait::async_trait]
pub trait StaleNotifier: Send + Sync {
    async fn mark_stale_for_doc(&self, source: &str, external_id: &str);
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use rusqlite::{params, Connection};
use serde_json::Value;
use tokio::sync::Mutex;

/// External document handed to the indexer by a connector.
/// Mirrors Onyx's `ExternalDoc` shape: enough to uniquely identify the source
/// row, full body for chunking, optional metadata for filtering.
#[derive(Debug, Clone)]
pub struct ExternalDoc {
    pub source: String,        // connector name (e.g. "workspace_files")
    pub external_id: String,   // stable id within the source (e.g. file path)
    pub title: String,         // human-readable title
    pub body: String,          // full text content
    pub updated_at: DateTime<Utc>,
    pub metadata: Value,       // arbitrary JSON metadata
}

/// Single chunk of an indexed document.
/// Chunks are produced from the body during ingestion using a simple
/// fixed-size token strategy. Each chunk lands in the FTS5 virtual table.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub doc_id: i64,
    pub ord: i64,
    pub text: String,
}

/// Public API to the document index.
/// Cloneable as `Arc<Indexer>` and shared with connector workers and the
/// `internal_search` tool.
pub struct Indexer {
    db: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    embedder: Option<Arc<crate::llm::LlmChain>>,
    stale_notifier: Option<Arc<dyn StaleNotifier>>,
}

impl Indexer {
    /// Open the index database, run migrations, set WAL mode.
    /// Falls back to in-memory DB on disk failure (logged) so startup never
    /// fails because of an indexer issue.
    pub fn open(db_path: PathBuf) -> Result<Arc<Self>, String> {
        // Make sure parent dir exists
        if let Some(parent) = db_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!(
                    "[index] failed to create parent {}: {}",
                    parent.display(),
                    e
                );
            }
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("open {}: {}", db_path.display(), e))?;

        // WAL mode: required because background sync workers write while
        // internal_search reads. Without WAL, readers block writers.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("set WAL: {}", e))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| format!("set synchronous: {}", e))?;

        // Run schema migrations
        schema::migrate(&conn).map_err(|e| format!("migrate: {}", e))?;

        info!("[index] opened {} (WAL)", db_path.display());

        Ok(Arc::new(Self {
            db: Arc::new(Mutex::new(conn)),
            db_path,
            embedder: None,
            stale_notifier: None,
        }))
    }

    /// Construct a new Indexer Arc that shares this one's DB connection but
    /// has the given embedder wired in. Used to attach the embedding chain
    /// after the LLM provider chain is built (which can't happen until config
    /// is loaded). The original Indexer remains usable.
    pub fn with_embedder(self: Arc<Self>, embedder: Arc<crate::llm::LlmChain>) -> Arc<Self> {
        Arc::new(Self {
            db: Arc::clone(&self.db),
            db_path: self.db_path.clone(),
            embedder: Some(embedder),
            stale_notifier: self.stale_notifier.clone(),
        })
    }

    /// Attach a stale notifier so that whenever a document is updated, the
    /// notifier is called and can invalidate any cached research sessions
    /// that referenced it.
    pub fn with_stale_notifier(self: Arc<Self>, notifier: Arc<dyn StaleNotifier>) -> Arc<Self> {
        Arc::new(Self {
            db: Arc::clone(&self.db),
            db_path: self.db_path.clone(),
            embedder: self.embedder.clone(),
            stale_notifier: Some(notifier),
        })
    }

    pub fn has_embedder(&self) -> bool {
        self.embedder.is_some()
    }

    /// Insert or update a document. Replaces existing rows for the same
    /// (source, external_id). Re-chunks the body and rewrites the FTS rows.
    /// If an embedder is wired, also embeds each new chunk and stores the
    /// vector. Embedding failures are logged and skipped — they don't fail
    /// the put_document call (FTS5-only ingestion still succeeds).
    pub async fn put_document(&self, doc: ExternalDoc) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let doc_for_embed = doc.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<(bool, Vec<(i64, String)>), String> {
            let mut conn = db.blocking_lock();
            let tx = conn
                .transaction()
                .map_err(|e| format!("begin: {}", e))?;

            // Compute content hash so we can skip ingestion if unchanged.
            let hash = crc32_hex(&doc.body);

            // Check existing row
            let existing: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, content_hash FROM documents WHERE source = ? AND external_id = ?",
                    params![&doc.source, &doc.external_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();

            // Skip if hash matches (no change)
            if let Some((_, prev_hash)) = &existing {
                if prev_hash == &hash {
                    debug!(
                        "[index] {} {} unchanged, skipping",
                        doc.source, doc.external_id
                    );
                    return Ok((false, Vec::new()));
                }
            }
            let was_update = existing.is_some();

            let updated_at = doc.updated_at.timestamp();
            let metadata_str = serde_json::to_string(&doc.metadata).unwrap_or_default();

            let doc_id: i64 = if let Some((id, _)) = existing {
                // Mark as stale-eligible: this is the path where the doc EXISTED
                // and the hash differs (we already returned early for unchanged).
                // Notification happens AFTER the transaction commits — see below.
                // Delete old chunks (FTS rows cascade via trigger)
                tx.execute("DELETE FROM chunks WHERE doc_id = ?", params![id])
                    .map_err(|e| format!("delete chunks: {}", e))?;
                tx.execute(
                    "UPDATE documents SET title = ?, body = ?, updated_at = ?, indexed_at = ?, content_hash = ?, metadata = ? WHERE id = ?",
                    params![
                        &doc.title,
                        &doc.body,
                        updated_at,
                        Utc::now().timestamp(),
                        &hash,
                        &metadata_str,
                        id
                    ],
                )
                .map_err(|e| format!("update doc: {}", e))?;
                id
            } else {
                tx.execute(
                    "INSERT INTO documents (source, external_id, title, body, updated_at, indexed_at, content_hash, metadata) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        &doc.source,
                        &doc.external_id,
                        &doc.title,
                        &doc.body,
                        updated_at,
                        Utc::now().timestamp(),
                        &hash,
                        &metadata_str,
                    ],
                )
                .map_err(|e| format!("insert doc: {}", e))?;
                tx.last_insert_rowid()
            };

            // Chunk the body. Simple fixed-size strategy: ~800 chars per chunk
            // with ~150 char overlap. Good enough for FTS5; vector retrieval
            // benefits more from semantic chunking but that's v2.
            let chunks = chunk_text(&doc.body, 800, 150);
            let mut chunk_ids: Vec<(i64, String)> = Vec::with_capacity(chunks.len());
            for (ord, text) in chunks.iter().enumerate() {
                tx.execute(
                    "INSERT INTO chunks (doc_id, ord, text) VALUES (?, ?, ?)",
                    params![doc_id, ord as i64, text],
                )
                .map_err(|e| format!("insert chunk: {}", e))?;
                chunk_ids.push((tx.last_insert_rowid(), text.clone()));
            }

            tx.commit().map_err(|e| format!("commit: {}", e))?;
            debug!(
                "[index] put {} {} ({} chunks){}",
                doc.source,
                doc.external_id,
                chunks.len(),
                if was_update { " [UPDATED]" } else { "" }
            );
            Ok((was_update, chunk_ids))
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?;

        // Destructure the (was_update, chunks_to_embed) tuple. If this was
        // an update of an existing document, notify the stale notifier so it
        // can invalidate any cached research sessions that referenced this doc.
        let (was_update, chunks_to_embed) = result?;
        if was_update {
            if let Some(notifier) = &self.stale_notifier {
                notifier.mark_stale_for_doc(&doc_for_embed.source, &doc_for_embed.external_id).await;
            }
        }
        // Embedding pass: best-effort, runs outside the transaction in async
        // context so we can call the LLM provider's /embeddings endpoint.
        if let Some(embedder) = &self.embedder {
            for (cid, text) in chunks_to_embed {
                let v = match embedder.embed_text(&text).await {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(
                            "[index] embed failed for chunk {}: {} (continuing FTS-only)",
                            cid, e
                        );
                        continue;
                    }
                };
                let mut v_owned = v;
                embed::normalize(&mut v_owned);
                let db = Arc::clone(&self.db);
                let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = db.blocking_lock();
                    embed::put_chunk_embedding(&conn, cid, &v_owned)
                        .map_err(|e| format!("store embedding: {}", e))
                })
                .await;
            }
        }
        // Suppress unused warning when embedder is None
        let _ = doc_for_embed;
        Ok(())
    }

    /// Delete a single document by (source, external_id).
    pub async fn delete_document(&self, source: &str, external_id: &str) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let source = source.to_string();
        let external_id = external_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut conn = db.blocking_lock();
            let tx = conn.transaction().map_err(|e| format!("begin: {}", e))?;
            tx.execute(
                "DELETE FROM chunks WHERE doc_id IN (SELECT id FROM documents WHERE source = ? AND external_id = ?)",
                params![&source, &external_id],
            ).map_err(|e| format!("delete chunks: {}", e))?;
            tx.execute(
                "DELETE FROM documents WHERE source = ? AND external_id = ?",
                params![&source, &external_id],
            ).map_err(|e| format!("delete doc: {}", e))?;
            tx.commit().map_err(|e| format!("commit: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Delete documents whose external_ids are NOT in `keep_ids` for the given source.
    /// Used by Slim-style prune passes.
    pub async fn prune(&self, source: &str, keep_ids: Vec<String>) -> Result<usize, String> {
        let db = Arc::clone(&self.db);
        let source = source.to_string();
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let mut conn = db.blocking_lock();
            let tx = conn.transaction().map_err(|e| format!("begin: {}", e))?;

            // Build a temp table of ids to keep, then delete the complement.
            tx.execute("CREATE TEMP TABLE keep_ids (external_id TEXT PRIMARY KEY)", [])
                .map_err(|e| format!("temp table: {}", e))?;
            {
                let mut stmt = tx
                    .prepare("INSERT OR IGNORE INTO keep_ids VALUES (?)")
                    .map_err(|e| format!("prepare insert: {}", e))?;
                for id in &keep_ids {
                    stmt.execute(params![id])
                        .map_err(|e| format!("insert keep: {}", e))?;
                }
            }
            let deleted_chunks = tx.execute(
                "DELETE FROM chunks WHERE doc_id IN (SELECT id FROM documents WHERE source = ? AND external_id NOT IN (SELECT external_id FROM keep_ids))",
                params![&source],
            ).map_err(|e| format!("delete chunks: {}", e))?;
            let deleted_docs = tx.execute(
                "DELETE FROM documents WHERE source = ? AND external_id NOT IN (SELECT external_id FROM keep_ids)",
                params![&source],
            ).map_err(|e| format!("delete docs: {}", e))?;
            tx.execute("DROP TABLE keep_ids", [])
                .map_err(|e| format!("drop temp: {}", e))?;
            tx.commit().map_err(|e| format!("commit: {}", e))?;
            debug!(
                "[index] pruned {}: {} docs, {} chunks",
                source, deleted_docs, deleted_chunks
            );
            Ok(deleted_docs)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Run hybrid search: BM25 (FTS5) + brute-force cosine on embeddings,
    /// fused with Reciprocal Rank Fusion. Falls back to BM25-only if no
    /// embedder is wired or query embedding fails.
    pub async fn search_hybrid(
        &self,
        query: String,
        top_k: usize,
        source_filter: Option<String>,
    ) -> Result<Vec<SearchHit>, String> {
        // Always run BM25 first — needed for snippets and as fallback
        let bm25_hits = self.search(query.clone(), top_k * 3, source_filter.clone()).await?;

        // If no embedder, just return BM25 (truncated)
        let embedder = match &self.embedder {
            Some(e) => Arc::clone(e),
            None => {
                let mut h = bm25_hits;
                h.truncate(top_k);
                return Ok(h);
            }
        };

        // Try to embed the query
        let query_vec = match embedder.embed_text(&query).await {
            Ok(v) => {
                let mut v = v;
                embed::normalize(&mut v);
                v
            }
            Err(e) => {
                log::debug!("[index] hybrid: embed query failed ({}), falling back to BM25", e);
                let mut h = bm25_hits;
                h.truncate(top_k);
                return Ok(h);
            }
        };

        // Brute-force cosine search
        let db = Arc::clone(&self.db);
        let src_filter = source_filter.clone();
        let cosine_hits: Vec<embed::VectorHit> = match tokio::task::spawn_blocking(move || -> Result<Vec<embed::VectorHit>, String> {
            let conn = db.blocking_lock();
            embed::brute_force_search(&conn, &query_vec, top_k * 3, src_filter.as_deref())
                .map_err(|e| format!("vec search: {}", e))
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))? {
            Ok(h) => h,
            Err(e) => {
                log::warn!("[index] cosine search failed ({}), falling back to BM25", e);
                let mut h = bm25_hits;
                h.truncate(top_k);
                return Ok(h);
            }
        };

        // Build a chunk_id list for the BM25 hits. SearchHit currently doesn't
        // store a chunk_id directly — we extend it via a separate lookup keyed
        // by (doc_id, chunk_ord). The cleaner long-term fix is to add chunk_id
        // to SearchHit; we keep the helper here for v2.
        let bm25_chunk_ids: Vec<i64> = {
            let db = Arc::clone(&self.db);
            let pairs: Vec<(i64, i64)> = bm25_hits
                .iter()
                .map(|h| (h.doc_id, h.chunk_ord))
                .collect();
            tokio::task::spawn_blocking(move || -> Vec<i64> {
                let conn = db.blocking_lock();
                pairs
                    .into_iter()
                    .filter_map(|(doc_id, ord)| {
                        conn.query_row(
                            "SELECT id FROM chunks WHERE doc_id = ? AND ord = ?",
                            rusqlite::params![doc_id, ord],
                            |r| r.get::<_, i64>(0),
                        )
                        .ok()
                    })
                    .collect()
            })
            .await
            .unwrap_or_default()
        };
        let cos_chunk_ids: Vec<i64> = cosine_hits.iter().map(|h| h.chunk_id).collect();

        // Run RRF fusion across both ranked lists. The result is a deduplicated
        // ordered list of (chunk_id, fused_score).
        let fused_ranked = embed::rrf_fuse(&bm25_chunk_ids, &cos_chunk_ids, 60.0);

        // Materialize the fused chunk_ids back into SearchHits via a single
        // batched lookup. Cosine-only chunks (the ones BM25 missed) are now
        // included in the result with their snippet text fetched from the
        // chunks table.
        let needed_ids: Vec<i64> = fused_ranked
            .iter()
            .take(top_k)
            .map(|(cid, _)| *cid)
            .collect();
        let db = Arc::clone(&self.db);
        let needed_for_blocking = needed_ids.clone();
        let mut materialized: Vec<SearchHit> = tokio::task::spawn_blocking(move || -> Vec<SearchHit> {
            let conn = db.blocking_lock();
            search::fetch_hits_by_chunk_ids(&conn, &needed_for_blocking).unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        // Assign rank scores from RRF (higher = better)
        let id_to_score: std::collections::HashMap<i64, f32> = fused_ranked.iter().cloned().collect();
        for hit in materialized.iter_mut() {
            let chunk_id_for_hit: i64 = {
                let db2 = Arc::clone(&self.db);
                let dt = (hit.doc_id, hit.chunk_ord);
                tokio::task::spawn_blocking(move || -> i64 {
                    let conn = db2.blocking_lock();
                    conn.query_row(
                        "SELECT id FROM chunks WHERE doc_id = ? AND ord = ?",
                        rusqlite::params![dt.0, dt.1],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                })
                .await
                .unwrap_or(0)
            };
            if let Some(score) = id_to_score.get(&chunk_id_for_hit) {
                hit.rank = (*score) as f64 * 100.0; // scale for readability
            }
        }
        materialized.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
        materialized.truncate(top_k);
        Ok(materialized)
    }

    /// Run a full-text search across the index. Returns up to `top_k` hits
    /// ordered by FTS5 BM25 (lower raw score = better match; we negate so
    /// higher returned `rank` is better).
    pub async fn search(
        &self,
        query: String,
        top_k: usize,
        source_filter: Option<String>,
    ) -> Result<Vec<SearchHit>, String> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> Result<Vec<SearchHit>, String> {
            let conn = db.blocking_lock();
            search::query(&conn, &query, top_k, source_filter.as_deref())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }

    /// Quick stats for the /stats endpoint and startup logging.
    pub async fn stats(&self) -> IndexStats {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> IndexStats {
            let conn = db.blocking_lock();
            let documents: i64 = conn
                .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
                .unwrap_or(0);
            let chunks: i64 = conn
                .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                .unwrap_or(0);
            let sources: i64 = conn
                .query_row("SELECT COUNT(DISTINCT source) FROM documents", [], |r| r.get(0))
                .unwrap_or(0);
            IndexStats { documents, chunks, sources }
        })
        .await
        .unwrap_or_default()
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Get / set the per-connector cursor blob. Used for incremental polling.
    pub async fn get_connector_cursor(&self, source: &str) -> Option<String> {
        let db = Arc::clone(&self.db);
        let source = source.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.blocking_lock();
            conn.query_row(
                "SELECT cursor FROM connector_state WHERE source = ?",
                params![&source],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .await
        .unwrap_or(None)
    }

    pub async fn set_connector_cursor(
        &self,
        source: &str,
        cursor: &str,
    ) -> Result<(), String> {
        let db = Arc::clone(&self.db);
        let source = source.to_string();
        let cursor = cursor.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = db.blocking_lock();
            conn.execute(
                "INSERT INTO connector_state (source, cursor, updated_at) VALUES (?, ?, ?) \
                 ON CONFLICT(source) DO UPDATE SET cursor = excluded.cursor, updated_at = excluded.updated_at",
                params![&source, &cursor, Utc::now().timestamp()],
            )
            .map_err(|e| format!("upsert cursor: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct IndexStats {
    pub documents: i64,
    pub chunks: i64,
    pub sources: i64,
}

/// Simple fixed-size character chunker with overlap. Word-aware split: never
/// breaks in the middle of a word at a chunk boundary.
fn chunk_text(text: &str, target: usize, overlap: usize) -> Vec<String> {
    if text.len() <= target {
        return vec![text.to_string()];
    }
    let bytes = text.as_bytes();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let end = (start + target).min(bytes.len());
        // Walk back to a whitespace boundary if we're not at the end
        let mut split = end;
        if split < bytes.len() {
            while split > start && !bytes[split - 1].is_ascii_whitespace() {
                split -= 1;
            }
            if split == start {
                split = end; // no whitespace found in window — split mid-word
            }
        }
        let chunk = String::from_utf8_lossy(&bytes[start..split]).into_owned();
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if split >= bytes.len() {
            break;
        }
        start = split.saturating_sub(overlap);
    }
    chunks
}

/// CRC32 hex digest. Cheap content hash for "did this file change?" comparisons.
/// Not cryptographic; collisions are theoretically possible but irrelevant
/// for our use case (file change detection).
fn crc32_hex(s: &str) -> String {
    // Manual CRC32 — IEEE 802.3 polynomial. Avoids pulling in another crate.
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in s.as_bytes() {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB88320
            } else {
                crc >> 1
            };
        }
    }
    format!("{:08x}", !crc)
}
