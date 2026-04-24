//! Phase 4b — persona POV catalog.
//!
//! Each Syntaur persona (Peter, Silvr, Thaddeus, Mushi, Nyota, Cortex,
//! Positron, Maurice, Kyron) owns a distinct module, a distinct
//! workspace state, and a distinct avatar in the shared topbar. A
//! regression that only manifests when the signed-in user IS that
//! persona (e.g. Silvr's /music recommendations panel never loading)
//! would slip past an anonymous-only verify sweep. This module lets
//! the CLI loop each persona through the same capture + diff pipeline
//! so the Finding carries the persona tag and baselines can be keyed
//! three-dimensionally: `(module, persona, viewport)`.
//!
//! IMPORTANT — scope boundary: this is NOT about rendering the *agent
//! chat* under each persona (that's the agents crate's work). It's
//! about the visual + interaction differences when the signed-in
//! USER is persona X and the gateway swaps in their session state.
//!
//! Session bootstrap — ONE of:
//!   * `auth_token_env`: name of an env var holding a long-lived API
//!     token for that persona's user. Implemented here; this is the
//!     path the first 4b run uses.
//!   * `login_flow`: path to a Phase 4 flow YAML that logs the
//!     persona in via the UI. **Punted** — TODO below in
//!     `Persona::auth_token` + at the `login_flow` catalog field. Only
//!     the `auth_token_env` path is wired up in this phase.
//!
//! The catalog is a plain YAML file so ops can add a new persona
//! entry without recompiling. Missing / unreadable env vars are a
//! WARN (persona is SKIPPED) — the CLI doesn't hard-fail the run,
//! because "I only have one persona's token handy, sweep what you
//! can" is a common ops posture when triaging a single-persona bug.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One persona entry in the catalog YAML.
///
/// Deserialises from:
///
/// ```yaml
/// - slug: peter
///   display_name: Peter
///   default_module: dashboard
///   auth_token_env: SYNTAUR_VERIFY_PERSONA_PETER_TOKEN
///   # primer_flow: primers/peter-default.yaml   # optional
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    /// Stable slug — used in Finding.persona, baseline path segment,
    /// corpus archive dir name. Must be `[a-z0-9-]`. The CLI does not
    /// sanitise it — the catalog author is trusted to keep it clean.
    pub slug: String,
    /// Human-facing name for log lines + Finding detail strings.
    pub display_name: String,
    /// The module slug this persona "lives in" — surfaced in log
    /// messages so the verify run makes the POV obvious even when
    /// you're sweeping `--all-personas --module dashboard` and every
    /// persona is being shown the same page.
    ///
    /// Not used for any routing decision in 4b: the CLI visits
    /// whatever `--module` the caller passed for every persona. A
    /// future phase may flip to "persona's default module only"
    /// as an opt-in optimisation.
    pub default_module: String,

    /// Env var holding a bearer token for this persona's session.
    /// When set AND the env var is present, the CLI injects the
    /// token via `Browser::with_auth_token`. When either is missing,
    /// the persona is SKIPPED with a warn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token_env: Option<String>,

    /// TODO(phase-4c): path to a Phase 4 flow YAML that logs the
    /// persona in via the UI. Reserved here so the catalog schema
    /// doesn't break when the flow-based login path lands. Callers
    /// setting this today will get a warn + persona-skipped — see
    /// `Persona::auth_token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_flow: Option<PathBuf>,

    /// Optional "do this before screenshot" flow. Runs against the
    /// persona's session to seed fixtures (add a todo, play a song,
    /// pin a widget) so the screenshot captures their real
    /// workspace state rather than the empty-state.
    ///
    /// Wiring into the CLI loop is punted alongside `login_flow` —
    /// the catalog carries it forward so ops can author primer YAMLs
    /// today and have them execute when Phase 4c lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primer_flow: Option<PathBuf>,
}

/// How `Persona::auth_token` resolved its session.
///
/// `Debug` is manually implemented below so the bearer token never
/// lands in a panic message or error chain — `AuthSource::Env` prints
/// as `Env { var, token: <redacted> }`.
#[derive(Clone)]
pub enum AuthSource {
    /// Token came from the env var named in `auth_token_env`.
    Env { var: String, token: String },
    /// Catalog says "log in via this flow" — not yet wired up.
    /// Callers treat this as a skip for now.
    FlowPunted { flow: PathBuf },
    /// No `auth_token_env` set AND no `login_flow` set — this
    /// persona is pure-catalog and the caller should skip it.
    NoneConfigured,
    /// `auth_token_env` set but the env var is empty / unset. The
    /// caller logs the var name in the skip warn so ops can see
    /// exactly what's missing.
    EnvMissing { var: String },
}

