//! `ocmod` — OpenClaw module manager CLI.
//!
//! Manages core and extension modules: list, install, remove, enable, disable.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use openclaw_sdk::manifest::{ModuleManifest, ModuleTier};

// ── Core module definitions (must match gateway's modules.rs) ───────────

struct CoreModuleDef {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    tool_count: usize,
}

const CORE_MODULES: &[CoreModuleDef] = &[
    CoreModuleDef { id: "core-files", name: "File Operations", description: "Read, write, edit, list files", tool_count: 9 },
    CoreModuleDef { id: "core-shell", name: "Shell & Code", description: "Execute commands and sandboxed code", tool_count: 4 },
    CoreModuleDef { id: "core-web", name: "Web & Search", description: "Web search, fetch, JSON query", tool_count: 4 },
    CoreModuleDef { id: "core-telegram", name: "Telegram", description: "Send Telegram messages", tool_count: 1 },
    CoreModuleDef { id: "mod-comms", name: "Communications", description: "Email and SMS", tool_count: 5 },
    CoreModuleDef { id: "mod-captcha", name: "CAPTCHA Solving", description: "2Captcha and browser bridge", tool_count: 5 },
    CoreModuleDef { id: "mod-office", name: "Office Documents", description: "Excel, Word, PowerPoint", tool_count: 7 },
    CoreModuleDef { id: "mod-accounts", name: "Account Management", description: "Social account creation, OAuth", tool_count: 8 },
    CoreModuleDef { id: "mod-browser", name: "Browser Automation", description: "Chromium DevTools Protocol", tool_count: 16 },
];

// ── Config types ────────────────────────────────────────────────────────

#[derive(serde::Deserialize, serde::Serialize, Default)]
struct OpenClawConfig {
    #[serde(default)]
    modules: ModulesConfig,
    #[serde(flatten)]
    other: serde_json::Value,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
struct ModulesConfig {
    #[serde(default)]
    entries: HashMap<String, ModuleEntry>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
struct ModuleEntry {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    config: serde_json::Value,
}

fn default_true() -> bool { true }

// ── Paths ───────────────────────────────────────────────────────────────

fn openclaw_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    PathBuf::from(home).join(".openclaw")
}

fn config_path() -> PathBuf {
    openclaw_dir().join("openclaw.json")
}

fn modules_dir() -> PathBuf {
    openclaw_dir().join("modules")
}

fn load_config() -> Result<OpenClawConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(OpenClawConfig::default());
    }
    let text = fs::read_to_string(&path).context("reading openclaw.json")?;
    serde_json::from_str(&text).context("parsing openclaw.json")
}

fn save_config(config: &OpenClawConfig) -> Result<()> {
    let path = config_path();
    let text = serde_json::to_string_pretty(config).context("serializing config")?;
    fs::write(&path, text).context("writing openclaw.json")?;
    Ok(())
}

// ── Extension module discovery ──────────────────────────────────────────

struct ExtensionModule {
    manifest: ModuleManifest,
    path: PathBuf,
}

fn discover_extensions() -> Vec<ExtensionModule> {
    let dir = modules_dir();
    let mut modules = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("openclaw.module.toml");
            if let Ok(manifest) = ModuleManifest::from_file(&manifest_path) {
                modules.push(ExtensionModule {
                    manifest,
                    path: entry.path(),
                });
            }
        }
    }
    modules.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    modules
}

// ── Commands ────────────────────────────────────────────────────────────

