//! Wikipedia lookup tool — answers factual questions via Wikipedia REST API.
//!
//! Uses the free Wikipedia REST API (no key needed). Fetches the summary
//! extract for an article, perfect for "what is a quokka" type voice queries.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct WikipediaTool;

#[async_trait]
impl Tool for WikipediaTool {
    fn name(&self) -> &str { "wikipedia" }

    fn description(&self) -> &str {
        "Look up a topic on Wikipedia and return a concise summary. Use for \
         factual questions about people, places, things, concepts, history, \
         science, etc."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "The topic to look up on Wikipedia."
                }
            },
            "required": ["topic"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let topic = args
            .get("topic")
            .and_then(|v| v.as_str())
            .ok_or("wikipedia: 'topic' is required")?
            .trim();

        let client = ctx.http.as_ref().ok_or("no HTTP client")?;

        // Wikipedia REST API — get the summary for a page title
        let url = format!(
            "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
            topic.replace(' ', "_")
        );

        let resp = client
            .get(&url)
            .header("User-Agent", "syntaur/0.1 (peter-voice-assistant)")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Wikipedia API: {}", e))?;

        if resp.status().as_u16() == 404 {
            // Try search instead of direct title
            return search_wikipedia(client, topic).await;
        }

        if !resp.status().is_success() {
            return Err(format!("Wikipedia API: HTTP {}", resp.status()));
        }

        let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;

        let title = body.get("title").and_then(|v| v.as_str()).unwrap_or(topic);
        let extract = body
            .get("extract")
            .and_then(|v| v.as_str())
            .unwrap_or("No summary available.");

        // Trim to ~500 chars for voice
        let summary = if extract.len() > 500 {
            let cut = extract[..500].rfind(". ").unwrap_or(497);
            format!("{}.", &extract[..=cut])
        } else {
            extract.to_string()
        };

        Ok(RichToolResult::text(format!("{}: {}", title, summary)))
    }
}

async fn search_wikipedia(
    client: &std::sync::Arc<reqwest::Client>,
    query: &str,
) -> Result<RichToolResult, String> {
    let search_url = "https://en.wikipedia.org/w/api.php";
    let resp = client
        .get(search_url)
        .query(&[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", query),
            ("format", "json"),
            ("srlimit", "3"),
        ])
        .header("User-Agent", "syntaur/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Wikipedia search: {}", e))?;

    let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
    let results = body
        .get("query")
        .and_then(|q| q.get("search"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(RichToolResult::text(format!(
            "No Wikipedia article found for '{}'.",
            query
        )));
    }

    // Take the first result and fetch its summary
    let first_title = results[0]
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(query);

    let summary_url = format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
        first_title.replace(' ', "_")
    );

    let resp = client
        .get(&summary_url)
        .header("User-Agent", "syntaur/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Wikipedia API: {}", e))?;

    if !resp.status().is_success() {
        let snippet = results[0]
            .get("snippet")
            .and_then(|v| v.as_str())
            .unwrap_or("No summary.");
        // Strip HTML from snippet
        let clean = snippet
            .replace("<span class=\"searchmatch\">", "")
            .replace("</span>", "");
        return Ok(RichToolResult::text(format!("{}: {}", first_title, clean)));
    }

    let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
    let extract = body
        .get("extract")
        .and_then(|v| v.as_str())
        .unwrap_or("No summary.");

    let summary = if extract.len() > 500 {
        let cut = extract[..500].rfind(". ").unwrap_or(497);
        format!("{}.", &extract[..=cut])
    } else {
        extract.to_string()
    };

    Ok(RichToolResult::text(format!("{}: {}", first_title, summary)))
}
