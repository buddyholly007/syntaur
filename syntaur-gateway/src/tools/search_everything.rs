//! `search_everything` — unified search across memories, workspace files,
//! uploaded docs, execution log, and (planned) todos.
//!
//! This is the FIRST-CHOICE search tool for agents. It exists because models
//! tend to cycle through `memory_recall` + `internal_search` + `memory_list`
//! in sequence when a single prompt could answer from any of them — a single
//! aggregated call is faster and stops the tool-call loop.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::extension::{Citation, RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct SearchEverythingTool;

const TOOL_NAME: &str = "search_everything";
const DEFAULT_PER_SOURCE: usize = 8;
const MAX_PER_SOURCE: usize = 20;
const MAIN_AGENT_ID: &str = "main";

#[async_trait]
impl Tool for SearchEverythingTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        "FIRST-CHOICE search: one call queries your persistent memories, \
         workspace files, uploaded docs, execution log, and daily notes together. \
         Use this BEFORE reaching for `memory_recall`, `internal_search`, or \
         `memory_list` separately — it returns everything in one ranked view. \
         Use this BEFORE asking the user about anything they may have said, \
         saved, or worked on before. If it returns no results, say so directly \
         — do NOT retry with rephrased queries."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Free-text search query. Tokenized; supports multi-word OR search."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results per source (default 8, max 20)."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'query' argument".to_string())?
            .trim()
            .to_string();

        if query.is_empty() {
            return Err("empty query".to_string());
        }

        let per_source = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|k| (k as usize).min(MAX_PER_SOURCE).max(1))
            .unwrap_or(DEFAULT_PER_SOURCE);

        // Run memory FTS and indexer search concurrently — both are read-only
        // so there's no ordering constraint.
        let memory_fut = search_memories(ctx, &query, per_source);
        let index_fut = search_index(ctx, &query, per_source);
        let (memory_res, index_res) = tokio::join!(memory_fut, index_fut);

        let memories = memory_res.unwrap_or_default();
        let (index_hits, index_err) = match index_res {
            Ok(h) => (h, None),
            Err(e) => (Vec::new(), Some(e)),
        };

        let mut content = format!("# Unified search: {}\n\n", query);
        let mut total = 0;
        let mut citations = Vec::new();

        if !memories.is_empty() {
            content.push_str(&format!("## Persistent memories ({} hit(s))\n\n", memories.len()));
            for row in &memories {
                content.push_str(&format!("- {}\n", row));
            }
            content.push('\n');
            total += memories.len();
        }

        if !index_hits.is_empty() {
            content.push_str(&format!(
                "## Workspace + indexed docs ({} hit(s))\n\n",
                index_hits.len()
            ));
            for (i, hit) in index_hits.iter().enumerate() {
                content.push_str(&format!(
                    "- [{}] **{}** _({})_\n  {}\n",
                    i + 1,
                    hit.title,
                    hit.source,
                    truncate(&hit.snippet, 240)
                ));
                citations.push(Citation {
                    source: hit.source.clone(),
                    external_id: hit.external_id.clone(),
                    title: hit.title.clone(),
                    snippet: hit.snippet.clone(),
                    rank: hit.rank,
                });
            }
            content.push('\n');
            total += index_hits.len();
        }

        if total == 0 {
            content.push_str(
                "**No results found.** Nothing in memories, workspace, indexed docs, or \
                 execution log matched this query.\n\n\
                 If the user asked about something not stored in Syntaur, say so \
                 directly — do NOT retry with rephrased queries.",
            );
            if let Some(err) = index_err {
                content.push_str(&format!("\n\n_(note: index search reported: {})_", err));
            }
        }

        Ok(RichToolResult {
            content,
            citations,
            artifacts: Vec::new(),
            structured: Some(json!({
                "query": query,
                "memory_hits": memories.len(),
                "index_hits": index_hits.len(),
                "total": total,
            })),
        })
    }
}

/// FTS5 query against the agent_memories store, scoped to the caller's agent.
async fn search_memories(ctx: &ToolContext<'_>, query: &str, limit: usize) -> Result<Vec<String>, String> {
    let Some(db) = ctx.db_path else {
        return Ok(Vec::new());
    };

    let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;

    let (scope_clause, mut scope_params) = memory_scope_sql(ctx.agent_id, ctx.user_id);

    // FTS5 OR-search on tokenized query.
    let sanitized = query.replace('"', "").replace('\'', "");
    let fts_query = sanitized
        .split_whitespace()
        .map(|w| format!("\"{}\"", w))
        .collect::<Vec<_>>()
        .join(" OR ");

    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT m.key, m.memory_type, m.title, m.content \
         FROM agent_memories m \
         JOIN agent_memories_fts f ON f.rowid = m.id \
         WHERE {} AND agent_memories_fts MATCH ? \
         ORDER BY m.importance DESC, m.updated_at DESC LIMIT {}",
        scope_clause, limit
    );
    scope_params.push(Box::new(fts_query));

    let refs: Vec<&dyn rusqlite::ToSql> = scope_params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;

    let rows: Vec<String> = stmt
        .query_map(refs.as_slice(), |r| {
            let key: String = r.get(0)?;
            let mtype: String = r.get(1)?;
            let title: String = r.get(2)?;
            let content: String = r.get(3)?;
            Ok(format!(
                "[{}] {} — {} ({})",
                mtype,
                key,
                title,
                truncate(&content, 160)
            ))
        })
        .map_err(|e| format!("query: {}", e))?
        .filter_map(Result::ok)
        .collect();

    Ok(rows)
}

/// Run the shared indexer over workspace_files / uploaded_files / execution_log etc.
/// Mirrors `internal_search`'s scoping rules so the same privacy boundaries apply.
async fn search_index(
    ctx: &ToolContext<'_>,
    query: &str,
    limit: usize,
) -> Result<Vec<crate::index::SearchHit>, String> {
    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| "indexer not available".to_string())?;

    let agent_ids = if ctx.agent_id == MAIN_AGENT_ID {
        None // main: everything except journal (indexer enforces that internally)
    } else {
        Some(vec![ctx.agent_id.to_string(), "shared".to_string()])
    };

    indexer
        .search_hybrid(query.to_string(), limit, None, agent_ids)
        .await
}

/// Build a WHERE clause for memory scoping. Mirrors `agent_scope_sql` in
/// agent_memory.rs — duplicated here because that helper is private.
fn memory_scope_sql(agent_id: &str, user_id: i64) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    let clause = if agent_id == "main" {
        "m.user_id = ? AND m.agent_id != 'journal'".to_string()
    } else if agent_id == "journal" {
        params.push(Box::new(agent_id.to_string()));
        "m.user_id = ? AND m.agent_id = ?".to_string()
    } else {
        params.push(Box::new(agent_id.to_string()));
        "(m.user_id = ? AND (m.agent_id = ? OR m.shared = 1 OR (m.agent_id = 'main' AND m.memory_type IN ('user','feedback'))))".to_string()
    };

    (clause, params)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.replace('\n', " ")
    } else {
        format!("{}...", s.chars().take(max).collect::<String>().replace('\n', " "))
    }
}