fn cmd_list() -> Result<()> {
    let config = load_config()?;
    let extensions = discover_extensions();

    println!("Core Modules:");
    println!("{:<20} {:<25} {:<8} {}", "ID", "NAME", "TOOLS", "STATUS");
    println!("{}", "-".repeat(70));
    for m in CORE_MODULES {
        let enabled = config.modules.entries.get(m.id)
            .map(|e| e.enabled)
            .unwrap_or(true);
        let status = if enabled { "\x1b[32menabled\x1b[0m" } else { "\x1b[31mdisabled\x1b[0m" };
        println!("{:<20} {:<25} {:<8} {}", m.id, m.name, m.tool_count, status);
    }

    if !extensions.is_empty() {
        println!("\nExtension Modules:");
        println!("{:<20} {:<25} {:<8} {:<10} {}", "ID", "NAME", "TOOLS", "VERSION", "STATUS");
        println!("{}", "-".repeat(80));
        for ext in &extensions {
            let m = &ext.manifest;
            let enabled = config.modules.entries.get(&m.id)
                .map(|e| e.enabled)
                .unwrap_or(true);
            let status = if enabled { "\x1b[32menabled\x1b[0m" } else { "\x1b[31mdisabled\x1b[0m" };
            println!("{:<20} {:<25} {:<8} {:<10} {}",
                m.id, m.name, m.tools.len(), m.version, status);
        }
    }

    let total_core: usize = CORE_MODULES.iter().map(|m| m.tool_count).sum();
    let total_ext: usize = extensions.iter().map(|e| e.manifest.tools.len()).sum();
    println!("\nTotal: {} core modules ({} tools), {} extensions ({} tools)",
        CORE_MODULES.len(), total_core, extensions.len(), total_ext);
    Ok(())
}

fn cmd_status() -> Result<()> {
    // Check if gateway is running
    let health: Result<String, _> = reqwest_blocking_get("http://127.0.0.1:18789/health");
    match health {
        Ok(body) => {
            println!("Gateway: \x1b[32mrunning\x1b[0m");
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(uptime) = v.get("uptime_secs").and_then(|u| u.as_u64()) {
                    println!("Uptime: {}m {}s", uptime / 60, uptime % 60);
                }
            }
        }
        Err(_) => {
            println!("Gateway: \x1b[31mnot running\x1b[0m");
        }
    }
    println!();
    cmd_list()
}

fn cmd_enable(id: &str) -> Result<()> {
    let mut config = load_config()?;
    config.modules.entries.insert(id.to_string(), ModuleEntry {
        enabled: true,
        config: config.modules.entries.get(id)
            .map(|e| e.config.clone())
            .unwrap_or(serde_json::Value::Null),
    });
    save_config(&config)?;
    println!("Enabled module '{}'. Restart gateway to apply.", id);
    Ok(())
}

fn cmd_disable(id: &str) -> Result<()> {
    let mut config = load_config()?;
    config.modules.entries.insert(id.to_string(), ModuleEntry {
        enabled: false,
        config: config.modules.entries.get(id)
            .map(|e| e.config.clone())
            .unwrap_or(serde_json::Value::Null),
    });
    save_config(&config)?;
    println!("Disabled module '{}'. Restart gateway to apply.", id);
    Ok(())
}

fn cmd_install(source: &str) -> Result<()> {
    let source_path = Path::new(source);

    if source_path.is_dir() {
        // Install from directory — copy manifest + bin
        let manifest_path = source_path.join("openclaw.module.toml");
        if !manifest_path.exists() {
            bail!("No openclaw.module.toml found in {}", source);
        }
        let manifest = ModuleManifest::from_file(&manifest_path)
            .context("parsing manifest")?;

        let dest = modules_dir().join(&manifest.id);
        if dest.exists() {
            bail!("Module '{}' already installed at {}", manifest.id, dest.display());
        }

        // Copy the directory
        copy_dir_recursive(source_path, &dest)?;
        println!("Installed '{}' v{} ({} tools)",
            manifest.id, manifest.version, manifest.tools.len());
    } else if source_path.exists() && source_path.extension().map_or(false, |e| e == "zst" || e == "tar") {
        // TODO: extract .tar.zst archive
        bail!("Archive installation not yet implemented. Use a directory for now.");
    } else {
        bail!("Source not found: {}", source);
    }
    Ok(())
}

fn cmd_remove(id: &str) -> Result<()> {
    let mod_dir = modules_dir().join(id);
    if !mod_dir.exists() {
        bail!("Module '{}' not found at {}", id, mod_dir.display());
    }

    // Check it's not a core module
    if CORE_MODULES.iter().any(|m| m.id == id) {
        bail!("Cannot remove core module '{}'. Use 'ocmod disable {}' instead.", id, id);
    }

    fs::remove_dir_all(&mod_dir).context("removing module directory")?;

    // Also remove from config
    let mut config = load_config()?;
    config.modules.entries.remove(id);
    save_config(&config)?;

    println!("Removed module '{}'. Restart gateway to apply.", id);
    Ok(())
}

