use std::time::Instant;

use async_trait::async_trait;
use log::{debug, warn};
use reqwest::Client;
use serde_json::json;

use super::{
    Backend, BackendCapabilities, BackendError, BackendHealth, CompletionRequest,
    CompletionResponse,
};
use crate::task::TokenUsage;

/// Backend that talks to a local llama.cpp / TurboQuant / LM Studio instance
/// via the OpenAI-compatible chat completions endpoint.
pub struct LocalBackend {
    id: String,
    url: String,
    model: String,
    capabilities: BackendCapabilities,
    client: Client,
}

impl LocalBackend {
    pub fn new(id: String, url: String, model: String, max_tokens: u32, tags: Vec<String>) -> Self {
        Self {
            id,
            capabilities: BackendCapabilities {
                max_tokens,
                supports_streaming: true,
                supports_tools: false,
                model_name: model.clone(),
                tags,
            },
            url,
            model,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
        }
    }

    fn build_messages(
        &self,
        request: &CompletionRequest,
    ) -> Vec<serde_json::Value> {
        let mut messages = Vec::new();

        if let Some(ref sys) = request.system_prompt {
            messages.push(json!({
                "role": "system",
                "content": sys,
            }));
        }

        for msg in &request.messages {
            messages.push(json!({
                "role": match msg.role {
                    crate::task::MessageRole::System => "system",
                    crate::task::MessageRole::User => "user",
                    crate::task::MessageRole::Assistant => "assistant",
                },
                "content": msg.content,
            }));
        }

        messages
    }
}

#[async_trait]
impl Backend for LocalBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn provider(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn health(&self) -> BackendHealth {
        let start = Instant::now();
        let url = format!("{}/v1/models", self.url);
        match self.client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                BackendHealth::healthy(start.elapsed().as_millis() as u64)
            }
            Ok(resp) => BackendHealth::unhealthy(format!("status {}", resp.status())),
            Err(e) => BackendHealth::unhealthy(e.to_string()),
        }
    }

    async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, BackendError> {
        let messages = self.build_messages(request);
        let model = request
            .model_override
            .as_deref()
            .unwrap_or(&self.model);

        let body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(self.capabilities.max_tokens),
            "temperature": request.temperature.unwrap_or(0.7),
            "stream": false,
        });

        let url = format!("{}/v1/chat/completions", self.url);
        debug!("[local:{}] POST {} model={}", self.id, url, model);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    BackendError::Timeout
                } else {
                    BackendError::NetworkError(e.to_string())
                }
            })?;

        let status = resp.status();
        let resp_body: serde_json::Value =
            resp.json().await.map_err(|e| BackendError::ModelError(e.to_string()))?;

        if !status.is_success() {
            let err_msg = resp_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            warn!("[local:{}] error {}: {}", self.id, status, err_msg);

            if status.as_u16() == 429 {
                return Err(BackendError::RateLimited {
                    retry_after_secs: None,
                });
            }
            if status.as_u16() == 503 || status.as_u16() == 502 {
                return Err(BackendError::Unavailable(err_msg.to_string()));
            }
            return Err(BackendError::ModelError(err_msg.to_string()));
        }

        let content = resp_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let finish_reason = resp_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("finish_reason"))
            .and_then(|f| f.as_str())
            .map(String::from);

        let tokens = resp_body.get("usage").map(|u| TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        });

        Ok(CompletionResponse {
            content,
            backend_id: self.id.clone(),
            model: model.to_string(),
            tokens,
            finish_reason,
        })
    }
}
