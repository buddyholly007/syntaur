//! `internal_search` — query the indexed knowledge base.
//!
//! First trait-based tool. Uses the FTS5 index built by the connector
//! framework to return cited results from across all indexed sources.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{Citation, RichToolResult, Tool, ToolContext};

pub struct InternalSearchTool;

const TOOL_NAME: &str = "internal_search";
const DEFAULT_TOP_K: usize = 8;
const MAX_TOP_K: usize = 25;

#[async_trait]
impl Tool for InternalSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": TOOL_NAME,
                "description": "Search the local knowledge index across all indexed sources \
                    (agent workspaces, etc.). Returns ranked snippets with citations. \
                    Use this BEFORE asking the user about something they may have already \
                    written down. Prefer this over the basic `memory_read` tool when you \
                    need cross-workspace or cross-source recall.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Free-text search query. Will be tokenized; \
                                supports multi-word AND search."
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results to return (default 8, max 25)"
                        },
                        "source": {
                            "type": "string",
                            "description": "Optional: restrict results to a single connector \
                                source (e.g. 'workspace_files'). Omit to search everything."
                        }
                    },
                    "required": ["query"]
                }
            }
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let indexer = ctx
            .indexer
            .as_ref()
            .ok_or_else(|| "indexer not available".to_string())?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'query' argument".to_string())?
            .to_string();

        if query.trim().is_empty() {
            return Err("empty query".to_string());
        }

        let top_k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|k| (k as usize).min(MAX_TOP_K).max(1))
            .unwrap_or(DEFAULT_TOP_K);

        let source_filter = args
            .get("source")
            .and_then(|v| v.as_str())
            .map(String::from);

        let hits = indexer
            .search_hybrid(query.clone(), top_k, source_filter.clone())
            .await?;

        if hits.is_empty() {
            return Ok(RichToolResult::text(format!(
                "No results for query: {}",
                query
            )));
        }

        // Format hits as markdown for the LLM. Citations duplicate this in
        // structured form so future renderers can show them properly.
        let mut content = format!(
            "# Search results for: {}\n\n{} hit(s)\n\n",
            query,
            hits.len()
        );

        let mut citations = Vec::with_capacity(hits.len());
        for (i, hit) in hits.iter().enumerate() {
            content.push_str(&format!(
                "## [{}] {}\n_source: {} • rank: {:.3}_\n\n{}\n\n",
                i + 1,
                hit.title,
                hit.source,
                hit.rank,
                hit.snippet
            ));
            citations.push(Citation {
                source: hit.source.clone(),
                external_id: hit.external_id.clone(),
                title: hit.title.clone(),
                snippet: hit.snippet.clone(),
                rank: hit.rank,
            });
        }

        Ok(RichToolResult {
            content,
            citations,
            artifacts: Vec::new(),
            structured: Some(json!({
                "query": query,
                "source_filter": source_filter,
                "hit_count": hits.len(),
            })),
        })
    }
}
