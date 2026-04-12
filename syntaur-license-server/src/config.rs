use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfig {
    pub server: ServerConfig,
    pub license: LicenseConfig,
    pub backends: Vec<BackendConfig>,
    pub agents: AgentsConfig,
    pub executor: ExecutorConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub server_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseConfig {
    pub stripe_secret_key: String,
    pub stripe_webhook_secret: String,
    pub stripe_price_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub id: String,
    pub provider: BackendProvider,
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub max_tokens: u32,
    pub priority: u32,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BackendProvider {
    Local,
    OpenRouter,
    Anthropic,
    OpenAiCompat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    pub default_major_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    pub max_retries: u32,
    #[serde(with = "duration_secs")]
    pub retry_delay: Duration,
    #[serde(with = "duration_secs")]
    pub default_timeout: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            retry_delay: Duration::from_secs(2),
            default_timeout: Duration::from_secs(120),
        }
    }
}

impl PlatformConfig {
    pub fn from_env() -> Self {
        let stripe_secret = std::env::var("STRIPE_SECRET_KEY").unwrap_or_default();
        let stripe_webhook = std::env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default();
        let stripe_price = std::env::var("STRIPE_PRICE_ID").unwrap_or_default();
        let server_url =
            std::env::var("SERVER_URL").unwrap_or_else(|_| "http://127.0.0.1:18800".into());
        let port: u16 = std::env::var("PORT")
            .unwrap_or_else(|_| "18800".into())
            .parse()
            .unwrap_or(18800);

        let mut backends = Vec::new();

        // Local backend from env
        if let Ok(url) = std::env::var("LOCAL_BACKEND_URL") {
            let model =
                std::env::var("LOCAL_BACKEND_MODEL").unwrap_or_else(|_| "local-model".into());
            backends.push(BackendConfig {
                id: "local".into(),
                provider: BackendProvider::Local,
                url,
                api_key: None,
                model,
                max_tokens: std::env::var("LOCAL_BACKEND_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4096),
                priority: 1,
                tags: vec!["general".into(), "coding".into()],
            });
        }

        // Cloud backend from env
        if let Ok(api_key) = std::env::var("CLOUD_API_KEY") {
            let url = std::env::var("CLOUD_API_URL")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
            let model = std::env::var("CLOUD_MODEL")
                .unwrap_or_else(|_| "openai/gpt-4o".into());
            backends.push(BackendConfig {
                id: "cloud".into(),
                provider: BackendProvider::OpenRouter,
                url,
                api_key: Some(api_key),
                model,
                max_tokens: std::env::var("CLOUD_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4096),
                priority: 2,
                tags: vec!["general".into(), "coding".into(), "search".into()],
            });
        }

        PlatformConfig {
            server: ServerConfig { port, server_url },
            license: LicenseConfig {
                stripe_secret_key: stripe_secret,
                stripe_webhook_secret: stripe_webhook,
                stripe_price_id: stripe_price,
            },
            backends,
            agents: AgentsConfig {
                default_major_agent: "assistant".into(),
            },
            executor: ExecutorConfig::default(),
        }
    }
}

pub(crate) mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}
