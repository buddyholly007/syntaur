//! Phase 4 — Face & pet detection + clustering for the photos library.
//!
//! GPU-side dependency: requires ONNX models (MTCNN for detect,
//! ArcFace for embedding) on the gaming-pc host. For MVP this module
//! ships the gateway-side scaffolding — schema is in place, endpoints
//! are wired, and the inference path falls back to a "model not loaded"
//! response until the operator drops the ONNX files into
//! `/home/sean/.syntaur/models/faces/` on gaming-pc and points
//! `face_inference_url` at the local inference service.
//!
//! When the model service is wired, `detect_and_embed_for_file` POSTs
//! the image bytes to `${face_inference_url}/detect` and gets back
//! `[{bbox: [x,y,w,h], embedding: [f32; 512], confidence: 0.0..1.0}]`.

use anyhow::{anyhow, Result};
use axum::{extract::State, http::StatusCode, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

const FACE_INFERENCE_URL: &str = "http://192.168.1.69:8901";  // gaming-pc face service
const CLUSTER_THRESHOLD_COSINE: f32 = 0.55;                     // ArcFace similarity for same identity

#[derive(Debug, Serialize, Deserialize)]
pub struct DetectedFace {
    pub bbox: [i32; 4],            // x, y, w, h
    pub embedding: Vec<f32>,       // 512-dim ArcFace vector
    pub confidence: f32,
}

/// Detect faces in a photo file and persist embeddings + cluster
/// assignments. Returns the count of faces stored.
pub async fn detect_and_embed_for_file(
    state: &Arc<AppState>,
    file_id: i64,
    user_id: i64,
    image_bytes: Vec<u8>,
) -> Result<usize> {
    // Hit the inference service. Times out fast so missing-model
    // doesn't block the ingest path.
    let resp = state.client
        .post(format!("{FACE_INFERENCE_URL}/detect"))
        .body(image_bytes)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| anyhow!("face inference unreachable (model not loaded?): {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("face inference HTTP {}", resp.status()));
    }
    let faces: Vec<DetectedFace> = resp.json().await
        .map_err(|e| anyhow!("face inference parse: {e}"))?;
    let face_count = faces.len();
    if face_count == 0 { return Ok(0); }

    let db_path = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("db: {e}"))?;
        for face in faces {
            let cluster_id = assign_to_cluster(&conn, user_id, &face.embedding)?;
            let blob = embedding_to_blob(&face.embedding);
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO library_face_detections
                 (file_id, cluster_id, bbox_x, bbox_y, bbox_w, bbox_h, embedding, confidence, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![file_id, cluster_id,
                        face.bbox[0], face.bbox[1], face.bbox[2], face.bbox[3],
                        &blob, face.confidence as f64, now],
            ).map_err(|e| anyhow!("insert detection: {e}"))?;
        }
        Ok(())
    }).await.map_err(|e| anyhow!("join: {e}"))??;
    Ok(face_count)
}

/// Assign an embedding to an existing cluster (closest match within
/// cosine threshold) or create a new cluster. Returns the cluster id.
fn assign_to_cluster(
    conn: &rusqlite::Connection,
    user_id: i64,
    embedding: &[f32],
) -> Result<i64> {
    let mut stmt = conn.prepare(
        "SELECT fc.id, fd.embedding
         FROM library_face_clusters fc
         JOIN library_face_detections fd ON fd.cluster_id = fc.id
         WHERE fc.user_id = ? AND fd.embedding IS NOT NULL
         ORDER BY fc.id"
    ).map_err(|e| anyhow!("prep cluster scan: {e}"))?;

    let rows = stmt.query_map(params![user_id], |r| Ok((
        r.get::<_, i64>(0)?,
        r.get::<_, Vec<u8>>(1)?,
    ))).map_err(|e| anyhow!("query: {e}"))?;

    let mut best: Option<(i64, f32)> = None;
    for row in rows {
        let (cid, blob) = row.map_err(|e| anyhow!("row: {e}"))?;
        let other = blob_to_embedding(&blob);
        if other.len() != embedding.len() { continue; }
        let sim = cosine_similarity(embedding, &other);
        if sim > CLUSTER_THRESHOLD_COSINE && best.map(|b| sim > b.1).unwrap_or(true) {
            best = Some((cid, sim));
        }
    }

    match best {
        Some((cid, _)) => Ok(cid),
        None => {
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO library_face_clusters (user_id, name, kind, sample_file_id, created_at) VALUES (?, NULL, 'person', NULL, ?)",
                params![user_id, now],
            ).map_err(|e| anyhow!("create cluster: {e}"))?;
            Ok(conn.last_insert_rowid())
        }
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0_f32; let mut na = 0.0_f32; let mut nb = 0.0_f32;
    for i in 0..a.len() {
        dot += a[i] * b[i]; na += a[i] * a[i]; nb += b[i] * b[i];
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-8);
    dot / denom
}

fn embedding_to_blob(e: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(e.len() * 4);
    for f in e { out.extend_from_slice(&f.to_le_bytes()); }
    out
}

fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// ── Cluster management endpoints ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ClusterRename { pub name: String }

pub async fn handle_list_clusters(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();

    let rows: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || -> Vec<serde_json::Value> {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return vec![] };
        let mut stmt = match conn.prepare(
            "SELECT fc.id, fc.name, fc.kind, fc.sample_file_id, COUNT(fd.id) as photos
             FROM library_face_clusters fc
             LEFT JOIN library_face_detections fd ON fd.cluster_id = fc.id
             WHERE fc.user_id = ?
             GROUP BY fc.id ORDER BY photos DESC"
        ) { Ok(s) => s, Err(_) => return vec![] };
        let mapped = stmt.query_map(params![user_id], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "name": r.get::<_, Option<String>>(1)?,
                "kind": r.get::<_, String>(2)?,
                "sample_file_id": r.get::<_, Option<i64>>(3)?,
                "photo_count": r.get::<_, i64>(4)?,
            }))
        });
        match mapped { Ok(it) => it.filter_map(|r| r.ok()).collect(), Err(_) => vec![] }
    }).await.unwrap_or_default();

    Ok(Json(serde_json::json!({ "clusters": rows })))
}

pub async fn handle_rename_cluster(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(cluster_id): axum::extract::Path<i64>,
    Json(body): Json<ClusterRename>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = crate::security::bearer_from_headers(&headers);
    let principal = crate::resolve_principal(&state, token).await?;
    let user_id = principal.user_id();
    let db_path = state.db_path.clone();
    let name = body.name.trim().to_string();
    if name.is_empty() { return Err(StatusCode::BAD_REQUEST); }

    let _ = tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) { Ok(c) => c, Err(_) => return };
        let _ = conn.execute(
            "UPDATE library_face_clusters SET name = ? WHERE id = ? AND user_id = ?",
            params![&name, cluster_id, user_id],
        );
    }).await;

    Ok(Json(serde_json::json!({ "success": true })))
}
