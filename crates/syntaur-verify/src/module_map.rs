//! Module impact map — maps source file paths to user-facing modules.
//!
//! Authored at `crates/syntaur-verify/module-map.yaml` in the repo
//! (so the map evolves with the gateway). Special module id `*`
//! means "every module" — used for theme + shared + route wiring +
//! dep-file changes that cascade everywhere.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A user-facing module — one URL that gets independently verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    /// Stable identifier (kebab-case). e.g. `dashboard`, `smart-home`.
    pub slug: String,
    /// URL path the module renders at. e.g. `/dashboard`.
    pub url: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: String,
}

/// A rule saying "if this path changed, re-verify these modules."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Path relative to workspace root. Exact match for now;
    /// Phase 3+ will add glob support when it starts mattering.
    pub path: String,
    /// Module slugs (or `"*"` for every module).
    pub affects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleMap {
    pub version: u32,
    pub modules: Vec<Module>,
    pub rules: Vec<Rule>,
}

impl ModuleMap {
    pub fn load(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading module map {}", path.display()))?;
        let m: ModuleMap =
            serde_yaml::from_str(&body).context("parsing module map YAML")?;
        if m.version != 1 {
            anyhow::bail!(
                "module map version {} unsupported (this tool reads v1)",
                m.version
            );
        }
        Ok(m)
    }

    pub fn module(&self, slug: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.slug == slug)
    }

    /// For a given set of changed paths, compute the module slugs
    /// that need re-verification. If any rule matches with `*`, all
    /// modules are returned.
    pub fn impacted_by(&self, changed_paths: &[String]) -> BTreeSet<String> {
        let mut affected: BTreeSet<String> = BTreeSet::new();
        let mut universe = false;

        let rule_index: BTreeMap<&str, &Rule> =
            self.rules.iter().map(|r| (r.path.as_str(), r)).collect();

        for p in changed_paths {
            if let Some(r) = rule_index.get(p.as_str()) {
                if r.affects.iter().any(|a| a == "*") {
                    universe = true;
                } else {
                    for m in &r.affects {
                        affected.insert(m.clone());
                    }
                }
            }
        }

        if universe {
            return self.modules.iter().map(|m| m.slug.clone()).collect();
        }
        affected
    }
}