fn cmd_info(id: &str) -> Result<()> {
    // Check core modules first
    for m in CORE_MODULES {
        if m.id == id {
            println!("Module: {} (core)", m.name);
            println!("ID: {}", m.id);
            println!("Description: {}", m.description);
            println!("Tools: {}", m.tool_count);
            println!("Tier: core (compiled into gateway)");
            let config = load_config()?;
            let enabled = config.modules.entries.get(m.id)
                .map(|e| e.enabled).unwrap_or(true);
            println!("Status: {}", if enabled { "enabled" } else { "disabled" });
            return Ok(());
        }
    }

    // Check extensions
    let extensions = discover_extensions();
    for ext in &extensions {
        if ext.manifest.id == id {
            let m = &ext.manifest;
            println!("Module: {} (extension)", m.name);
            println!("ID: {}", m.id);
            println!("Version: {}", m.version);
            println!("Description: {}", m.description);
            if !m.authors.is_empty() {
                println!("Authors: {}", m.authors.join(", "));
            }
            if let Some(lic) = &m.license {
                println!("License: {}", lic);
            }
            println!("Tier: extension (MCP server)");
            println!("Path: {}", ext.path.display());
            if let Some(rt) = &m.runtime {
                println!("Binary: {}", rt.binary);
            }
            println!("Tools ({}):", m.tools.len());
            for t in &m.tools {
                let approval = if t.requires_approval { " [approval]" } else { "" };
                println!("  - {}: {}{}", t.name, t.description, approval);
            }
            let config = load_config()?;
            let enabled = config.modules.entries.get(&m.id)
                .map(|e| e.enabled).unwrap_or(true);
            println!("Status: {}", if enabled { "enabled" } else { "disabled" });
            return Ok(());
        }
    }

    bail!("Module '{}' not found", id);
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn reqwest_blocking_get(url: &str) -> Result<String, String> {
    // Minimal HTTP GET without pulling in reqwest
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    let url = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{}", path);

    let mut stream = TcpStream::connect_timeout(
        &host_port.parse().map_err(|e| format!("{}", e))?,
        Duration::from_secs(2),
    ).map_err(|e| format!("{}", e))?;

    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();

    let request = format!("GET {} HTTP/1.0\r\nHost: {}\r\n\r\n", path, host_port);
    std::io::Write::write_all(&mut stream, request.as_bytes())
        .map_err(|e| format!("{}", e))?;

    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|e| format!("{}", e))?;

    // Extract body after \r\n\r\n
    if let Some(pos) = response.find("\r\n\r\n") {
        Ok(response[pos + 4..].to_string())
    } else {
        Err("No HTTP body".to_string())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");

    let result = match cmd {
        "list" | "ls" => cmd_list(),
        "status" | "st" => cmd_status(),
        "enable" => {
            let id = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ocmod enable <module-id>");
                std::process::exit(1);
            });
            cmd_enable(id)
        }
        "disable" => {
            let id = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ocmod disable <module-id>");
                std::process::exit(1);
            });
            cmd_disable(id)
        }
        "install" => {
            let source = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ocmod install <path>");
                std::process::exit(1);
            });
            cmd_install(source)
        }
        "remove" | "rm" => {
            let id = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ocmod remove <module-id>");
                std::process::exit(1);
            });
            cmd_remove(id)
        }
        "info" => {
            let id = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ocmod info <module-id>");
                std::process::exit(1);
            });
            cmd_info(id)
        }
        "help" | "--help" | "-h" => {
            println!("ocmod — OpenClaw module manager\n");
            println!("Usage: ocmod <command> [args]\n");
            println!("Commands:");
            println!("  list, ls              List all modules with status");
            println!("  status, st            Gateway status + module list");
            println!("  info <id>             Show module details and tools");
            println!("  enable <id>           Enable a module");
            println!("  disable <id>          Disable a module");
            println!("  install <path>        Install an extension module");
            println!("  remove, rm <id>       Remove an extension module");
            println!("  help                  Show this help");
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {}\nRun 'ocmod help' for usage.", other);
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
