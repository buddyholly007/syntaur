//! Streaming completion support for OpenAI-compatible backends.
//!
//! Both local (llama.cpp) and cloud (OpenRouter) backends support SSE streaming
//! via `"stream": true`. This module provides a unified streaming interface.

use futures_util::StreamExt;
use log::{debug, warn};
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;

use crate::config::BackendProvider;
use crate::task::Message;

/// A single chunk from a streaming completion.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// A token or partial content.
    Delta(String),
    /// Stream finished, with optional finish reason.
    Done(Option<String>),
    /// An error occurred.
    Error(String),
}

/// Stream a completion from an OpenAI-compatible endpoint.
/// Returns an mpsc receiver that yields chunks as they arrive.
pub async fn stream_completion(
    client: &Client,
    base_url: &str,
    api_key: &str,
    provider: &BackendProvider,
    model: &str,
    messages: &[Message],
    system_prompt: Option<&str>,
    max_tokens: u32,
    temperature: f32,
) -> Result<mpsc::Receiver<StreamChunk>, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let mut oai_messages = Vec::new();
    if let Some(sys) = system_prompt {
        oai_messages.push(json!({"role": "system", "content": sys}));
    }
    for msg in messages {
        oai_messages.push(json!({
            "role": match msg.role {
                crate::task::MessageRole::System => "system",
                crate::task::MessageRole::User => "user",
                crate::task::MessageRole::Assistant => "assistant",
            },
            "content": msg.content,
        }));
    }

    let body = json!({
        "model": model,
        "messages": oai_messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": true,
    });

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json");

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }
    if *provider == BackendProvider::OpenRouter {
        req = req
            .header("HTTP-Referer", "https://syntaur.dev")
            .header("X-Title", "Syntaur");
    }

    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("stream request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("stream HTTP {}: {}", status, &body[..body.len().min(200)]));
    }

    let (tx, rx) = mpsc::channel(64);

    // Spawn a task to read SSE events and forward chunks
    let mut byte_stream = resp.bytes_stream();
    tokio::spawn(async move {
        let mut buffer = String::new();

        while let Some(chunk) = byte_stream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(StreamChunk::Error(e.to_string())).await;
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data.trim() == "[DONE]" {
                        let _ = tx.send(StreamChunk::Done(Some("stop".into()))).await;
                        return;
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        // Extract delta content from OpenAI SSE format
                        if let Some(delta) = parsed
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("delta"))
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            if !delta.is_empty() {
                                if tx.send(StreamChunk::Delta(delta.to_string())).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                        }

                        // Check for finish_reason
                        if let Some(reason) = parsed
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("finish_reason"))
                            .and_then(|f| f.as_str())
                        {
                            if reason != "null" {
                                let _ = tx
                                    .send(StreamChunk::Done(Some(reason.to_string())))
                                    .await;
                                return;
                            }
                        }
                    }
                }
            }
        }

        // Stream ended without [DONE]
        let _ = tx.send(StreamChunk::Done(None)).await;
    });

    Ok(rx)
}
