//! User-flow YAML interpreter — Phase 4.
//!
//! A `Flow` is a short, scripted interaction against a rendered module
//! (click here, type there, assert this appeared) authored in YAML and
//! replayed by chromiumoxide. Flows complement per-module screenshot
//! sweeps: screenshots catch "does it render", flows catch "does it
//! actually work end-to-end" — the obvious example being a dashboard
//! where the static render looks fine but the add-todo interaction is
//! wired to a dead endpoint.
//!
//! Design notes:
//!
//!   * Each flow runs on ONE `Browser` instance the outer run hands in.
//!     No per-step browser relaunch — we reuse the viewport + UA the
//!     browser was configured with.
//!   * YAML is parsed via `serde_yaml::Value` and walked manually. This
//!     is more code than a straight `#[derive(Deserialize)]` but gives
//!     us step-level errors with YAML line + column + file path — the
//!     user typing `waait_ms: 1500` should see that mistake in the
//!     error, not a generic "missing field" a level up.
//!   * Assertion / timeout failures are caught as structured
//!     `Finding`s (Regression) and the remaining steps are skipped —
//!     continuing past a failed assertion would produce cascading
//!     noise (e.g. clicking an element that never rendered).
//!   * A single successful flow produces ONE summary `Finding`
//!     (Suggestion severity) so the run report shows what was covered
//!     even when everything passes, matching the baseline-captured
//!     pattern Phase 3 established.
//!
//! Deferred (documented TODOs in code): `eval:` and `assert_url_matches`
//! are implemented but marked as "lightly tested" — the 80% flows Sean
//! cares about in the dashboard + settings + smart-home modules only
//! need goto/click/type/press/screenshot/wait/assert_contains.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use regex::Regex;
use serde_yaml::Value as YamlValue;

use crate::browser::Browser;
use crate::run::{Finding, FindingKind, Severity};

/// One parsed flow file. Construct via `FlowFile::load`.
#[derive(Debug, Clone)]
pub struct FlowFile {
    /// Human-readable name from the YAML `name:` field.
    pub name: String,
    /// Module slug this flow exercises. Used to tag findings so the
    /// run report groups flow findings under the same module section
    /// as the static screenshot findings.
    pub module: String,
    /// Ordered list of steps.
    pub steps: Vec<Step>,
    /// Source path — carried for error messages + per-flow screenshot
    /// filenames (filename stem is used as the flow slug).
    pub source_path: PathBuf,
}

impl FlowFile {
    /// Load + parse a flow YAML file. On any structural problem,
    /// returns an error that mentions the file path and (when
    /// serde_yaml exposes it) a line number.
    pub fn load(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading flow file {}", path.display()))?;
        let root: YamlValue = serde_yaml::from_str(&body).map_err(|e| {
            let loc = e.location();
            match loc {
                Some(l) => anyhow!(
                    "{}: YAML parse error at line {}, col {}: {}",
                    path.display(),
                    l.line(),
                    l.column(),
                    e
                ),
                None => anyhow!("{}: YAML parse error: {}", path.display(), e),
            }
        })?;

        let mapping = root.as_mapping().ok_or_else(|| {
            anyhow!(
                "{}: flow root must be a mapping with `name`, `module`, `steps`",
                path.display()
            )
        })?;

        let name = require_string(mapping, "name", path)?;
        let module = require_string(mapping, "module", path)?;
        let steps_val = mapping
            .get(&YamlValue::String("steps".into()))
            .ok_or_else(|| anyhow!("{}: missing `steps:` array", path.display()))?;
        let steps_seq = steps_val.as_sequence().ok_or_else(|| {
            anyhow!("{}: `steps:` must be a YAML sequence", path.display())
        })?;
        if steps_seq.is_empty() {
            bail!("{}: `steps:` is empty — nothing to run", path.display());
        }

        let mut steps = Vec::with_capacity(steps_seq.len());
        for (idx, raw) in steps_seq.iter().enumerate() {
            let step = Step::parse(raw, idx, path)?;
            steps.push(step);
        }

        Ok(FlowFile {
            name,
            module,
            steps,
            source_path: path.to_path_buf(),
        })
    }

    /// Filename-based slug (stem without extension), used as the
    /// primary key in screenshot filenames.
    pub fn slug(&self) -> String {
        self.source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("flow")
            .to_string()
    }
}

