//! First-run configuration generator for Syntaur.
//!
//! Takes installer choices (LLM backend, agent name, voice settings,
//! modules, etc.) and produces a complete `syntaur.json` + populated
//! agent workspace with templated default files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// All choices from the installer, collected into one struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallChoices {
    // Identity
    pub agent_name: String,
    pub user_name: String,

    // LLM backends
    pub llm_primary: LlmChoice,
    pub llm_fallbacks: Vec<LlmChoice>,

    // Voice
    pub voice_enabled: bool,
    pub tts_engine: Option<String>,    // "orpheus", "piper", "elevenlabs"
    pub tts_voice: Option<String>,     // voice ID
    pub stt_engine: Option<String>,    // "parakeet", "whisper", "deepgram"
    pub wake_word: Option<String>,     // "hey atlas", custom, or none

    // Communication
    pub telegram_token: Option<String>,
    pub telegram_chat_id: Option<i64>,

    // Smart home
    pub smart_home_enabled: bool,
    pub ha_url: Option<String>,
    pub ha_token: Option<String>,

    // Modules
    pub enabled_modules: Vec<String>,
    pub disabled_modules: Vec<String>,

    // Admin
    pub admin_username: String,
    pub admin_password: String,

    // Data
    pub data_dir: PathBuf,
    pub conversation_retention_days: Option<u32>, // None = forever
    pub telemetry: bool,

    // Network
    pub gateway_port: u16,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmChoice {
    pub backend_type: LlmBackendType,
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LlmBackendType {
    Ollama,
    LlamaCpp,
    OpenRouter,
    OpenAi,
    Anthropic,
    Custom,
}

impl Default for InstallChoices {
    fn default() -> Self {
        Self {
            agent_name: "Claw".to_string(),
            user_name: "User".to_string(),
            llm_primary: LlmChoice {
                backend_type: LlmBackendType::OpenRouter,
                url: None,
                api_key: None,
                model: Some("nvidia/llama-3.3-nemotron-super-49b-v1:free".to_string()),
            },
            llm_fallbacks: Vec::new(),
            voice_enabled: false,
            tts_engine: None,
            tts_voice: None,
            stt_engine: None,
            wake_word: None,
            telegram_token: None,
            telegram_chat_id: None,
            smart_home_enabled: false,
            ha_url: None,
            ha_token: None,
            enabled_modules: Vec::new(),
            disabled_modules: Vec::new(),
            admin_username: "admin".to_string(),
            admin_password: String::new(),
            data_dir: PathBuf::from("~/.syntaur"),
            conversation_retention_days: None,
            telemetry: false,
            gateway_port: 18789,
            timezone: "UTC".to_string(),
        }
    }
}

/// Generate a complete Syntaur installation from installer choices.
pub fn generate(choices: &InstallChoices) -> Result<()> {
    let base = expand_tilde(&choices.data_dir);

    // Create directory structure
    create_dirs(&base)?;

    // Generate syntaur.json
    let config = generate_config(choices);
    let config_path = base.join("syntaur.json");
    fs::write(&config_path, serde_json::to_string_pretty(&config)?)
        .context("writing syntaur.json")?;

    // Generate agent workspace from templates
    let workspace = base.join(format!("workspace-{}", slug(&choices.agent_name)));
    generate_workspace(choices, &workspace)?;

    // Write install metadata
    let meta = serde_json::json!({
        "installed_at": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "agent_name": choices.agent_name,
        "user_name": choices.user_name,
    });
    fs::write(base.join("install.json"), serde_json::to_string_pretty(&meta)?)
        .context("writing install.json")?;

    Ok(())
}

fn create_dirs(base: &Path) -> Result<()> {
    for dir in &[
        "",
        "modules",
        "cron",
        "credentials",
        "backups",
    ] {
        fs::create_dir_all(base.join(dir))?;
    }
    Ok(())
}

fn generate_config(choices: &InstallChoices) -> serde_json::Value {
    let mut providers = serde_json::Map::new();

    // Primary LLM
    let (provider_name, provider_config) = llm_to_provider(&choices.llm_primary, "primary");
    providers.insert(provider_name.clone(), provider_config);

    // Fallbacks
    for (i, fb) in choices.llm_fallbacks.iter().enumerate() {
        let (name, config) = llm_to_provider(fb, &format!("fallback-{}", i + 1));
        providers.insert(name, config);
    }

    let agent_id = slug(&choices.agent_name);
    let workspace_path = format!("workspace-{}", agent_id);

    let mut config = serde_json::json!({
        "gateway": {
            "port": choices.gateway_port,
            "auth": {
                "mode": "password",
                "password": choices.admin_password
            }
        },
        "models": {
            "providers": providers,
            "default": provider_name
        },
        "agents": {
            "defaults": {
                "model": provider_name,
                "tools": { "profile": "full" }
            },
            "list": [{
                "id": agent_id,
                "name": choices.agent_name,
                "workspace": workspace_path
            }]
        },
        "channels": {},
        "bindings": [],
        "mcp": { "servers": {} },
        "modules": { "entries": {} },
        "session": {
            "timezone": choices.timezone
        },
        "plugins": {},
        "hooks": {},
        "commands": {}
    });

    // Telegram channel
    if let (Some(token), Some(chat_id)) = (&choices.telegram_token, choices.telegram_chat_id) {
        config["channels"]["telegram"] = serde_json::json!({
            "type": "telegram",
            "token": token,
            "allowed_chat_ids": [chat_id]
        });
        config["bindings"] = serde_json::json!([{
            "agent": agent_id,
            "channel": "telegram"
        }]);
    }

    // Smart home
    if choices.smart_home_enabled {
        if let (Some(url), Some(token)) = (&choices.ha_url, &choices.ha_token) {
            config["connectors"] = serde_json::json!({
                "home_assistant": {
                    "base_url": url,
                    "token": token
                }
            });
        }
    }

    // Module disable entries
    let mut module_entries = serde_json::Map::new();
    for m in &choices.disabled_modules {
        module_entries.insert(m.clone(), serde_json::json!({ "enabled": false }));
    }
    config["modules"]["entries"] = serde_json::Value::Object(module_entries);

    // Conversation retention
    if let Some(days) = choices.conversation_retention_days {
        config["session"]["retention_days"] = serde_json::json!(days);
    }

    config
}

fn generate_workspace(choices: &InstallChoices, workspace: &Path) -> Result<()> {
    fs::create_dir_all(workspace.join("memory"))?;
    fs::create_dir_all(workspace.join("skills"))?;

    // Template variables
    let vars = TemplateVars {
        agent_name: &choices.agent_name,
        user_name: &choices.user_name,
        voice_enabled: choices.voice_enabled,
        smart_home_enabled: choices.smart_home_enabled,
        telegram_enabled: choices.telegram_token.is_some(),
        install_date: &chrono::Utc::now().format("%Y-%m-%d").to_string(),
        llm_primary: &format_llm_choice(&choices.llm_primary),
        llm_fallback: &choices.llm_fallbacks.iter()
            .map(|f| format_llm_choice(f))
            .collect::<Vec<_>>()
            .join(", "),
        stt_engine: choices.stt_engine.as_deref().unwrap_or("none"),
        tts_engine: choices.tts_engine.as_deref().unwrap_or("none"),
        ha_url: choices.ha_url.as_deref().unwrap_or(""),
    };

    // Write each template file with variable substitution
    let templates: &[(&str, &str)] = &[
        ("AGENTS.md", include_str!("../../syntaur-defaults/agent-template/AGENTS.md")),
        ("SOUL.md", include_str!("../../syntaur-defaults/agent-template/SOUL.md")),
        ("HEARTBEAT.md", include_str!("../../syntaur-defaults/agent-template/HEARTBEAT.md")),
        ("MEMORY.md", include_str!("../../syntaur-defaults/agent-template/MEMORY.md")),
        ("PENDING_TASKS.md", include_str!("../../syntaur-defaults/agent-template/PENDING_TASKS.md")),
        ("README.md", include_str!("../../syntaur-defaults/agent-template/README.md")),
        ("FIRST_RUN.md", include_str!("../../syntaur-defaults/agent-template/FIRST_RUN.md")),
    ];

    for (filename, template) in templates {
        let content = apply_template(template, &vars);
        fs::write(workspace.join(filename), content)?;
    }

    Ok(())
}

struct TemplateVars<'a> {
    agent_name: &'a str,
    user_name: &'a str,
    voice_enabled: bool,
    smart_home_enabled: bool,
    telegram_enabled: bool,
    install_date: &'a str,
    llm_primary: &'a str,
    llm_fallback: &'a str,
    stt_engine: &'a str,
    tts_engine: &'a str,
    ha_url: &'a str,
}

