//! Module manifest — parsed from `openclaw.module.toml`.
//!
//! Every module (core or extension) declares its metadata, configuration
//! schema, and provided capabilities in a TOML manifest file.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Module tier determines how it integrates with the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleTier {
    /// Compiled into the gateway binary via Cargo feature flags.
    Core,
    /// Separate binary communicating via MCP protocol or HTTP sidecar.
    Extension,
}

/// Extension module protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExtensionProtocol {
    /// MCP over stdio (JSON-RPC). The gateway spawns the binary and
    /// communicates via stdin/stdout.
    Mcp,
    /// HTTP sidecar on a localhost port. The gateway starts the binary
    /// and proxies tool calls via HTTP.
    HttpSidecar {
        port: u16,
    },
}

/// Declarative tool metadata from the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub requires_approval: bool,
}

/// Health check configuration for extension modules.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum HealthCheck {
    Http {
        url: String,
        #[serde(default = "default_interval")]
        interval_seconds: u64,
    },
    Tcp {
        port: u16,
    },
    Process,
}

fn default_interval() -> u64 {
    60
}

/// Extension module runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionRuntime {
    /// Path to the binary (relative to module install dir).
    pub binary: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    pub protocol: ExtensionProtocol,
    pub health_check: Option<HealthCheck>,
}

/// The module manifest — parsed from `openclaw.module.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    /// Unique module identifier (e.g. "home-assistant", "social-manager").
    pub id: String,
    /// Human-readable name (e.g. "Home Assistant Integration").
    pub name: String,
    /// Semantic version.
    pub version: semver::Version,
    /// Short description.
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub license: Option<String>,
    /// Core (compiled in) or Extension (separate binary).
    pub tier: ModuleTier,
    /// Minimum gateway version required.
    pub min_gateway_version: Option<semver::VersionReq>,

    /// Tools this module provides (declarative, for listing even when
    /// the module is not running).
    #[serde(default)]
    pub tools: Vec<ToolDeclaration>,

    /// Configuration schema (JSON Schema). The module manager validates
    /// user config against this before passing it to the module.
    pub config_schema: Option<Value>,

    /// Module dependencies (other module IDs that must be present).
    #[serde(default)]
    pub dependencies: Vec<String>,

    /// Extension runtime config (only for tier = "extension").
    pub runtime: Option<ExtensionRuntime>,
}

impl ModuleManifest {
    /// Parse a manifest from TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_str)
    }

    /// Parse a manifest from a TOML file path.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_toml(&content)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_core_manifest() {
        let toml = r#"
            id = "home-assistant"
            name = "Home Assistant Integration"
            version = "0.1.0"
            description = "Control lights, thermostats, scenes, and media via Home Assistant"
            authors = ["Sean"]
            license = "MIT"
            tier = "core"

            [[tools]]
            name = "control_light"
            description = "Turn lights on/off and set brightness"

            [[tools]]
            name = "set_thermostat"
            description = "Set thermostat temperature"
            requires_approval = true
        "#;

        let manifest = ModuleManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.id, "home-assistant");
        assert_eq!(manifest.tier, ModuleTier::Core);
        assert_eq!(manifest.tools.len(), 2);
        assert!(!manifest.tools[0].requires_approval);
        assert!(manifest.tools[1].requires_approval);
    }

    #[test]
    fn parse_extension_manifest() {
        let toml = r#"
            id = "social-manager"
            name = "Social Media Manager"
            version = "0.1.0"
            description = "Post to Bluesky, Threads, and YouTube"
            tier = "extension"

            [runtime]
            binary = "bin/rust-social-manager"
            args = ["--mcp-server"]

            [runtime.protocol]
            type = "mcp"

            [runtime.health_check]
            type = "process"

            [[tools]]
            name = "bsky_post"
            description = "Create a Bluesky post"
            requires_approval = true
        "#;

        let manifest = ModuleManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.id, "social-manager");
        assert_eq!(manifest.tier, ModuleTier::Extension);
        assert!(manifest.runtime.is_some());
        let rt = manifest.runtime.unwrap();
        assert_eq!(rt.binary, "bin/rust-social-manager");
        assert!(matches!(rt.protocol, ExtensionProtocol::Mcp));
    }
}
