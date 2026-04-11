//! News headlines tool — fetches top headlines via web search.
//!
//! Uses SearXNG (already configured) to get current news. No API key needed.
//! Returns a concise voice-ready summary of top stories.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct NewsTool;

#[async_trait]
impl Tool for NewsTool {
    fn name(&self) -> &str { "news" }

    fn description(&self) -> &str {
        "Get current news headlines. Optionally filter by topic. Returns the \
         top 5 headlines with brief descriptions."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "Optional topic to filter news (e.g. 'technology', 'sports', 'Sacramento'). Default: top news."
                }
            }
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        let query = if topic.is_empty() {
            "top news today".to_string()
        } else {
            format!("{} news today", topic)
        };

        let client = ctx.http.as_ref().ok_or("no HTTP client")?;

        // Use SearXNG for news search with the news category
        let searxng_url = "http://127.0.0.1:8080/search";
        let resp = client
            .get(searxng_url)
            .query(&[
                ("q", query.as_str()),
                ("format", "json"),
                ("categories", "news"),
                ("engines", "google news,bing news,duckduckgo"),
                ("language", "en"),
            ])
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("SearXNG news search: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("SearXNG: HTTP {}", resp.status()));
        }

        let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
        let results = body
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if results.is_empty() {
            let suffix = if topic.is_empty() {
                String::new()
            } else {
                format!(" for '{}'", topic)
            };
            return Ok(RichToolResult::text(format!("No news found{}.", suffix)));
        }

        let headlines: Vec<String> = results
            .iter()
            .take(5)
            .map(|r| {
                let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                let snippet = r
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect::<String>();
                if snippet.is_empty() {
                    format!("- {}", title)
                } else {
                    format!("- {}: {}", title, snippet)
                }
            })
            .collect();

        let label = if topic.is_empty() {
            "Top news".to_string()
        } else {
            format!("{} news", topic)
        };

        Ok(RichToolResult::text(format!(
            "{}:\n{}",
            label,
            headlines.join("\n")
        )))
    }
}
