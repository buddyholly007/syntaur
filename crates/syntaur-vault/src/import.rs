//! One-shot migration: scan CLAUDE.md-style files for known secret
//! patterns + bulk-insert into the vault.
//!
//! Curated-rule-table approach instead of blind "any long-looking
//! string" scan — avoids capturing incidental hex hashes, git SHAs,
//! etc. Each rule knows its vault entry name, description, tags, and
//! the regex that extracts the secret value from a context line.
//!
//! Sean's CLAUDE.md files (on both gaming PC and claudevm) wrote
//! secrets with predictable markdown shapes:
//!
//!   OpenRouter API key: `sk-or-v1-....`
//!   **Claude bot** ... token `1234567890:AAGU...`
//!   Sean's chat ID: `8643081032`
//!   **Bridge token**: `...`

use std::fs;
use std::path::Path;

use anyhow::Result;
use regex::Regex;

pub struct Rule {
    /// Vault entry name (key).
    pub name: &'static str,
    pub description: &'static str,
    pub tags: &'static [&'static str],
    /// Regex with ONE capture group for the secret value. Case
    /// sensitive. First match wins — rules are ordered most-specific
    /// to least.
    pub regex: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule {
        name: "openrouter",
        description: "OpenRouter API key (free tier, 1000 req/day with $10+ credits)",
        tags: &["api", "llm"],
        // Typical: `OpenRouter API key: `sk-or-v1-....``
        regex: r"OpenRouter API key[^`]*`(sk-or-v1-[a-zA-Z0-9_\-]{20,})`",
    },
    Rule {
        name: "telegram.claude_bot",
        description: "@ClaudeGamingPC_bot — default notification bot for Claude",
        tags: &["telegram", "bot"],
        // Lines like: **Claude bot** (default ...) ... token `12345:AAG...`
        // Lazy `.*?` lets the pattern cross the backtick-wrapped
        // `@handle` and land on the `token ...` backtick that
        // actually contains the secret.
        regex: r"(?i)Claude bot\b.*?token\s*`(\d{8,12}:[A-Za-z0-9_\-]{30,})`",
    },
    Rule {
        name: "telegram.felix_bot",
        description: "@FelixOpenclaw83_bot — Felix/OpenClaw agent chat",
        tags: &["telegram", "bot"],
        regex: r"(?i)Felix.*?bot\b.*?token\s*`(\d{8,12}:[A-Za-z0-9_\-]{30,})`",
    },
    Rule {
        name: "telegram.taxreceipt_bot",
        description: "@TaxReceipt_bot — tax/receipt pipeline",
        tags: &["telegram", "bot", "tax"],
        regex: r"(?i)TaxReceipt\s*bot\b.*?token\s*`([\d:A-Za-z0-9_\-]{6,})`",
    },
    Rule {
        name: "telegram.chat_id",
        description: "Sean's Telegram chat_id for all personal notifications",
        tags: &["telegram"],
        regex: r"(?i)Sean'?s chat ID[^`]*`(\d{6,12})`",
    },
    Rule {
        name: "telegram.bridge_token",
        description: "rust-telegram-gateway bridge token (claudevm:19877)",
        tags: &["telegram", "infrastructure"],
        regex: r"(?i)Bridge token[^`]*`([a-f0-9]{32,})`",
    },
    Rule {
        name: "ssh.gaming_pc_fallback_password",
        description: "SSH fallback password for sean@gaming-pc (key auth preferred)",
        tags: &["ssh", "fallback"],
        regex: r"(?i)Password is `(\d{3,8})`",
    },
];

pub struct Finding {
    pub rule_name: &'static str,
    pub description: &'static str,
    pub tags: Vec<String>,
    pub value: String,
    pub source_file: String,
    pub line_no: usize,
    pub context: String,
}

/// Run all rules against the contents of every file. Dedupes by
/// `rule_name`: first hit wins.
pub fn scan_files(paths: &[&Path]) -> Result<Vec<Finding>> {
    let compiled: Vec<(&Rule, Regex)> = RULES
        .iter()
        .map(|r| Regex::new(r.regex).map(|rx| (r, rx)))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut out: Vec<Finding> = Vec::new();
    let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();

    for path in paths {
        let body = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[import] skipping {}: {e}", path.display());
                continue;
            }
        };
        for (line_no, line) in body.lines().enumerate() {
            for (rule, rx) in &compiled {
                if seen.contains(rule.name) {
                    continue;
                }
                if let Some(cap) = rx.captures(line) {
                    if let Some(m) = cap.get(1) {
                        out.push(Finding {
                            rule_name: rule.name,
                            description: rule.description,
                            tags: rule.tags.iter().map(|s| s.to_string()).collect(),
                            value: m.as_str().to_string(),
                            source_file: path.display().to_string(),
                            line_no: line_no + 1,
                            context: line.trim().to_string(),
                        });
                        seen.insert(rule.name);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Shortened value for console display — shows first 6 + last 4 so
/// Sean can eyeball it's the right one without printing the full
/// secret to stdout/terminal history.
pub fn redact(value: &str) -> String {
    let n = value.chars().count();
    if n <= 10 {
        format!("{} chars", n)
    } else {
        let head: String = value.chars().take(6).collect();
        let tail: String = value.chars().skip(n - 4).collect();
        format!("{head}…{tail} ({n} chars)")
    }
}