impl Persona {
    /// Resolve the session bootstrap into a concrete bearer token.
    ///
    /// Returns:
    ///   * `Ok(AuthSource::Env { token, .. })` — use this token.
    ///   * `Ok(AuthSource::FlowPunted { .. })`, `EnvMissing { .. }`,
    ///     or `NoneConfigured` — caller should SKIP this persona
    ///     with a warn. These are NOT errors: the run continues,
    ///     it just doesn't cover this persona.
    ///   * `Err(_)` — reserved for genuinely unexpected failures;
    ///     currently unreachable.
    pub fn auth_token(&self) -> Result<AuthSource> {
        if let Some(var) = &self.auth_token_env {
            match std::env::var(var) {
                Ok(v) if !v.trim().is_empty() => {
                    return Ok(AuthSource::Env {
                        var: var.clone(),
                        token: v,
                    });
                }
                _ => {
                    return Ok(AuthSource::EnvMissing { var: var.clone() });
                }
            }
        }
        if let Some(flow) = &self.login_flow {
            // TODO(phase-4c): drive this flow through the Phase 4
            // runner against the target URL, capture the token the
            // UI stashes in sessionStorage, and return it as
            // AuthSource::Env. For now we surface the intent so the
            // caller can log "skipping persona, flow path not yet
            // supported" without silently dropping the entry.
            return Ok(AuthSource::FlowPunted { flow: flow.clone() });
        }
        Ok(AuthSource::NoneConfigured)
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn default_module(&self) -> &str {
        &self.default_module
    }
}

/// A parsed personas catalog, preserving file order so the CLI log
/// output is deterministic.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonaCatalog {
    personas: Vec<Persona>,
}

impl PersonaCatalog {
    /// Load + parse a YAML catalog. The root may be either a bare
    /// sequence of persona maps or an object with a `personas:` key
    /// — both forms are accepted so ops can pick the shape they
    /// prefer without triggering a parse error.
    pub fn load(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading personas catalog {}", path.display()))?;
        Self::parse_str(&body).with_context(|| format!("parsing {}", path.display()))
    }

    /// Parse a catalog from a string. Exposed for tests + future
    /// callers that want to embed a default catalog without touching
    /// disk. Never writes.
    pub fn parse_str(body: &str) -> Result<Self> {
        // Try the object form first (`{ personas: [...] }`) because
        // the bare-list form is also a valid sub-shape of it in
        // serde_yaml's view — so we accept either by trying the
        // more-structured form first and falling back.
        if let Ok(wrapped) = serde_yaml::from_str::<PersonaCatalog>(body) {
            if !wrapped.personas.is_empty() {
                return Ok(wrapped);
            }
        }
        let bare: Vec<Persona> = serde_yaml::from_str(body)
            .context("catalog YAML must be a sequence of persona entries or { personas: [...] }")?;
        Ok(Self { personas: bare })
    }

    /// Iterate in catalog order.
    pub fn all(&self) -> &[Persona] {
        &self.personas
    }

    /// Find a persona by slug. Returns `None` if not in the catalog —
    /// the CLI surfaces this as `unknown persona X` with the full
    /// slug list.
    pub fn get(&self, slug: &str) -> Option<&Persona> {
        self.personas.iter().find(|p| p.slug == slug)
    }

    /// Slugs in catalog order, for error messages.
    pub fn slugs(&self) -> Vec<String> {
        self.personas.iter().map(|p| p.slug.clone()).collect()
    }