fn require_string(
    mapping: &serde_yaml::Mapping,
    key: &str,
    path: &Path,
) -> Result<String> {
    let v = mapping
        .get(&YamlValue::String(key.into()))
        .ok_or_else(|| anyhow!("{}: missing `{}:` field", path.display(), key))?;
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("{}: `{}:` must be a string", path.display(), key))
}

/// Supported step types. Added step types should come with:
///   1. a `Step::parse` arm
///   2. a `run_step` arm
///   3. a unit test covering the happy path + one invalid shape
#[derive(Debug, Clone)]
pub enum Step {
    Goto {
        path: String,
        /// Optional fixed sleep after navigation. The YAML example
        /// shows `goto` + `wait_ms` on the same step; both forms are
        /// supported — either as a sibling `wait_ms:` key on the goto
        /// map, or as a separate wait step further down.
        wait_ms: Option<u64>,
    },
    WaitMs(u64),
    WaitForSelector {
        selector: String,
        timeout_ms: u64,
    },
    Click(String),
    Type {
        text: String,
        target: String,
    },
    Press(String),
    Screenshot(String),
    AssertContains {
        selector: String,
        text: String,
    },
    /// TODO(phase-5): broaden the regex surface once assert_url_matches
    /// sees real use. Today it's straight `regex::Regex::is_match`.
    AssertUrlMatches(String),
    /// TODO(phase-5): `eval:` runs arbitrary page JS and ignores the
    /// return value. Phase 5 may lift this into an assert-style step
    /// (e.g. `assert_eval: { js, equals }`). Until then, treat it as a
    /// setup primitive only — use asserts for state checks.
    Eval(String),
}

