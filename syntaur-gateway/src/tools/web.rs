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

/// Phase 4.5 SSRF guard. Rejects the URL forms most commonly used to pivot
/// an LLM-triggered `web_fetch` into an internal-network probe:
///
///   - Non-http(s) schemes (`file://`, `gopher://`, `ftp://`, `data:`, etc.)
///   - Loopback hosts (`127.0.0.0/8`, `::1`, `localhost`, `0.0.0.0`)
///   - RFC1918 private ranges (`10/8`, `172.16/12`, `192.168/16`)
///   - Link-local (`169.254/16`, `fe80::/10`) — blocks cloud metadata IMDS
///   - ULA IPv6 (`fc00::/7`)
///
/// A pure blocklist keeps the zero-config external-fetch path working; the
/// operator doesn't have to curate an allowlist. Legit external domains
/// resolve to public IPs and pass through.
pub fn check_url_safe(url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Invalid URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Scheme '{scheme}' not allowed. web_fetch supports http(s) only."
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_ascii_lowercase();
    if host == "localhost" || host == "0.0.0.0" {
        return Err("Blocked: localhost/0.0.0.0 address (SSRF guard)".to_string());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() {
            return Err("Blocked: loopback address (SSRF guard)".to_string());
        }
        match ip {
            std::net::IpAddr::V4(v4) => {
                let o = v4.octets();
                let private = o[0] == 10
                    || (o[0] == 172 && (16..=31).contains(&o[1]))
                    || (o[0] == 192 && o[1] == 168)
                    || (o[0] == 169 && o[1] == 254)
                    || v4.is_multicast()
                    || v4.is_broadcast();
                if private {
                    return Err(format!(
                        "Blocked: private/link-local/multicast IPv4 {v4} (SSRF guard)"
                    ));
                }
            }
            std::net::IpAddr::V6(v6) => {
                let seg = v6.segments();
                let link_local = (seg[0] & 0xffc0) == 0xfe80;
                let ula = (seg[0] & 0xfe00) == 0xfc00;
                if v6.is_loopback() || link_local || ula || v6.is_multicast() {
                    return Err(format!(
                        "Blocked: private/link-local/multicast IPv6 {v6} (SSRF guard)"
                    ));
                }
            }
        }
    }
    Ok(parsed)
}

/// Fetch a web page
pub async fn web_fetch(url: &str) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("Empty URL".to_string());
    }
    check_url_safe(url)?;

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
