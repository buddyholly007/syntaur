//! Pure-Rust replacement for the Python `searxng-mcp` shim.
//!
//! Two tools:
//!   * `searxng_search(query, max_results)` — wraps `<SEARXNG>/search?format=json`
//!   * `searxng_fetch(url)` — fetches a URL and returns plain-text body
//!
//! `SEARXNG` is read from `SEARXNG_URL` env, defaulting to `http://localhost:4242`.

use std::env;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcp_protocol::messages::ServerInfo;
use mcp_protocol::server::{ServerHandler, ToolCallResult, ToolDef};
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_SEARXNG: &str = "http://localhost:4242";
const REQUEST_TIMEOUT_SECS: u64 = 15;
const MAX_FETCH_BYTES: usize = 8_000;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; syntaur-mcp-search-rs/0.1)";

struct SearchHandler {
    searxng_url: String,
    http: reqwest::Client,
}

impl SearchHandler {
    fn new() -> anyhow::Result<Self> {
        let searxng_url =
            env::var("SEARXNG_URL").unwrap_or_else(|_| DEFAULT_SEARXNG.to_string());
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { searxng_url, http })
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<String, String> {
        let url = format!("{}/search", self.searxng_url);
        let resp = self
            .http
            .get(&url)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await
            .map_err(|e| format!("searxng request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("searxng returned {}", resp.status()));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("searxng returned invalid json: {}", e))?;

        let results = body
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if results.is_empty() {
            return Ok("No results.".to_string());
        }

        let formatted: Vec<String> = results
            .into_iter()
            .take(max_results)
            .map(|r| {
                let title = r
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = r
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let snippet = r
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                format!("**{}**\n{}\n{}", title, url, snippet)
            })
            .collect();
        Ok(formatted.join("\n\n"))
    }

    async fn fetch(&self, url: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Error fetching {}: {}", url, e))?;

        if !resp.status().is_success() {
            return Err(format!("Error fetching {}: status {}", url, resp.status()));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Error reading body for {}: {}", url, e))?;

        // html2text handles structured extraction (paragraphs, links, lists)
        // far better than the regex tag-strip the Python shim used. It returns
        // String directly (no Result), so we just call it.
        let text = html2text::from_read(&bytes[..], 100);
        let trimmed = text.trim();
        if trimmed.len() > MAX_FETCH_BYTES {
            Ok(trimmed[..MAX_FETCH_BYTES].to_string())
        } else {
            Ok(trimmed.to_string())
        }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

fn default_max_results() -> usize {
    10
}

#[derive(Deserialize)]
struct FetchArgs {
    url: String,
}

#[async_trait]
impl ServerHandler for SearchHandler {
    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: "searxng-mcp-rs".to_string(),
            version: "0.1.0".to_string(),
        }
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "searxng_search",
                description: "Search the web using SearXNG (aggregates Google, Bing, DuckDuckGo). Returns titles, URLs, and snippets.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"},
                        "max_results": {"type": "integer", "description": "Max results (default 10)", "default": 10}
                    },
                    "required": ["query"]
                }),
            },
            ToolDef {
                name: "searxng_fetch",
                description: "Fetch and extract text content from a URL.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL to fetch"}
                    },
                    "required": ["url"]
                }),
            },
        ]
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> ToolCallResult {
        match name {
            "searxng_search" => {
                let a: SearchArgs = match serde_json::from_value(arguments) {
                    Ok(a) => a,
                    Err(e) => return ToolCallResult::error(format!("invalid arguments: {}", e)),
                };
                match self.search(&a.query, a.max_results).await {
                    Ok(text) => ToolCallResult::text(text),
                    Err(e) => ToolCallResult::error(format!("Error: {}", e)),
                }
            }
            "searxng_fetch" => {
                let a: FetchArgs = match serde_json::from_value(arguments) {
                    Ok(a) => a,
                    Err(e) => return ToolCallResult::error(format!("invalid arguments: {}", e)),
                };
                match self.fetch(&a.url).await {
                    Ok(text) => ToolCallResult::text(text),
                    Err(e) => ToolCallResult::error(e),
                }
            }
            _ => ToolCallResult::error(format!("Unknown tool: {}", name)),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .try_init();

    let handler = match SearchHandler::new() {
        Ok(h) => Arc::new(h),
        Err(e) => {
            eprintln!("init failed: {:#}", e);
            return ExitCode::from(2);
        }
    };
    log::info!("searxng url = {}", handler.searxng_url);

    if let Err(e) = mcp_protocol::run_stdio_server(handler).await {
        log::error!("server exited with error: {}", e);
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