fn apply_template(template: &str, vars: &TemplateVars) -> String {
    let mut result = template.to_string();

    // Simple variable substitution
    result = result.replace("{{agent_name}}", vars.agent_name);
    result = result.replace("{{user_name}}", vars.user_name);
    result = result.replace("{{install_date}}", vars.install_date);
    result = result.replace("{{llm_primary}}", vars.llm_primary);
    result = result.replace("{{llm_fallback}}", vars.llm_fallback);
    result = result.replace("{{stt_engine}}", vars.stt_engine);
    result = result.replace("{{tts_engine}}", vars.tts_engine);
    result = result.replace("{{ha_url}}", vars.ha_url);

    // Conditional blocks: {{#if flag}}...{{/if}}
    result = process_conditional(&result, "voice_enabled", vars.voice_enabled);
    result = process_conditional(&result, "smart_home_enabled", vars.smart_home_enabled);
    result = process_conditional(&result, "telegram_enabled", vars.telegram_enabled);

    result
}

fn process_conditional(input: &str, flag: &str, enabled: bool) -> String {
    let open = format!("{{{{#if {}}}}}", flag);
    let close = "{{/if}}";

    let mut result = String::new();
    let mut remaining = input;

    while let Some(start) = remaining.find(&open) {
        result.push_str(&remaining[..start]);
        let after_open = &remaining[start + open.len()..];

        if let Some(end) = after_open.find(close) {
            if enabled {
                result.push_str(&after_open[..end]);
            }
            remaining = &after_open[end + close.len()..];
        } else {
            // Unclosed conditional — keep as-is
            result.push_str(&remaining[start..]);
            remaining = "";
        }
    }
    result.push_str(remaining);
    result
}

