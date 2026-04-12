use std::time::Instant;

use async_trait::async_trait;
use log::{debug, warn};
use reqwest::Client;
use serde_json::json;

use super::{
    Backend, BackendCapabilities, BackendError, BackendHealth, CompletionRequest,
    CompletionResponse,
};
use crate::config::BackendProvider;
use crate::task::TokenUsage;

/// Backend that talks to cloud APIs (OpenRouter, Anthropic, or any OpenAI-compatible endpoint).
pub struct CloudBackend {
    id: String,
    provider: BackendProvider,
    url: String,
    api_key: String,
    model: String,
    capabilities: BackendCapabilities,
    client: Client,
}

impl CloudBackend {
    pub fn new(
        id: String,
        provider: BackendProvider,
        url: String,
        api_key: String,
        model: String,
        max_tokens: u32,
        tags: Vec<String>,
    ) -> Self {
        Self {
            id,
            capabilities: BackendCapabilities {
                max_tokens,
                supports_streaming: true,
                supports_tools: true,
                model_name: model.clone(),
                tags,
            },
            provider,
            url,
            api_key,
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

    /// Build request body for Anthropic native API (Messages API).
    fn build_anthropic_body(
        &self,
        request: &CompletionRequest,
        model: &str,
    ) -> serde_json::Value {
        let mut messages = Vec::new();
        for msg in &request.messages {
            let role = match msg.role {
                crate::task::MessageRole::System => continue, // handled separately
                crate::task::MessageRole::User => "user",
                crate::task::MessageRole::Assistant => "assistant",
            };
            messages.push(json!({
                "role": role,
                "content": msg.content,
            }));
        }

        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(self.capabilities.max_tokens),
        });

        // Collect system prompts
        let mut system_parts = Vec::new();
        if let Some(ref sys) = request.system_prompt {
            system_parts.push(sys.clone());
        }
        for msg in &request.messages {
            if msg.role == crate::task::MessageRole::System {
                system_parts.push(msg.content.clone());
            }
        }
        if !system_parts.is_empty() {
            body["system"] = json!(system_parts.join("\n\n"));
        }

        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }

        body
    }

    /// Build request body for OpenAI-compatible APIs (OpenRouter, LM Studio, etc.).
    fn build_openai_body(
        &self,
        request: &CompletionRequest,
        model: &str,
    ) -> serde_json::Value {
        let messages = self.build_messages(request);

        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(self.capabilities.max_tokens),
            "stream": false,
        });

        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }

        body
    }

    async fn complete_anthropic(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, BackendError> {
        let model = request.model_override.as_deref().unwrap_or(&self.model);
        let body = self.build_anthropic_body(request, model);
        let url = format!("{}/v1/messages", self.url.trim_end_matches('/'));

        debug!("[cloud:{}] POST {} model={}", self.id, url, model);

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
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
            return Err(self.parse_error(status.as_u16(), &resp_body));
        }

        let content = resp_body
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        let tokens = resp_body.get("usage").map(|u| TokenUsage {
            prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            total_tokens: (u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                + u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0))
                as u32,
        });

        let finish_reason = resp_body
            .get("stop_reason")
            .and_then(|f| f.as_str())
            .map(String::from);

        Ok(CompletionResponse {
            content,
            backend_id: self.id.clone(),
            model: model.to_string(),
            tokens,
            finish_reason,
        })
    }

    async fn complete_openai(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, BackendError> {
        let model = request.model_override.as_deref().unwrap_or(&self.model);
        let body = self.build_openai_body(request, model);
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        debug!("[cloud:{}] POST {} model={}", self.id, url, model);

        let mut req_builder = self
            .client
            .post(&url)
            .header("content-type", "application/json");

        // OpenRouter uses Bearer auth, others may vary
        req_builder = req_builder.header("authorization", format!("Bearer {}", self.api_key));

        // OpenRouter-specific headers
        if self.provider == BackendProvider::OpenRouter {
            req_builder = req_builder
                .header("HTTP-Referer", "https://syntaur.dev")
                .header("X-Title", "Syntaur");
        }

        let resp = req_builder
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
            return Err(self.parse_error(status.as_u16(), &resp_body));
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

    fn parse_error(&self, status: u16, body: &serde_json::Value) -> BackendError {
        let msg = body
            .get("error")
            .and_then(|e| e.get("message").or(Some(e)))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        warn!("[cloud:{}] error {}: {}", self.id, status, msg);

        match status {
            429 => BackendError::RateLimited {
                retry_after_secs: None,
            },
            402 => BackendError::Unavailable(format!("payment required: {}", msg)),
            502 | 503 | 504 => BackendError::Unavailable(msg.to_string()),
            400 => BackendError::InvalidRequest(msg.to_string()),
            _ => BackendError::ModelError(format!("{}: {}", status, msg)),
        }
    }
}

#[async_trait]
impl Backend for CloudBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn provider(&self) -> &str {
        match self.provider {
            BackendProvider::Anthropic => "anthropic",
            BackendProvider::OpenRouter => "openrouter",
            BackendProvider::OpenAiCompat => "openai_compat",
            BackendProvider::Local => "local",
        }
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn health(&self) -> BackendHealth {
        let start = Instant::now();

        // For OpenRouter/OpenAI-compat, try the models endpoint
        let url = match self.provider {
            BackendProvider::Anthropic => format!("{}/v1/messages", self.url.trim_end_matches('/')),
            _ => format!("{}/models", self.url.trim_end_matches('/')),
        };

        let mut req = self.client.get(&url);

        match self.provider {
            BackendProvider::Anthropic => {
                req = req
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01");
            }
            _ => {
                req = req.header("authorization", format!("Bearer {}", self.api_key));
            }
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 405 => {
                // 405 = Method Not Allowed on Anthropic messages endpoint, but server is up
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
        match self.provider {
            BackendProvider::Anthropic => self.complete_anthropic(request).await,
            _ => self.complete_openai(request).await,
        }
    }
}
