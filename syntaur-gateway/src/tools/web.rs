use log::{info, warn};

const SEARXNG_URL: &str = "http://localhost:4242";

/// Search the web via local SearXNG instance
pub async fn web_search(query: &str, max_results: usize) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("Empty search query".to_string());
    }

    let url = format!("{}/search?q={}&format=json",
        SEARXNG_URL,
        urlencoding(query),
    );

    info!("[web_search] Searching: {}", query);

    let client = reqwest::Client::new();
    let resp = client.get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Search failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Search returned HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("Parse error: {}", e))?;

    let results = body.get("results")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().take(max_results).collect::<Vec<_>>())
        .unwrap_or_default();

    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = Vec::new();
    for (i, r) in results.iter().enumerate() {
        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let snippet = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
        output.push(format!("{}. {}\n   {}\n   {}", i + 1, title, url, snippet));
    }

    info!("[web_search] {} results for '{}'", results.len(), query);
    Ok(output.join("\n\n"))
}

/// Fetch a web page
pub async fn web_fetch(url: &str) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("Empty URL".to_string());
    }

    info!("[web_fetch] Fetching: {}", url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|e| format!("Client error: {}", e))?;

    let resp = client.get(url)
        .send()
        .await
        .map_err(|e| format!("Fetch failed: {}", e))?;

    let body = resp.text().await
        .map_err(|e| format!("Read error: {}", e))?;

    // Strip HTML tags for readability
    let text = strip_html(&body);
    let trimmed: String = text.chars().take(10000).collect();

    Ok(trimmed)
}

fn urlencoding(s: &str) -> String {
    s.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        '&' => "%26".to_string(),
        '=' => "%3D".to_string(),
        '?' => "%3F".to_string(),
        '#' => "%23".to_string(),
        '+' => "%2B".to_string(),
        _ if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' => c.to_string(),
        _ => format!("%{:02X}", c as u8),
    }).collect()
}

/// Parse JSON and extract by dot-path
pub fn json_query(data: &str, path: &str) -> Result<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    if path.is_empty() {
        return Ok(serde_json::to_string_pretty(&parsed).unwrap_or_default());
    }

    let mut current = &parsed;
    for key in path.split('.') {
        current = if let Ok(idx) = key.parse::<usize>() {
            current.get(idx).ok_or_else(|| format!("Index {} not found", idx))?
        } else {
            current.get(key).ok_or_else(|| format!("Key '{}' not found", key))?
        };
    }

    match current {
        serde_json::Value::String(s) => Ok(s.clone()),
        other => Ok(serde_json::to_string_pretty(other).unwrap_or_default()),
    }
}

/// Send a Telegram message
pub async fn send_telegram(token: &str, chat_id: &str, message: &str) -> Result<String, String> {
    if token.is_empty() || chat_id.is_empty() || message.is_empty() {
        return Err("bot_token, chat_id, and message are required".to_string());
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("https://api.telegram.org/bot{}/sendMessage", token))
        .json(&serde_json::json!({"chat_id": chat_id, "text": message}))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Send failed: {}", e))?;

    if resp.status().is_success() {
        Ok("Message sent".to_string())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Telegram error: {}", body))
    }
}

fn strip_html(html: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let text = re.replace_all(html, " ");
    let ws = regex::Regex::new(r"\s+").unwrap();
    ws.replace_all(&text, " ").trim().to_string()
}
