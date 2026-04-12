//! Network LLM service discovery.
//!
//! Scans the local network for running LLM services:
//! - Ollama (port 11434)
//! - LM Studio (port 1234)
//! - TurboQuant / llama.cpp (port 1235)
//! - Any OpenAI-compatible endpoint

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, TcpStream};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmService {
    pub name: String,
    pub url: String,
    pub host: String,
    pub port: u16,
    pub models: Vec<String>,
    pub service_type: LlmServiceType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LlmServiceType {
    Ollama,
    LmStudio,
    LlamaCpp,
    OpenAiCompatible,
}

impl std::fmt::Display for LlmServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ollama => write!(f, "Ollama"),
            Self::LmStudio => write!(f, "LM Studio"),
            Self::LlamaCpp => write!(f, "llama.cpp / TurboQuant"),
            Self::OpenAiCompatible => write!(f, "OpenAI-compatible"),
        }
    }
}

/// Well-known LLM service ports and their types.
const KNOWN_PORTS: &[(u16, LlmServiceType)] = &[
    (11434, LlmServiceType::Ollama),
    (1234, LlmServiceType::LmStudio),
    (1235, LlmServiceType::LlamaCpp),
    (1236, LlmServiceType::LlamaCpp),  // LLM proxy
    (8080, LlmServiceType::OpenAiCompatible),  // common llama.cpp port
];

/// Discover LLM services on the local network.
pub async fn discover_llm_services() -> Vec<LlmService> {
    let mut services = Vec::new();

    // Get local network range
    let local_ips = get_local_subnet_ips();

    // Check localhost first (most common)
    for (port, svc_type) in KNOWN_PORTS {
        if let Some(svc) = probe_host("127.0.0.1", *port, svc_type.clone()).await {
            services.push(svc);
        }
    }

    // Check LAN hosts in parallel
    let mut handles = Vec::new();
    for ip in &local_ips {
        let ip = ip.to_string();
        if ip == "127.0.0.1" { continue; }
        for (port, svc_type) in KNOWN_PORTS {
            let ip = ip.clone();
            let port = *port;
            let svc_type = svc_type.clone();
            handles.push(tokio::spawn(async move {
                probe_host(&ip, port, svc_type).await
            }));
        }
    }

    for handle in handles {
        if let Ok(Some(svc)) = handle.await {
            services.push(svc);
        }
    }

    services
}

async fn probe_host(host: &str, port: u16, svc_type: LlmServiceType) -> Option<LlmService> {
    // Quick TCP connect check first
    let addr = format!("{}:{}", host, port);
    if TcpStream::connect_timeout(
        &addr.parse().ok()?,
        Duration::from_millis(500),
    ).is_err() {
        return None;
    }

    // Try to get model list via the appropriate API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;

    let (url, models) = match &svc_type {
        LlmServiceType::Ollama => {
            let url = format!("http://{}:{}", host, port);
            let models = fetch_ollama_models(&client, &url).await;
            (url, models)
        }
        _ => {
            // OpenAI-compatible: GET /v1/models
            let url = format!("http://{}:{}", host, port);
            let models = fetch_openai_models(&client, &url).await;
            (url, models)
        }
    };

    let name = format!("{} at {}", svc_type, addr);

    Some(LlmService {
        name,
        url,
        host: host.to_string(),
        port,
        models,
        service_type: svc_type,
    })
}

async fn fetch_ollama_models(client: &reqwest::Client, base_url: &str) -> Vec<String> {
    let url = format!("{}/api/tags", base_url);
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    body.get("models")
        .and_then(|m| m.as_array())
        .map(|models| {
            models.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_openai_models(client: &reqwest::Client, base_url: &str) -> Vec<String> {
    let url = format!("{}/v1/models", base_url);
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    body.get("data")
        .and_then(|d| d.as_array())
        .map(|models| {
            models.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Get IPs on the local subnet to probe. Returns a small set of
/// common addresses rather than scanning the full /24.
fn get_local_subnet_ips() -> Vec<String> {
    // Parse local IP to determine subnet
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = std::process::Command::new("ip")
            .args(["route", "get", "1.1.1.1"])
            .output()
        {
            let out = String::from_utf8_lossy(&output.stdout);
            // "1.1.1.1 via 192.168.1.1 dev eth0 src 192.168.1.69"
            if let Some(src_ip) = out.split("src ").nth(1).and_then(|s| s.split_whitespace().next()) {
                if let Some(prefix) = src_ip.rsplitn(2, '.').nth(1) {
                    // Scan common LAN addresses
                    let mut ips = Vec::new();
                    for i in 1..=254u8 {
                        ips.push(format!("{}.{}", prefix, i));
                    }
                    return ips;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("route")
            .args(["-n", "get", "default"])
            .output()
        {
            let out = String::from_utf8_lossy(&output.stdout);
            if let Some(gw_line) = out.lines().find(|l| l.contains("gateway:")) {
                if let Some(gw) = gw_line.split(':').nth(1).map(|s| s.trim()) {
                    if let Some(prefix) = gw.rsplitn(2, '.').nth(1) {
                        let mut ips = Vec::new();
                        for i in 1..=254u8 {
                            ips.push(format!("{}.{}", prefix, i));
                        }
                        return ips;
                    }
                }
            }
        }
    }

    // Fallback: just check common subnets
    let mut ips = Vec::new();
    for prefix in &["192.168.1", "192.168.0", "10.0.0"] {
        for i in 1..=254u8 {
            ips.push(format!("{}.{}", prefix, i));
        }
    }
    ips
}
