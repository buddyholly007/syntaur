//! FTS5 query execution and result shaping.

use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub source: String,
    pub external_id: String,
    pub title: String,
    pub snippet: String,
    pub rank: f64,
    pub doc_id: i64,
    pub chunk_ord: i64,
}

/// Run an FTS5 query with optional source and agent filtering.
///
/// `agent_ids`: if `Some`, restrict to documents owned by those agents.
pub fn query(
    conn: &Connection,
    user_query: &str,
    top_k: usize,
    source_filter: Option<&str>,
    agent_ids: Option<&[String]>,
) -> Result<Vec<SearchHit>, String> {
    let sanitized = sanitize_fts_query(user_query);

    // Build WHERE clause dynamically
    let mut conditions = vec!["chunks_fts MATCH ?".to_string()];
    let mut bind_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    bind_values.push(Box::new(sanitized));

    if let Some(src) = source_filter {
        conditions.push("d.source = ?".to_string());
        bind_values.push(Box::new(src.to_string()));
    }
    if let Some(ids) = agent_ids {
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        conditions.push(format!("d.agent_id IN ({placeholders})"));
        for id in ids {
            bind_values.push(Box::new(id.clone()));
        }
    } else {
        // No agent filter (main agent) — exclude journal documents for privacy
        conditions.push("d.agent_id != 'journal'".to_string());
    }
    bind_values.push(Box::new(top_k as i64));

    let where_clause = conditions.join(" AND ");
    let sql = format!(
        r#"
        SELECT
            d.source,
            d.external_id,
            d.title,
            snippet(chunks_fts, 0, '<<', '>>', '...', 32) AS excerpt,
            chunks_fts.rank AS rank,
            c.doc_id AS doc_id,
            c.ord AS chunk_ord
        FROM chunks_fts
        JOIN chunks c ON c.id = chunks_fts.rowid
        JOIN documents d ON d.id = c.doc_id
        WHERE {where_clause}
        ORDER BY rank
        LIMIT ?
        "#
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare search: {}", e))?;

    let params_ref: Vec<&dyn rusqlite::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let rows: Vec<SearchHit> = stmt
        .query_map(params_ref.as_slice(), |r| {
            Ok(SearchHit {
                source: r.get(0)?,
                external_id: r.get(1)?,
                title: r.get(2)?,
                snippet: r.get(3)?,
                rank: -r.get::<_, f64>(4)?, // negate so higher = better
                doc_id: r.get(5)?,
                chunk_ord: r.get(6)?,
            })
        })
        .map_err(|e| format!("query: {}", e))?
        .filter_map(Result::ok)
        .collect();

    Ok(rows)
}

/// Sanitize a free-text user query for FTS5.
///
/// We strip FTS5 metacharacters from each whitespace-separated token, wrap
/// the cleaned tokens in double quotes, and OR-join them. The OR semantics
/// (vs FTS5 default AND) is intentional: with BM25 ranking, OR returns
/// documents that match ANY term ordered by how many terms match and how
/// rare those terms are.
///
/// Tokens shorter than 2 chars are dropped.
fn sanitize_fts_query(input: &str) -> String {
    let words: Vec<String> = input
        .split_whitespace()
        .filter_map(|w| {
            let cleaned: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if cleaned.len() < 2 {
                None
            } else {
                Some(format!("\"{}\"", cleaned))
            }
        })
        .collect();

    if words.is_empty() {
        return "\"___nothing_matches_this___\"".to_string();
    }
    words.join(" OR ")
}


/// Look up SearchHits for a specific list of chunk IDs (used by hybrid search
/// to materialize cosine-only chunks that BM25 didn't surface).
/// Returns hits in the SAME order as the input chunk_ids list — caller is
/// expected to assign rank scores from cosine similarity.
pub fn fetch_hits_by_chunk_ids(
    conn: &Connection,
    chunk_ids: &[i64],
) -> Result<Vec<SearchHit>, String> {
    if chunk_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = chunk_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        r#"
        SELECT
            d.source,
            d.external_id,
            d.title,
            substr(c.text, 1, 200) AS excerpt,
            c.doc_id AS doc_id,
            c.ord AS chunk_ord,
            c.id AS chunk_id
        FROM chunks c
        JOIN documents d ON d.id = c.doc_id
        WHERE c.id IN ({})
        "#,
        placeholders
    );

    let params_dyn: Vec<&dyn rusqlite::ToSql> = chunk_ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare lookup: {}", e))?;

    let mut id_to_hit: std::collections::HashMap<i64, SearchHit> = std::collections::HashMap::new();
    let rows = stmt
        .query_map(params_dyn.as_slice(), |r| {
            let chunk_id: i64 = r.get(6)?;
            Ok((
                chunk_id,
                SearchHit {
                    source: r.get(0)?,
                    external_id: r.get(1)?,
                    title: r.get(2)?,
                    snippet: r.get(3)?,
                    rank: 0.0, // caller fills this
                    doc_id: r.get(4)?,
                    chunk_ord: r.get(5)?,
                },
            ))
        })
        .map_err(|e| format!("query: {}", e))?;

    for row in rows.flatten() {
        id_to_hit.insert(row.0, row.1);
    }

    // Return in the input order
    let mut out = Vec::with_capacity(chunk_ids.len());
    for cid in chunk_ids {
        if let Some(hit) = id_to_hit.remove(cid) {
            out.push(hit);
        }
    }
    Ok(out)
}
