use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Lenient config loader — NEVER crashes on unknown keys.
/// All fields are Optional with defaults. Unknown keys absorbed into `extra`.

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub agents: AgentsConfig,
    pub models: ModelsConfig,
    pub channels: ChannelsConfig,
    pub gateway: GatewayConfig,
    pub plugins: PluginsConfig,
    pub hooks: HooksConfig,
    pub session: SessionConfig,
    pub commands: CommandsConfig,
    pub bindings: Vec<BindingConfig>,
    pub mcp: McpConfig,
    #[serde(default)]
    pub openapi: OpenApiConfig,
    #[serde(default)]
    pub connectors: ConnectorsConfig,
    #[serde(default)]
    pub approval: ApprovalConfig,
    /// OAuth2 authorization_code providers. Each entry's key is the
    /// provider id used in /connect <provider> and OpenApiAuth::OAuth2AuthCode.
    /// v5 Item 4.
    #[serde(default)]
    pub oauth: OAuthConfig,
    /// Runtime module enable/disable configuration.
    #[serde(default)]
    pub modules: crate::modules::ModulesConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct OAuthConfig {
    pub providers: HashMap<String, OAuthProviderConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct OAuthProviderConfig {
    pub authorization_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scopes: String,
    pub redirect_uri: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ApprovalConfig {
    /// Tools that ALWAYS require approval (additive to the hardcoded list).
    pub always_required: Vec<String>,
    /// Tools that NEVER require approval (overrides hardcoded list — use carefully).
    pub never_required: Vec<String>,
    /// Per-agent override map: { "agent_id": { "always": [...], "never": [...] } }
    pub per_agent: HashMap<String, AgentApprovalOverride>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AgentApprovalOverride {
    pub always: Vec<String>,
    pub never: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct OpenApiConfig {
    pub specs: HashMap<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ConnectorsConfig {
    pub paperless: Option<PaperlessConnectorConfig>,
    pub home_assistant: Option<HomeAssistantConnectorConfig>,
    pub bluesky: Option<BlueskyConnectorConfig>,
    pub github: Option<GithubConnectorConfig>,
    pub email: Vec<EmailConnectorConfig>,
    pub execution_log_base: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PaperlessConnectorConfig {
    pub base_url: String,
    pub token: String,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
}



#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct HomeAssistantConnectorConfig {
    /// Base URL of the Home Assistant instance, e.g. `http://192.168.1.3:8123`.
    pub base_url: String,
    /// Long-lived access token for the HA REST API. Generated from
    /// HA Profile -> Security -> Long-Lived Access Tokens.
    pub bearer_token: String,
    /// Optional shared secret that the voice `/v1/chat/completions`
    /// endpoint requires in the `Authorization: Bearer ...` header.
    /// When empty/None, the endpoint is open on the bind address.
    #[serde(default)]
    pub voice_secret: Option<String>,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
    /// ESPHome satellite host:port (e.g., "192.168.1.190:6053"). When set,
    /// Syntaur connects directly to the satellite, replacing HA for voice.
    #[serde(default)]
    pub satellite_host: Option<String>,
    /// Noise PSK for ESPHome API encryption (base64-encoded 32-byte key).
    #[serde(default)]
    pub noise_psk: Option<String>,
    /// Wyoming TTS server host:port (e.g., "192.168.1.69:10400").
    #[serde(default)]
    pub tts_host: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BlueskyConnectorConfig {
    pub actor: String,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct GithubConnectorConfig {
    pub user: String,
    pub token: String,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct EmailConnectorConfig {
    pub account_id: String,
    pub host: String,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
}

fn default_true_bool() -> bool { true }
fn default_imap_port() -> u16 { 993 }

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AgentsConfig {
    pub defaults: AgentDefaults,
    pub list: Vec<AgentEntry>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct AgentDefaults {
    pub model: ModelSelection,
    pub workspace: String,
    #[serde(rename = "contextTokens")]
    pub context_tokens: u64,
    #[serde(rename = "contextPruning")]
    pub context_pruning: ContextPruning,
    pub compaction: CompactionConfig,
    pub heartbeat: HeartbeatConfig,
    pub subagents: SubagentConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            model: ModelSelection::default(),
            workspace: "~/.syntaur/workspace".to_string(),
            context_tokens: 180000,
            context_pruning: ContextPruning::default(),
            compaction: CompactionConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            subagents: SubagentConfig::default(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ModelSelection {
    pub primary: String,
    pub fallbacks: Vec<String>,
    /// Optional cheaper/faster model used for lightweight phases
    /// (research planning, report synthesis, internal naming).
    /// Falls back to `primary` when unset.
    pub fast: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ContextPruning {
    pub mode: String,
    pub ttl: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CompactionConfig {
    pub mode: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct HeartbeatConfig {
    pub every: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SubagentConfig {
    pub model: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AgentEntry {
    pub id: String,
    pub model: Option<ModelSelection>,
    pub workspace: Option<String>,
    pub tools: Option<ToolsConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ToolsConfig {
    pub profile: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- Models ---

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ModelsConfig {
    pub mode: String,
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ProviderConfig {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub api: String,
    pub models: Vec<ModelDef>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ModelDef {
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    pub cost: Option<serde_json::Value>,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u64,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- Channels ---

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ChannelsConfig {
    pub telegram: TelegramConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "dmPolicy")]
    pub dm_policy: String,
    #[serde(rename = "groupPolicy")]
    pub group_policy: String,
    pub streaming: String,
    pub accounts: HashMap<String, TelegramAccount>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TelegramAccount {
    pub name: String,
    pub enabled: Option<bool>,
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "dmPolicy")]
    pub dm_policy: String,
    #[serde(rename = "groupPolicy")]
    pub group_policy: String,
    pub streaming: String,
    #[serde(rename = "allowFrom")]
    pub allow_from: Vec<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- Gateway ---

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct GatewayConfig {
    pub port: u16,
    pub mode: String,
    pub bind: String,
    pub auth: GatewayAuth,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: 18789,
            mode: "local".to_string(),
            bind: "loopback".to_string(),
            auth: GatewayAuth::default(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct GatewayAuth {
    pub mode: String,
    pub token: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- Security ---

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct SecurityConfig {
    #[serde(rename = "requireVoiceAuth")]
    pub require_voice_auth: bool,
    #[serde(rename = "requireSetupAuthAfterFirstRun")]
    pub require_setup_auth_after_first_run: bool,
    #[serde(rename = "allowQueryStringTokens")]
    pub allow_query_string_tokens: bool,
    #[serde(rename = "rateLimitLoginPerMinute")]
    pub rate_limit_login_per_minute: u32,
    #[serde(rename = "maxUploadSizeMb")]
    pub max_upload_size_mb: u64,
    #[serde(rename = "shellExecutionMode")]
    pub shell_execution_mode: String,
    #[serde(rename = "tokenExpiryHours")]
    pub token_expiry_hours: Option<u64>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            require_voice_auth: true,
            require_setup_auth_after_first_run: true,
            allow_query_string_tokens: false,
            rate_limit_login_per_minute: 5,
            max_upload_size_mb: 50,
            shell_execution_mode: "argv".to_string(),
            token_expiry_hours: None,
        }
    }
}

impl SecurityConfig {
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if !self.require_voice_auth {
            w.push("Voice endpoint is open without authentication. Anyone on your network can trigger smart-home actions and tools via this endpoint.".to_string());
        }
        if !self.require_setup_auth_after_first_run {
            w.push("Setup endpoints are accessible without admin login. Anyone on your network can modify your configuration, upload files, and change firewall rules.".to_string());
        }
        if self.allow_query_string_tokens {
            w.push("Tokens in URLs may leak into browser history, server logs, and HTTP referrer headers.".to_string());
        }
        if self.rate_limit_login_per_minute == 0 {
            w.push("Login rate limiting is disabled. Brute-force attacks against your password are not throttled.".to_string());
        }
        if self.max_upload_size_mb > 500 {
            w.push("Large upload limit increases denial-of-service risk.".to_string());
        }
        if self.shell_execution_mode == "shell" {
            w.push("Shell mode passes commands through sh -c which allows command chaining and injection. Use only if required for specific scripts.".to_string());
        }
        if self.token_expiry_hours.is_none() || self.token_expiry_hours == Some(0) {
            w.push("API tokens never expire. Compromised tokens remain valid until manually revoked.".to_string());
        }
        w
    }
}

// --- Plugins ---

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PluginsConfig {
    pub entries: HashMap<String, PluginEntry>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PluginEntry {
    pub enabled: bool,
    pub config: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- LCM Config (extracted from plugin config) ---

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct LcmConfig {
    #[serde(rename = "freshTailCount")]
    pub fresh_tail_count: usize,
    #[serde(rename = "contextThreshold")]
    pub context_threshold: f64,
    #[serde(rename = "incrementalMaxDepth")]
    pub incremental_max_depth: i32,
    #[serde(rename = "ignoreSessionPatterns")]
    pub ignore_session_patterns: Vec<String>,
    #[serde(rename = "summaryModel")]
    pub summary_model: String,
    #[serde(rename = "expansionModel")]
    pub expansion_model: String,
}

impl Default for LcmConfig {
    fn default() -> Self {
        Self {
            fresh_tail_count: 32,
            context_threshold: 0.75,
            incremental_max_depth: -1,
            ignore_session_patterns: vec!["agent:*:cron:**".to_string()],
            summary_model: String::new(),
            expansion_model: String::new(),
        }
    }
}

// --- Hooks, Session, Commands, Bindings, MCP ---

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct HooksConfig {
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SessionConfig {
    #[serde(rename = "dmScope")]
    pub dm_scope: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CommandsConfig {
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BindingConfig {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct McpConfig {
    pub servers: HashMap<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// --- Config Loading ---

pub struct ConfigLoadResult {
    pub config: Config,
    pub warnings: Vec<String>,
}

pub fn load_config(path: &Path) -> ConfigLoadResult {
    let mut warnings = Vec::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(format!("Cannot read config {}: {}. Using defaults.", path.display(), e));
            return ConfigLoadResult {
                config: Config::default(),
                warnings,
            };
        }
    };

    let config: Config = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(format!("Config parse error: {}. Using defaults.", e));
            // Try to salvage partial config
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(_) => warnings.push("Config is valid JSON but has structural issues.".to_string()),
                Err(_) => warnings.push("Config is not valid JSON.".to_string()),
            }
            Config::default()
        }
    };

    // Check for unknown top-level keys
    if !config.extra.is_empty() {
        for key in config.extra.keys() {
            warnings.push(format!("Unknown config key '{}' (ignored)", key));
        }
    }

    // Validate critical fields
    if config.agents.list.is_empty() {
        warnings.push("No agents defined in config".to_string());
    }
    if config.models.providers.is_empty() {
        warnings.push("No LLM providers defined in config".to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                warnings.push(format!("Config file has permissive mode {:o} — should be 600. Run: chmod 600 {}", mode, path.display()));
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    ConfigLoadResult { config, warnings }
}

impl Config {
    /// Extract LCM config from the lossless-claw plugin entry
    pub fn lcm_config(&self) -> LcmConfig {
        self.plugins.entries.get("lossless-claw")
            .and_then(|p| p.config.as_ref())
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Get the workspace path for an agent, falling back to defaults
    pub fn agent_workspace(&self, agent_id: &str) -> PathBuf {
        let agent = self.agents.list.iter().find(|a| a.id == agent_id);
        let workspace = agent
            .and_then(|a| a.workspace.as_deref())
            .unwrap_or(&self.agents.defaults.workspace);

        let expanded = workspace.replace("~", &std::env::var("HOME").unwrap_or_default());
        PathBuf::from(expanded)
    }

    /// Per-agent script execution allowlist from `agents.list[id].tools.scriptAllowlist`.
    pub fn agent_script_allowlist(&self, agent_id: &str) -> Vec<String> {
        self.agents.list.iter()
            .find(|a| a.id == agent_id)
            .and_then(|a| a.tools.as_ref())
            .and_then(|t| t.extra.get("scriptAllowlist"))
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Get the model config for an agent (agent-specific or default)
    pub fn agent_model(&self, agent_id: &str) -> &ModelSelection {
        self.agents.list.iter()
            .find(|a| a.id == agent_id)
            .and_then(|a| a.model.as_ref())
            .unwrap_or(&self.agents.defaults.model)
    }

    /// Compute the effective set of tools that require approval for the given
    /// agent. Combines: hardcoded REQUIRES_APPROVAL list + global always_required
    /// + per-agent always — minus global never_required + per-agent never.
    pub fn approval_requires(&self, agent_id: &str, tool_name: &str) -> bool {
        // Hardcoded base list (mirror of approval::REQUIRES_APPROVAL — duplicated
        // here to avoid a circular module dep). If a future tool is added to the
        // hardcoded list, add it here too.
        const HARDCODED: &[&str] = &[
            "create_email_account",
            "create_facebook_account",
            "create_instagram_account",
            "meta_oauth",
            "threads_post",
            "email_send_account",
            "sms_get_number",
            "sms_wait_for_code",
            "browser_fill_form",
            "browser_click",
        ];
        let in_base = HARDCODED.contains(&tool_name);
        let in_always = self.approval.always_required.iter().any(|s| s == tool_name);
        let in_never = self.approval.never_required.iter().any(|s| s == tool_name);
        let mut required = (in_base || in_always) && !in_never;
        if let Some(over) = self.approval.per_agent.get(agent_id) {
            if over.always.iter().any(|s| s == tool_name) {
                required = true;
            }
            if over.never.iter().any(|s| s == tool_name) {
                required = false;
            }
        }
        required
    }

    /// Resolve a model string like "lmstudio/Qwen3.5-27B" to (provider_name, model_id)
    pub fn resolve_model(&self, model_str: &str) -> Option<(String, String)> {
        // Try "provider/model" format first (e.g., "primary/gpt-4o")
        let parts: Vec<&str> = model_str.splitn(2, '/').collect();
        if parts.len() == 2 {
            if self.models.providers.contains_key(parts[0]) {
                return Some((parts[0].to_string(), parts[1].to_string()));
            }
        }
        // Bare provider name (e.g., "primary") — use the provider's configured model
        if let Some(prov) = self.models.providers.get(model_str) {
            // Model ID can be in the models list or in the flattened extra fields
            let model_id = prov.models.first().map(|m| m.id.clone())
                .or_else(|| prov.extra.get("model").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_default();
            if !model_id.is_empty() {
                return Some((model_str.to_string(), model_id));
            }
        }
        None
    }

    /// Get Telegram accounts with their bot tokens
    pub fn telegram_accounts(&self) -> Vec<(String, &TelegramAccount)> {
        self.channels.telegram.accounts.iter()
            .filter(|(_, acc)| acc.enabled.unwrap_or(true) && !acc.bot_token.is_empty())
            .map(|(id, acc)| (id.clone(), acc))
            .collect()
    }

    /// Get agent-to-telegram-account bindings
    pub fn agent_telegram_binding(&self, agent_id: &str) -> Option<&str> {
        self.bindings.iter()
            .find(|b| b.agent_id == agent_id)
            .and_then(|b| b.match_rule.as_ref())
            .and_then(|m| m.get("accountId"))
            .and_then(|v| v.as_str())
    }
}
