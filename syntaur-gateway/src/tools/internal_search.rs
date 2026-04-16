//! `internal_search` — query the indexed knowledge base.
//!
//! Per-agent scoping: non-main agents search only their own knowledge +
//! shared documents. The main agent searches everything by default but can
//! restrict to a specific agent's knowledge via the `agent` parameter.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{Citation, RichToolResult, Tool, ToolContext};

pub struct InternalSearchTool;

const TOOL_NAME: &str = "internal_search";
const DEFAULT_TOP_K: usize = 8;
const MAX_TOP_K: usize = 25;
const MAIN_AGENT_ID: &str = "main";

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
                "description": "Search the local knowledge index. Each agent has its own \
                    knowledge base plus access to shared documents. The main agent can \
                    search across all agents' knowledge. Returns ranked snippets with \
                    citations. Use this BEFORE asking the user about something they may \
                    have already written down.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Free-text search query. Will be tokenized; \
                                supports multi-word OR search."
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results to return (default 8, max 25)"
                        },
                        "source": {
                            "type": "string",
                            "description": "Optional: restrict results to a single connector \
                                source (e.g. 'workspace_files'). Omit to search everything."
                        },
                        "agent": {
                            "type": "string",
                            "description": "Optional (main agent only): restrict to a \
                                specific agent's knowledge. Omit to search all agents."
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

        // Per-agent scoping:
        // - Main agent: searches all by default, or a specific agent if requested
        // - Other agents: always scoped to own agent_id + "shared"
        let agent_ids = if ctx.agent_id == MAIN_AGENT_ID {
            // Main agent can optionally restrict to a specific agent.
            // Journal documents are always excluded from main-agent searches
            // (privacy boundary — consistent with conversation isolation).
            args.get("agent")
                .and_then(|v| v.as_str())
                .map(|a| vec![a.to_string(), "shared".to_string()])
            // None = search everything except journal
            // (the search::query function handles journal exclusion when agent_ids is None)
        } else {
            // Non-main agents: own knowledge + shared
            Some(vec![ctx.agent_id.to_string(), "shared".to_string()])
        };

        let hits = indexer
            .search_hybrid(query.clone(), top_k, source_filter.clone(), agent_ids.clone())
            .await?;

        if hits.is_empty() {
            return Ok(RichToolResult::text(format!(
                "No results for query: {}",
                query
            )));
        }

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
                "agent_scope": agent_ids,
                "hit_count": hits.len(),
            })),
        })
    }
}
