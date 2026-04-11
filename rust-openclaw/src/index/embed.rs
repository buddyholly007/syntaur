//! Pure-Rust vector embedding helpers.
//!
//! Embeddings are stored as little-endian f32 BLOBs in the SQLite chunk_embeddings
//! table, pre-normalized so cosine similarity at query time becomes a plain dot
//! product. At our scale (~10K docs × 768 dim), brute-force search runs in
//! single-digit ms and needs no ANN structure.
//!
//! No new deps — uses only `std`, `rusqlite`, and the existing `llm.rs` for
//! generating embeddings via OpenAI-compatible `/embeddings` endpoints.

use rusqlite::{params, Connection};

/// Encode a vector of f32s as a little-endian byte buffer for BLOB storage.
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode a little-endian byte buffer back into f32s. Truncates trailing
/// partial floats; never panics on bad input.
pub fn bytes_to_vec(b: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(b.len() / 4);
    let chunks = b.chunks_exact(4);
    for c in chunks {
        out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    out
}

/// Normalize a vector in place to unit length so future cosine = dot product.
/// No-op for the zero vector.
pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// Dot product of two equal-length f32 slices. Used as cosine similarity
/// when both vectors are pre-normalized.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut s = 0.0_f32;
    let mut i = 0;
    // Loop unrolling by 4 — modest speedup for the inner loop
    while i + 4 <= n {
        s += a[i] * b[i]
            + a[i + 1] * b[i + 1]
            + a[i + 2] * b[i + 2]
            + a[i + 3] * b[i + 3];
        i += 4;
    }
    while i < n {
        s += a[i] * b[i];
        i += 1;
    }
    s
}

/// Insert or replace a chunk's embedding. Vector is expected to already be
/// normalized; the caller is responsible for that (we don't re-normalize here
/// because it would mask off-by-one bugs in the caller).
pub fn put_chunk_embedding(
    conn: &Connection,
    chunk_id: i64,
    vector: &[f32],
) -> rusqlite::Result<()> {
    let bytes = vec_to_bytes(vector);
    conn.execute(
        "INSERT OR REPLACE INTO chunk_embeddings (chunk_id, dim, vector) VALUES (?, ?, ?)",
        params![chunk_id, vector.len() as i64, bytes],
    )?;
    Ok(())
}

/// One vector hit from a brute-force search. `score` is cosine similarity
/// (in [-1, 1], higher is better).
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub chunk_id: i64,
    pub score: f32,
}

/// Brute-force search the chunk_embeddings table. Returns top_k hits ranked by
/// dot product against the query (caller pre-normalizes the query). For our
/// scale (~10K vectors × 768 dim) this completes in single-digit ms.
///
/// Optionally filtered by source via the JOIN to documents.
pub fn brute_force_search(
    conn: &Connection,
    query: &[f32],
    top_k: usize,
    source_filter: Option<&str>,
) -> rusqlite::Result<Vec<VectorHit>> {
    // Stream every (chunk_id, vector) pair through the dot-product loop and
    // keep a small min-heap of the top_k highest scores.
    let sql = if source_filter.is_some() {
        "SELECT ce.chunk_id, ce.vector FROM chunk_embeddings ce \
         JOIN chunks c ON c.id = ce.chunk_id \
         JOIN documents d ON d.id = c.doc_id \
         WHERE d.source = ?"
    } else {
        "SELECT chunk_id, vector FROM chunk_embeddings"
    };

    let mut stmt = conn.prepare(sql)?;

    // Use a Vec sorted at the end — for top_k <= 100 this is faster than a
    // BinaryHeap due to small constants.
    let mut all: Vec<VectorHit> = Vec::with_capacity(1024);

    let mut rows = if let Some(src) = source_filter {
        stmt.query(params![src])?
    } else {
        stmt.query([])?
    };

    while let Some(row) = rows.next()? {
        let chunk_id: i64 = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        let v = bytes_to_vec(&bytes);
        let score = dot(query, &v);
        all.push(VectorHit { chunk_id, score });
    }

    all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    all.truncate(top_k);
    Ok(all)
}

/// Reciprocal Rank Fusion. Combines two ranked lists by summing 1 / (k + rank)
/// for each item, where `k` is a damping constant (60 is the conventional
/// default from the original RRF paper).
///
/// Returns a `Vec<(item, fused_score)>` sorted by fused score descending.
pub fn rrf_fuse<T: Clone + Eq + std::hash::Hash>(
    list_a: &[T],
    list_b: &[T],
    k: f32,
) -> Vec<(T, f32)> {
    use std::collections::HashMap;
    let mut scores: HashMap<T, f32> = HashMap::new();
    for (rank, item) in list_a.iter().enumerate() {
        *scores.entry(item.clone()).or_insert(0.0) += 1.0 / (k + (rank as f32) + 1.0);
    }
    for (rank, item) in list_b.iter().enumerate() {
        *scores.entry(item.clone()).or_insert(0.0) += 1.0 / (k + (rank as f32) + 1.0);
    }
    let mut out: Vec<(T, f32)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}
