pub mod cloud;
pub mod local;
pub mod router;
pub mod stream;

use std::fmt;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::task::{Message, TokenUsage};

/// Trait that all AI execution backends must implement.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Unique identifier for this backend instance.
    fn id(&self) -> &str;

    /// Provider type (local, openrouter, anthropic, etc.).
    fn provider(&self) -> &str;

    /// What this backend can do.
    fn capabilities(&self) -> &BackendCapabilities;

    /// Check if the backend is healthy and responsive.
    async fn health(&self) -> BackendHealth;

    /// Send a completion request and get a response.
    async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, BackendError>;
}

/// Describes what a backend supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub max_tokens: u32,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub model_name: String,
    pub tags: Vec<String>,
}

/// Health status of a backend.
#[derive(Debug, Clone)]
pub struct BackendHealth {
    pub available: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub checked_at: Instant,
}

impl BackendHealth {
    pub fn healthy(latency_ms: u64) -> Self {
        Self {
            available: true,
            latency_ms: Some(latency_ms),
            error: None,
            checked_at: Instant::now(),
        }
    }

    pub fn unhealthy(error: String) -> Self {
        Self {
            available: false,
            latency_ms: None,
            error: Some(error),
            checked_at: Instant::now(),
        }
    }
}

/// A request to the AI model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Optional override for which model to use on this backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Tags that the chosen backend should match (e.g. "coding", "search").
    #[serde(default)]
    pub required_tags: Vec<String>,
}

impl CompletionRequest {
    pub fn simple(user_message: impl Into<String>) -> Self {
        Self {
            messages: vec![Message::user(user_message)],
            max_tokens: None,
            temperature: None,
            system_prompt: None,
            model_override: None,
            required_tags: Vec::new(),
        }
    }

    pub fn with_system(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.required_tags = tags;
        self
    }
}

/// Response from a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: String,
    pub backend_id: String,
    pub model: String,
    pub tokens: Option<TokenUsage>,
    pub finish_reason: Option<String>,
}

/// Errors that can occur during backend operations.
/// Errors that can occur during backend operations.
#[derive(Debug, Clone)]
pub enum BackendError {
    Unavailable(String),
    RateLimited { retry_after_secs: Option<u64> },
    Timeout,
    InvalidRequest(String),
    ModelError(String),
    NetworkError(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(msg) => write!(f, "AI model is temporarily unavailable: {}", msg),
            Self::RateLimited { retry_after_secs } => {
                write!(f, "AI model is rate limited — too many requests.")?;
                if let Some(s) = retry_after_secs {
                    write!(f, " Try again in {}s.", s)?;
                } else {
                    write!(f, " Wait a moment and try again.")?;
                }
                Ok(())
            }
            Self::Timeout => write!(f, "AI model took too long to respond — it may be overloaded, try again"),
            Self::InvalidRequest(msg) => write!(f, "Request could not be processed: {}", msg),
            Self::ModelError(msg) => write!(f, "AI model returned an error: {}", msg),
            Self::NetworkError(msg) => write!(f, "Can't reach AI model server — check that it's running: {}", msg),
        }
    }
}

impl std::error::Error for BackendError {}

/// Preferences for routing a request to a backend.
#[derive(Debug, Clone, Default)]
pub struct RoutePreferences {
    pub preferred_backend: Option<String>,
    pub fallback_backends: Vec<String>,
    pub required_tags: Vec<String>,
    pub max_latency_ms: Option<u64>,
}
