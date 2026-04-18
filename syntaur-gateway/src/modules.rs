//! Runtime module registry.
//!
//! Each module declares a group of tools it provides. At startup, the
//! gateway checks the `modules` config section and only registers tools
//! from enabled modules. This gives users modular enable/disable without
//! recompilation.
//!
//! All tools are always compiled into the binary — the module system
//! controls which tools are registered in the ToolRegistry at runtime.

use std::collections::HashMap;

use log::info;
use serde::{Deserialize, Serialize};

/// A core module — a named group of tools compiled into the gateway.
pub struct CoreModule {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Tool names this module provides.
    pub tools: &'static [&'static str],
    /// Whether this module is enabled by default.
    pub default_enabled: bool,
}

/// All core modules compiled into the gateway.
pub static CORE_MODULES: &[CoreModule] = &[
    // Split from the original `core-files` module so read-only lookups
    // (what agents do 99% of the time) can stay enabled even when users
    // want to block write/edit for safety. Existing configs that disable
    // `core-files` keep blocking writes but no longer block reads.
    CoreModule {
        id: "core-files-read",
        name: "File Reading (safe)",
        description: "Read files and list directories in the workspace. Pure read-only — cannot modify anything.",
        tools: &["memory_read", "read", "list_files", "file_read"],
        default_enabled: true,
    },
    CoreModule {
        id: "core-files",
        name: "File Write & Edit",
        description: "Write, edit, and create files in the workspace. Disable to give agents read-only access.",
        tools: &["memory_write", "write", "edit", "file_write", "file_edit"],
        default_enabled: true,
    },
    CoreModule {
        id: "core-shell",
        name: "Shell & Code Execution",
        description: "Execute shell commands and run sandboxed code",
        tools: &["exec", "shell", "run", "code_execute"],
        default_enabled: true,
    },
    CoreModule {
        id: "core-web",
        name: "Web & Search",
        description: "Web search, fetch pages, and query JSON APIs",
        tools: &["web_search", "web_fetch", "json_query", "internal_search"],
        default_enabled: true,
    },
    CoreModule {
        id: "core-telegram",
        name: "Telegram",
        description: "Send messages via Telegram",
        tools: &["send_telegram"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-comms",
        name: "Communications",
        description: "Email and SMS tools",
        tools: &["email_read", "email_send", "sms_get_number", "sms_read", "sms_wait_for_code"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-captcha",
        name: "CAPTCHA Solving",
        description: "Solve CAPTCHAs via 2Captcha API and browser bridge",
        tools: &["solve_captcha", "browser_solve_captcha",
                  "captcha_bridge_solve", "captcha_bridge_balance", "captcha_bridge_list_sites"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-office",
        name: "Office Documents",
        description: "Create and manipulate Excel, Word, and PowerPoint files",
        tools: &["office_create", "office_view", "office_get", "office_set",
                  "office_batch", "office_merge", "office_skill"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-accounts",
        name: "Account Management",
        description: "Create social media accounts and manage OAuth tokens",
        tools: &["create_instagram_account", "meta_oauth", "meta_refresh_token",
                  "threads_post", "create_facebook_account", "create_email_account",
                  "youtube_token_refresh", "youtube_reauth"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-browser",
        name: "Browser Automation",
        description: "Automate web browsers via Chromium DevTools Protocol",
        tools: &["browser_open", "browser_close", "browser_open_and_fill",
                  "browser_fill_form", "browser_fill", "browser_select",
                  "browser_set_dropdown", "browser_click", "browser_read_brief",
                  "browser_read", "browser_find_inputs", "browser_screenshot",
                  "browser_execute_js", "browser_click_at", "browser_hold_at",
                  "browser_wait"],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-coders",
        name: "Terminal / SSH",
        description: "Web-based terminal with SSH, SFTP, port forwarding, and AI assist",
        tools: &[],
        default_enabled: true,
    },
    CoreModule {
        id: "mod-voice-journal",
        name: "Voice Journal",
        description: "Record, transcribe, and search spoken conversations from wearables, phone, or desktop mic. \
                      Includes wake word training, daily transcripts, and voice training data collection.",
        tools: &["search_journal", "journal_summary", "list_recordings"],
        default_enabled: false,
    },
];

/// Module configuration from syntaur.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModulesConfig {
    /// Per-module overrides. Key is module ID, value is module config.
    #[serde(default)]
    pub entries: HashMap<String, ModuleEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Module-specific configuration.
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_true() -> bool { true }

impl ModulesConfig {
    /// Check if a module is enabled (checks config, falls back to module default).
    pub fn is_enabled(&self, module: &CoreModule) -> bool {
        match self.entries.get(module.id) {
            Some(entry) => entry.enabled,
            None => module.default_enabled,
        }
    }

    /// Get the set of tool names that should be excluded (from disabled modules).
    pub fn disabled_tools(&self) -> Vec<&'static str> {
        let mut disabled = Vec::new();
        for module in CORE_MODULES {
            if !self.is_enabled(module) {
                disabled.extend_from_slice(module.tools);
            }
        }
        disabled
    }
}

/// Log active/disabled modules at startup.
pub fn log_module_status(config: &ModulesConfig) {
    let mut active = Vec::new();
    let mut disabled = Vec::new();

    for module in CORE_MODULES {
        if config.is_enabled(module) {
            active.push(module.id);
        } else {
            disabled.push(module.id);
        }
    }

    info!(
        "[modules] {} active: {}",
        active.len(),
        active.join(", ")
    );
    if !disabled.is_empty() {
        info!(
            "[modules] {} disabled: {}",
            disabled.len(),
            disabled.join(", ")
        );
    }
}

/// Scan ~/.syntaur/modules/ for extension module manifests and inject them
/// into the MCP servers config map. This lets the existing McpRegistry
/// handle spawning, initialization, and tool discovery for extension modules
/// without any changes to the MCP subsystem.
pub fn inject_extension_modules(
    mcp_servers: &mut std::collections::HashMap<String, serde_json::Value>,
    modules_dir: &std::path::Path,
    modules_config: &ModulesConfig,
) {
    let entries = match std::fs::read_dir(modules_dir) {
        Ok(e) => e,
        Err(_) => {
            log::debug!("[modules] no modules directory at {}", modules_dir.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let manifest_path = {
            let p = entry.path();
            if p.join("syntaur.module.toml").exists() { p.join("syntaur.module.toml") } else { p.join("syntaur.module.toml") }
        };
        if !manifest_path.exists() {
            continue;
        }

        let manifest = match syntaur_sdk::ModuleManifest::from_file(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                log::warn!(
                    "[modules] failed to parse {}: {}",
                    manifest_path.display(),
                    e
                );
                continue;
            }
        };

        // Only extension modules get injected into MCP
        if manifest.tier != syntaur_sdk::manifest::ModuleTier::Extension {
            continue;
        }

        // Check if module is enabled
        if let Some(entry) = modules_config.entries.get(&manifest.id) {
            if !entry.enabled {
                log::info!("[modules] extension '{}' disabled -- skipping", manifest.id);
                continue;
            }
        }

        // Check for runtime config
        let runtime = match &manifest.runtime {
            Some(r) => r,
            None => {
                log::warn!(
                    "[modules] extension '{}' has no runtime config -- skipping",
                    manifest.id
                );
                continue;
            }
        };

        // Don't inject if already configured in mcp.servers (manual config takes precedence)
        if mcp_servers.contains_key(&manifest.id) {
            log::debug!(
                "[modules] extension '{}' already in mcp.servers -- skipping injection",
                manifest.id
            );
            continue;
        }

        // Build the MCP server config entry
        let mcp_entry = serde_json::json!({
            "command": runtime.binary,
            "args": runtime.args,
            "env": runtime.env,
            "enabled": true,
        });

        log::info!(
            "[modules] extension '{}' v{} -> injecting as MCP server ({})",
            manifest.id,
            manifest.version,
            runtime.binary
        );
        mcp_servers.insert(manifest.id.clone(), mcp_entry);
    }
}