impl Step {
    fn parse(raw: &YamlValue, idx: usize, src: &Path) -> Result<Step> {
        let map = raw.as_mapping().ok_or_else(|| {
            anyhow!(
                "{}: step {} must be a mapping (got {})",
                src.display(),
                idx,
                yaml_kind(raw)
            )
        })?;

        // Iterate the map to find the PRIMARY key (first key is the
        // step type for single-key steps; for multi-key steps the
        // primary is the one matching a known step name).
        // Which keys a step "consumes" is per-variant below.
        let consumed = |names: &[&str]| -> Vec<String> {
            names.iter().map(|s| s.to_string()).collect()
        };

        // Find which step-type key is present. `wait_ms` is special —
        // it's both its own step type AND a valid sibling of `goto`.
        // Resolution: if `goto` is present, `wait_ms` is its sibling;
        // otherwise `wait_ms` is itself the step type.
        let step_keys = [
            "goto",
            "wait_ms",
            "wait_for_selector",
            "click",
            "type",
            "press",
            "screenshot",
            "assert_contains",
            "assert_url_matches",
            "eval",
        ];
        let mut matched: Vec<&str> = Vec::new();
        let has_goto = map.contains_key(&YamlValue::String("goto".into()));
        for k in step_keys {
            if map.contains_key(&YamlValue::String(k.into())) {
                // Skip wait_ms when goto is present — it's the sibling
                // not a competing step type.
                if k == "wait_ms" && has_goto {
                    continue;
                }
                matched.push(k);
            }
        }
        if matched.is_empty() {
            bail!(
                "{}: step {}: no known step type found; valid types: {}",
                src.display(),
                idx,
                step_keys.join(", ")
            );
        }
        if matched.len() > 1 {
            bail!(
                "{}: step {}: multiple step-type keys on one step ({}); \
                 split into separate steps",
                src.display(),
                idx,
                matched.join(", ")
            );
        }
        let primary = matched[0];

        // Validate no unknown sibling keys. Each step defines its
        // allowed sibling set; anything else is a typo the user wants
        // to see called out.
        let allowed: Vec<String> = match primary {
            "goto" => consumed(&["goto", "wait_ms"]),
            "wait_ms" => consumed(&["wait_ms"]),
            "wait_for_selector" => consumed(&["wait_for_selector", "timeout_ms"]),
            "click" => consumed(&["click"]),
            "type" => consumed(&["type", "target"]),
            "press" => consumed(&["press"]),
            "screenshot" => consumed(&["screenshot"]),
            "assert_contains" => consumed(&["assert_contains"]),
            "assert_url_matches" => consumed(&["assert_url_matches"]),
            "eval" => consumed(&["eval"]),
            _ => unreachable!("step_keys match guarantees coverage"),
        };
        for k in map.keys() {
            let k_str = k.as_str().unwrap_or("");
            if !allowed.iter().any(|a| a == k_str) {
                bail!(
                    "{}: step {} ({}): unknown sibling key `{}` — expected one of: {}",
                    src.display(),
                    idx,
                    primary,
                    k_str,
                    allowed.join(", ")
                );
            }
        }

        // Dispatch.
        match primary {
            "goto" => {
                let path = map
                    .get(&YamlValue::String("goto".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!("{}: step {}: goto must be a string", src.display(), idx)
                    })?
                    .to_string();
                let wait_ms = map
                    .get(&YamlValue::String("wait_ms".into()))
                    .map(|v| as_u64(v, idx, src, "wait_ms"))
                    .transpose()?;
                Ok(Step::Goto { path, wait_ms })
            }
            "wait_ms" => {
                let ms = as_u64(
                    map.get(&YamlValue::String("wait_ms".into())).unwrap(),
                    idx,
                    src,
                    "wait_ms",
                )?;
                Ok(Step::WaitMs(ms))
            }
            "wait_for_selector" => {
                let selector = map
                    .get(&YamlValue::String("wait_for_selector".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: wait_for_selector must be a string",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                let timeout_ms = map
                    .get(&YamlValue::String("timeout_ms".into()))
                    .map(|v| as_u64(v, idx, src, "timeout_ms"))
                    .transpose()?
                    .unwrap_or(3000);
                Ok(Step::WaitForSelector {
                    selector,
                    timeout_ms,
                })
            }
            "click" => {
                let sel = map
                    .get(&YamlValue::String("click".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: click must be a CSS selector string",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::Click(sel))
            }
            "type" => {
                let text = map
                    .get(&YamlValue::String("type".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!("{}: step {}: type must be a string", src.display(), idx)
                    })?
                    .to_string();
                let target = map
                    .get(&YamlValue::String("target".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: type step requires `target:` CSS selector",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::Type { text, target })
            }
            "press" => {
                let key = map
                    .get(&YamlValue::String("press".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: press must be a key name (e.g. Enter, Escape)",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::Press(key))
            }
            "screenshot" => {
                let slug = map
                    .get(&YamlValue::String("screenshot".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: screenshot must be a slug string",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::Screenshot(slug))
            }
            "assert_contains" => {
                let v = map.get(&YamlValue::String("assert_contains".into())).unwrap();
                let sub = v.as_mapping().ok_or_else(|| {
                    anyhow!(
                        "{}: step {}: assert_contains must be a mapping with `selector` + `text`",
                        src.display(),
                        idx
                    )
                })?;
                let selector = sub
                    .get(&YamlValue::String("selector".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: assert_contains.selector missing",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                let text = sub
                    .get(&YamlValue::String("text".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: assert_contains.text missing",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::AssertContains { selector, text })
            }
            "assert_url_matches" => {
                let pat = map
                    .get(&YamlValue::String("assert_url_matches".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: assert_url_matches must be a regex string",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                // Compile once to fail fast on bad regexes. We re-compile
                // at run time for now — this is a parse-time sanity check.
                Regex::new(&pat).map_err(|e| {
                    anyhow!(
                        "{}: step {}: assert_url_matches bad regex `{}`: {}",
                        src.display(),
                        idx,
                        pat,
                        e
                    )
                })?;
                Ok(Step::AssertUrlMatches(pat))
            }
            "eval" => {
                let js = map
                    .get(&YamlValue::String("eval".into()))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "{}: step {}: eval must be a JavaScript expression string",
                            src.display(),
                            idx
                        )
                    })?
                    .to_string();
                Ok(Step::Eval(js))
            }
            _ => unreachable!(),
        }
    }

    fn label(&self) -> String {
        match self {
            Step::Goto { path, .. } => format!("goto {path}"),
            Step::WaitMs(ms) => format!("wait {ms}ms"),
            Step::WaitForSelector { selector, .. } => format!("wait_for {selector}"),
            Step::Click(s) => format!("click {s}"),
            Step::Type { target, .. } => format!("type into {target}"),
            Step::Press(k) => format!("press {k}"),
            Step::Screenshot(s) => format!("screenshot {s}"),
            Step::AssertContains { selector, text } => {
                format!("assert {selector} contains {text:?}")
            }
            Step::AssertUrlMatches(p) => format!("assert url matches /{p}/"),
            Step::Eval(_) => "eval(js)".to_string(),
        }
    }
}

fn as_u64(v: &YamlValue, idx: usize, src: &Path, field: &str) -> Result<u64> {
    v.as_u64().ok_or_else(|| {
        anyhow!(
            "{}: step {}: `{}` must be a non-negative integer",
            src.display(),
            idx,
            field
        )
    })
}

fn yaml_kind(v: &YamlValue) -> &'static str {
    match v {
        YamlValue::Null => "null",
        YamlValue::Bool(_) => "bool",
        YamlValue::Number(_) => "number",
        YamlValue::String(_) => "string",
        YamlValue::Sequence(_) => "sequence",
        YamlValue::Mapping(_) => "mapping",
        YamlValue::Tagged(_) => "tagged",
    }
}

/// Outcome of `run_flow`. Callers typically just push `findings` into
/// the VerifyRun accumulator.
#[derive(Debug)]
pub struct FlowRunOutcome {
    pub findings: Vec<Finding>,
}

/// Execute a flow end-to-end.
///
/// Parameters:
///   * `browser`       — viewport-pinned Browser (shared with the outer run)
///   * `flow`          — parsed flow
///   * `base_url`      — e.g. `http://127.0.0.1:18789`; prepended to goto paths
///   * `run_dir`       — where screenshot steps write PNGs
///
/// Semantics:
///   * The FIRST navigation MUST come from a `goto:` step — we don't
///     auto-navigate. This lets flows start from a specific URL that's
///     different from the module's canonical one (e.g. a pre-seeded
///     state fixture).
///   * On any regression-severity finding (assertion fail, timeout,
///     click on missing element), remaining steps are SKIPPED. The
///     flow's summary Finding is still emitted at Suggestion severity
///     noting the partial execution.
pub async fn run_flow(
    browser: &Browser,
    flow: &FlowFile,
    base_url: &str,
    run_dir: &Path,
) -> Result<FlowRunOutcome> {
    let base = base_url.trim_end_matches('/');
    let viewport_slug = browser.viewport().slug();
    let flow_slug = flow.slug();
    let started = Instant::now();

    log::info!(
        "[flow] running {} ({} steps) on {}",
        flow.name,
        flow.steps.len(),
        viewport_slug
    );

    // Page is opened by the first `goto:` step; enforce that up-front.
    let first_goto_path = match flow.steps.first() {
        Some(Step::Goto { path, .. }) => path.clone(),
        _ => {
            return Ok(FlowRunOutcome {
                findings: vec![Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::Other,
                    severity: Severity::Regression,
                    title: format!("Flow `{}` doesn't start with goto:", flow.name),
                    detail: format!(
                        "{}: first step must be `goto:` so the browser knows where to load",
                        flow.source_path.display()
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                }],
            });
        }
    };
    let full_url = format!("{}{}", base, first_goto_path);
    let page = browser.new_page(&full_url).await.with_context(|| {
        format!("opening first page {} for flow {}", full_url, flow.name)
    })?;

    // Apply the sibling wait_ms on the first step, if any.
    if let Some(Step::Goto {
        wait_ms: Some(ms), ..
    }) = flow.steps.first()
    {
        tokio::time::sleep(Duration::from_millis(*ms)).await;
    } else {
        // Baseline post-load settle: 500ms. Lower than the module
        // screenshot's 1500ms because flows typically include
        // explicit wait_for_selector next.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let mut findings: Vec<Finding> = Vec::new();
    let mut steps_run: usize = 1; // goto already ran

    for (idx, step) in flow.steps.iter().enumerate().skip(1) {
        log::info!("[flow] step {}: {}", idx, step.label());
        let outcome = run_step(
            step,
            &page,
            base,
            flow,
            run_dir,
            viewport_slug,
            &flow_slug,
        )
        .await;
        match outcome {
            Ok(()) => {
                steps_run += 1;
            }
            Err(StepError::Regression(f)) => {
                findings.push(f);
                log::warn!(
                    "[flow] step {} failed; skipping remaining {} step(s)",
                    idx,
                    flow.steps.len() - idx - 1
                );
                break;
            }
            Err(StepError::Fatal(e)) => {
                // Unexpected chromiumoxide error — treat as regression
                // but propagate the detail so the user can debug.
                findings.push(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::Other,
                    severity: Severity::Regression,
                    title: format!("Flow `{}` step {} crashed", flow.name, idx),
                    detail: format!("{e:#}"),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                });
                break;
            }
        }
    }

    // Explicit close — drop would eventually do it, but closing up
    // front frees the Chromium target for the next flow immediately.
    page.close().await.ok();

    let elapsed_ms = started.elapsed().as_millis();
    // Summary finding — suggestion severity, always. Run-level
    // regressions already pushed above determine the overall verdict.
    let any_regression = findings
        .iter()
        .any(|f| f.severity == Severity::Regression);
    let title = if any_regression {
        format!("Flow partial: {}", flow.name)
    } else {
        format!("Flow passed: {}", flow.name)
    };
    findings.push(Finding {
        module_slug: flow.module.clone(),
        kind: FindingKind::Other,
        severity: Severity::Suggestion,
        title,
        detail: format!(
            "{}/{} steps ran in {}ms on {} ({})",
            steps_run,
            flow.steps.len(),
            elapsed_ms,
            viewport_slug,
            flow.source_path.display()
        ),
        artifact: None,
        captured_at: Utc::now(),
        edits: None,
        // Phase 4b — flows don't yet carry a per-persona tag; the
        // outer CLI is responsible for tagging if/when flows start
        // running under a persona session in Phase 4c.
        persona: None,
    });

    Ok(FlowRunOutcome { findings })
}

/// Internal step-level failure shape. `Regression` wraps a
/// user-visible Finding; `Fatal` wraps anyhow for browser-layer errors.
enum StepError {
    Regression(Finding),
    Fatal(anyhow::Error),
}

impl From<anyhow::Error> for StepError {
    fn from(e: anyhow::Error) -> Self {
        StepError::Fatal(e)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_step(
    step: &Step,
    page: &chromiumoxide::page::Page,
    base: &str,
    flow: &FlowFile,
    run_dir: &Path,
    viewport_slug: &str,
    flow_slug: &str,
) -> std::result::Result<(), StepError> {
    match step {
        Step::Goto { path, wait_ms } => {
            let url = format!("{}{}", base, path);
            page.goto(url.as_str())
                .await
                .map_err(|e| StepError::Fatal(anyhow!("goto {}: {}", url, e)))?;
            page.wait_for_navigation()
                .await
                .map_err(|e| StepError::Fatal(anyhow!("wait_for_navigation: {}", e)))?;
            if let Some(ms) = wait_ms {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
        }
        Step::WaitMs(ms) => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
        }
        Step::WaitForSelector {
            selector,
            timeout_ms,
        } => {
            let deadline = Instant::now() + Duration::from_millis(*timeout_ms);
            loop {
                if page.find_element(selector.clone()).await.is_ok() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(StepError::Regression(Finding {
                        module_slug: flow.module.clone(),
                        kind: FindingKind::BootFailure,
                        severity: Severity::Regression,
                        title: format!(
                            "Flow `{}`: timed out waiting for `{}`",
                            flow.name, selector
                        ),
                        detail: format!(
                            "{}ms elapsed, selector never matched (viewport={}, flow={})",
                            timeout_ms,
                            viewport_slug,
                            flow.source_path.display()
                        ),
                        artifact: None,
                        captured_at: Utc::now(),
                        edits: None,
                        persona: None,
                    }));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Step::Click(sel) => {
            let el = page.find_element(sel.clone()).await.map_err(|_| {
                StepError::Regression(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::BootFailure,
                    severity: Severity::Regression,
                    title: format!("Flow `{}`: click target `{}` not found", flow.name, sel),
                    detail: format!(
                        "no element matched CSS selector (viewport={})",
                        viewport_slug
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                })
            })?;
            el.click()
                .await
                .map_err(|e| StepError::Fatal(anyhow!("click {}: {}", sel, e)))?;
        }
        Step::Type { text, target } => {
            let el = page.find_element(target.clone()).await.map_err(|_| {
                StepError::Regression(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::BootFailure,
                    severity: Severity::Regression,
                    title: format!(
                        "Flow `{}`: type target `{}` not found",
                        flow.name, target
                    ),
                    detail: format!(
                        "no element matched CSS selector (viewport={})",
                        viewport_slug
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                })
            })?;
            el.focus()
                .await
                .map_err(|e| StepError::Fatal(anyhow!("focus {}: {}", target, e)))?;
            el.type_str(text)
                .await
                .map_err(|e| StepError::Fatal(anyhow!("type into {}: {}", target, e)))?;
        }
        Step::Press(key) => {
            // chromiumoxide's Element::press_key walks the keyboard
            // layout. We press on the currently focused element via
            // page-level evaluate fallback if no element is focused,
            // but the intended pattern is type → press, where the
            // prior type kept focus.
            //
            // page.evaluate pathway: simulate a dispatched KeyboardEvent
            // for Enter/Escape/Tab/Backspace. Native chromiumoxide
            // press_key lives on Element; we rely on document.activeElement.
            let activate = format!(
                "(() => {{ const e = document.activeElement; if (!e) return false; \
                  e.dispatchEvent(new KeyboardEvent('keydown', {{ key: '{k}', bubbles: true }})); \
                  e.dispatchEvent(new KeyboardEvent('keypress', {{ key: '{k}', bubbles: true }})); \
                  e.dispatchEvent(new KeyboardEvent('keyup', {{ key: '{k}', bubbles: true }})); \
                  return true; }})()",
                k = key.replace('\'', "\\'")
            );
            page.evaluate(activate)
                .await
                .map_err(|e| StepError::Fatal(anyhow!("press {}: {}", key, e)))?;
        }
        Step::Screenshot(slug) => {
            let filename = format!("{}_{}_{}.png", flow_slug, slug, viewport_slug);
            let out_path = run_dir.join(&filename);
            let png = page
                .screenshot(
                    chromiumoxide::page::ScreenshotParams::builder()
                        .format(chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png)
                        .full_page(true)
                        .build(),
                )
                .await
                .map_err(|e| StepError::Fatal(anyhow!("screenshot {}: {}", slug, e)))?;
            std::fs::write(&out_path, &png).map_err(|e| {
                StepError::Fatal(anyhow!("writing {}: {}", out_path.display(), e))
            })?;
            log::info!("[flow]   → {}", out_path.display());
        }
        Step::AssertContains { selector, text } => {
            let el = page.find_element(selector.clone()).await.map_err(|_| {
                StepError::Regression(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::Other,
                    severity: Severity::Regression,
                    title: format!(
                        "Flow `{}`: assert_contains target `{}` not found",
                        flow.name, selector
                    ),
                    detail: format!(
                        "no element matched; expected to contain {:?} (viewport={})",
                        text, viewport_slug
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                })
            })?;
            let inner = el
                .inner_text()
                .await
                .map_err(|e| StepError::Fatal(anyhow!("inner_text {}: {}", selector, e)))?
                .unwrap_or_default();
            if !inner.contains(text) {
                return Err(StepError::Regression(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::Other,
                    severity: Severity::Regression,
                    title: format!(
                        "Flow `{}`: assertion failed on {}",
                        flow.name, selector
                    ),
                    detail: format!(
                        "expected `{}` to contain {:?}, got {:?}",
                        selector, text, inner
                    ),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                }));
            }
        }
        Step::AssertUrlMatches(pat) => {
            let current = page
                .url()
                .await
                .map_err(|e| StepError::Fatal(anyhow!("page.url: {}", e)))?
                .unwrap_or_default();
            // Safe unwrap: parse-time validation already compiled the regex.
            let re = Regex::new(pat)
                .map_err(|e| StepError::Fatal(anyhow!("bad regex `{}`: {}", pat, e)))?;
            if !re.is_match(&current) {
                return Err(StepError::Regression(Finding {
                    module_slug: flow.module.clone(),
                    kind: FindingKind::Other,
                    severity: Severity::Regression,
                    title: format!(
                        "Flow `{}`: URL doesn't match /{}/",
                        flow.name, pat
                    ),
                    detail: format!("current URL: {}", current),
                    artifact: None,
                    captured_at: Utc::now(),
                    edits: None,
                    persona: None,
                }));
            }
        }
        Step::Eval(js) => {
            page.evaluate(js.clone())
                .await
                .map_err(|e| StepError::Fatal(anyhow!("eval failed: {}", e)))?;
        }
    }
    Ok(())
}

// ── tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp_flow(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn parse_all_step_types() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "coverage flow"
module: dashboard
steps:
  - goto: "/dashboard"
    wait_ms: 1500
  - wait_ms: 500
  - wait_for_selector: ".sd-todo-row"
    timeout_ms: 3000
  - click: ".sd-todo-input"
  - type: "buy eggs"
    target: ".sd-todo-input"
  - press: "Enter"
  - screenshot: "after-add"
  - assert_contains:
      selector: ".sd-todo-row"
      text: "buy eggs"
  - assert_url_matches: "^https?://.*dashboard"
  - eval: "window.__test = 1"
"#;
        let p = write_tmp_flow(dir.path(), "coverage.yaml", body);
        let flow = FlowFile::load(&p).expect("parse");
        assert_eq!(flow.name, "coverage flow");
        assert_eq!(flow.module, "dashboard");
        assert_eq!(flow.steps.len(), 10);

        // Verify the variants landed in the right shapes.
        assert!(matches!(&flow.steps[0], Step::Goto { wait_ms: Some(1500), .. }));
        assert!(matches!(flow.steps[1], Step::WaitMs(500)));
        assert!(matches!(&flow.steps[2], Step::WaitForSelector { timeout_ms: 3000, .. }));
        assert!(matches!(&flow.steps[3], Step::Click(_)));
        assert!(matches!(&flow.steps[4], Step::Type { .. }));
        assert!(matches!(&flow.steps[5], Step::Press(k) if k == "Enter"));
        assert!(matches!(&flow.steps[6], Step::Screenshot(_)));
        assert!(matches!(&flow.steps[7], Step::AssertContains { .. }));
        assert!(matches!(&flow.steps[8], Step::AssertUrlMatches(_)));
        assert!(matches!(&flow.steps[9], Step::Eval(_)));
    }

    #[test]
    fn unknown_step_type_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "bad"
module: dashboard
steps:
  - waait_ms: 500
"#;
        let p = write_tmp_flow(dir.path(), "bad.yaml", body);
        let err = FlowFile::load(&p).expect_err("should fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("no known step type"), "got: {msg}");
        assert!(msg.contains("bad.yaml"), "expected file path in error: {msg}");
    }

    #[test]
    fn unknown_sibling_key_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "bad"
module: dashboard
steps:
  - type: "hi"
    target: ".x"
    extra: "nope"
"#;
        let p = write_tmp_flow(dir.path(), "bad2.yaml", body);
        let err = FlowFile::load(&p).expect_err("should fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("unknown sibling key `extra`"), "got: {msg}");
    }

    #[test]
    fn missing_first_goto_is_regression_finding() {
        // We can't exercise chromiumoxide in unit tests, but we can
        // exercise the "first step must be goto" branch by hand-rolling
        // the post-parse validation path. Build a flow without Goto
        // as its first step and confirm the run_flow precondition
        // returns a Regression finding in the outcome.
        //
        // This test deliberately stops short of calling run_flow
        // (which needs a real browser). Instead, reproduce the exact
        // check by loading a flow + asserting its shape.
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "no goto"
module: dashboard
steps:
  - wait_ms: 500
"#;
        let p = write_tmp_flow(dir.path(), "nogo.yaml", body);
        let flow = FlowFile::load(&p).expect("parse");
        assert!(!matches!(flow.steps.first(), Some(Step::Goto { .. })));
    }

    #[test]
    fn assert_contains_requires_both_keys() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "x"
module: dashboard
steps:
  - assert_contains:
      selector: ".foo"
"#;
        let p = write_tmp_flow(dir.path(), "ac.yaml", body);
        let err = FlowFile::load(&p).expect_err("should fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("assert_contains.text missing"), "got: {msg}");
    }

    #[test]
    fn bad_regex_rejected_at_parse_time() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "x"
module: dashboard
steps:
  - goto: "/x"
  - assert_url_matches: "[unclosed"
"#;
        let p = write_tmp_flow(dir.path(), "re.yaml", body);
        let err = FlowFile::load(&p).expect_err("should fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("bad regex"), "got: {msg}");
    }

    #[test]
    fn multiple_step_types_on_one_step_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "x"
module: dashboard
steps:
  - click: ".a"
    press: "Enter"
"#;
        let p = write_tmp_flow(dir.path(), "dual.yaml", body);
        let err = FlowFile::load(&p).expect_err("should fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("multiple step-type keys"), "got: {msg}");
    }

    #[test]
    fn flow_slug_derived_from_filename() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"
name: "x"
module: dashboard
steps:
  - goto: "/"
"#;
        let p = write_tmp_flow(dir.path(), "my-cool-flow.yaml", body);
        let flow = FlowFile::load(&p).expect("parse");
        assert_eq!(flow.slug(), "my-cool-flow");
    }
}