fn llm_to_provider(choice: &LlmChoice, name: &str) -> (String, serde_json::Value) {
    match choice.backend_type {
        LlmBackendType::Ollama => {
            let url = choice.url.as_deref().unwrap_or("http://127.0.0.1:11434");
            (name.to_string(), serde_json::json!({
                "api": "openai-completions",
                "base_url": format!("{}/v1", url),
                "model": choice.model.as_deref().unwrap_or("qwen3:8b")
            }))
        }
        LlmBackendType::LlamaCpp => {
            let url = choice.url.as_deref().unwrap_or("http://127.0.0.1:1235");
            (name.to_string(), serde_json::json!({
                "api": "openai-completions",
                "base_url": format!("{}/v1", url),
                "model": choice.model.as_deref().unwrap_or("local")
            }))
        }
        LlmBackendType::OpenRouter => {
            (name.to_string(), serde_json::json!({
                "api": "openai-completions",
                "base_url": "https://openrouter.ai/api/v1",
                "api_key": choice.api_key.as_deref().unwrap_or(""),
                "model": choice.model.as_deref().unwrap_or("nvidia/llama-3.3-nemotron-super-49b-v1:free")
            }))
        }
        LlmBackendType::OpenAi => {
            (name.to_string(), serde_json::json!({
                "api": "openai-completions",
                "base_url": "https://api.openai.com/v1",
                "api_key": choice.api_key.as_deref().unwrap_or(""),
                "model": choice.model.as_deref().unwrap_or("gpt-4o-mini")
            }))
        }
        LlmBackendType::Anthropic => {
            (name.to_string(), serde_json::json!({
                "api": "anthropic",
                "api_key": choice.api_key.as_deref().unwrap_or(""),
                "model": choice.model.as_deref().unwrap_or("claude-sonnet-4-6")
            }))
        }
        LlmBackendType::Custom => {
            let url = choice.url.as_deref().unwrap_or("http://127.0.0.1:8080");
            (name.to_string(), serde_json::json!({
                "api": "openai-completions",
                "base_url": format!("{}/v1", url),
                "api_key": choice.api_key.as_deref().unwrap_or(""),
                "model": choice.model.as_deref().unwrap_or("default")
            }))
        }
    }
}

fn format_llm_choice(choice: &LlmChoice) -> String {
    match choice.backend_type {
        LlmBackendType::Ollama => format!("Ollama ({})", choice.model.as_deref().unwrap_or("default")),
        LlmBackendType::LlamaCpp => "llama.cpp (local)".to_string(),
        LlmBackendType::OpenRouter => format!("OpenRouter ({})", choice.model.as_deref().unwrap_or("free")),
        LlmBackendType::OpenAi => format!("OpenAI ({})", choice.model.as_deref().unwrap_or("gpt-4o-mini")),
        LlmBackendType::Anthropic => format!("Anthropic ({})", choice.model.as_deref().unwrap_or("claude-sonnet-4-6")),
        LlmBackendType::Custom => format!("Custom ({})", choice.url.as_deref().unwrap_or("?")),
    }
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        PathBuf::from(format!("{}/{}", home, &s[2..]))
    } else {
        path.to_path_buf()
    }
}