    /// Filter the catalog down to the requested slugs, preserving
    /// catalog order. Returns `Err` listing any unknown slugs so the
    /// CLI can print them all at once rather than one-at-a-time.
    pub fn select(&self, requested: &[String]) -> Result<Vec<&Persona>> {
        let mut missing: Vec<&String> = Vec::new();
        for slug in requested {
            if self.get(slug).is_none() {
                missing.push(slug);
            }
        }
        if !missing.is_empty() {
            anyhow::bail!(
                "unknown persona slug(s): {} — catalog has: {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                self.slugs().join(", ")
            );
        }
        // Keep the catalog-file order even when the user passed slugs
        // out of order — predictable log output trumps CLI order.
        Ok(self
            .personas
            .iter()
            .filter(|p| requested.iter().any(|r| r == &p.slug))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> &'static str {
        r#"
- slug: peter
  display_name: Peter
  default_module: dashboard
  auth_token_env: SYNTAUR_VERIFY_PERSONA_PETER_TOKEN
- slug: silvr
  display_name: Silvr
  default_module: music
  auth_token_env: SYNTAUR_VERIFY_PERSONA_SILVR_TOKEN
- slug: mushi
  display_name: Mushi
  default_module: journal
  login_flow: primers/mushi-login.yaml
"#
    }

    #[test]
    fn catalog_loads_bare_sequence_form() {
        let cat = PersonaCatalog::parse_str(sample_catalog()).expect("parse");
        assert_eq!(cat.all().len(), 3);
        assert_eq!(cat.slugs(), vec!["peter", "silvr", "mushi"]);
        assert_eq!(cat.get("silvr").unwrap().display_name, "Silvr");
        assert_eq!(cat.get("silvr").unwrap().default_module, "music");
    }

    #[test]
    fn catalog_loads_wrapped_object_form() {
        let body = r#"
personas:
  - slug: peter
    display_name: Peter
    default_module: dashboard
    auth_token_env: X
"#;
        let cat = PersonaCatalog::parse_str(body).expect("parse");
        assert_eq!(cat.all().len(), 1);
        assert_eq!(cat.get("peter").unwrap().slug, "peter");
    }

    #[test]
    fn persona_skips_when_env_var_unset_or_empty() {
        // Use a deliberately unlikely var name so we don't collide
        // with anything the ambient test env might have set.
        let p = Persona {
            slug: "peter".into(),
            display_name: "Peter".into(),
            default_module: "dashboard".into(),
            auth_token_env: Some(
                "SYNTAUR_VERIFY_PERSONA_TEST_DEFINITELY_UNSET_ZZZZZZ".into(),
            ),
            login_flow: None,
            primer_flow: None,
        };
        // Make doubly sure the var is unset even if a stray export is
        // hanging around in the developer shell.
        // SAFETY: env access is process-global but test only touches a
        // one-off var name; no other test relies on it.
        unsafe {
            std::env::remove_var(
                "SYNTAUR_VERIFY_PERSONA_TEST_DEFINITELY_UNSET_ZZZZZZ",
            );
        }

        match p.auth_token().expect("resolve") {
            AuthSource::EnvMissing { var } => {
                assert_eq!(var, "SYNTAUR_VERIFY_PERSONA_TEST_DEFINITELY_UNSET_ZZZZZZ");
            }
            other => panic!("expected EnvMissing, got {other:?}"),
        }
    }

    #[test]
    fn persona_with_only_login_flow_returns_flow_punted() {
        // Flow-based login isn't implemented yet (TODO in auth_token).
        // Personas that only declare login_flow must surface as
        // FlowPunted so the CLI can SKIP them with a clear message
        // instead of silently dropping coverage.
        let p = Persona {
            slug: "mushi".into(),
            display_name: "Mushi".into(),
            default_module: "journal".into(),
            auth_token_env: None,
            login_flow: Some(PathBuf::from("primers/mushi-login.yaml")),
            primer_flow: None,
        };
        match p.auth_token().expect("resolve") {
            AuthSource::FlowPunted { flow } => {
                assert_eq!(flow, PathBuf::from("primers/mushi-login.yaml"));
            }
            other => panic!("expected FlowPunted, got {other:?}"),
        }
    }

    #[test]
    fn select_preserves_catalog_order_and_errors_on_unknown() {
        let cat = PersonaCatalog::parse_str(sample_catalog()).expect("parse");

        // Out-of-order request — catalog order must win.
        let got = cat
            .select(&vec!["silvr".to_string(), "peter".to_string()])
            .expect("select");
        let slugs: Vec<&str> = got.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, vec!["peter", "silvr"]);

        // Unknown slug — error mentions it.
        let err = cat
            .select(&vec!["peter".to_string(), "ghost".to_string()])
            .expect_err("should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("ghost"), "err didn't mention ghost: {msg}");
    }
}

// Tokens-from-AuthSource need a Debug impl for the test above.
impl std::fmt::Debug for AuthSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthSource::Env { var, .. } => write!(f, "Env {{ var: {var:?}, token: <redacted> }}"),
            AuthSource::FlowPunted { flow } => write!(f, "FlowPunted {{ flow: {flow:?} }}"),
            AuthSource::NoneConfigured => write!(f, "NoneConfigured"),
            AuthSource::EnvMissing { var } => write!(f, "EnvMissing {{ var: {var:?} }}"),
        }
    }
}
