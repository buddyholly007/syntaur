//! syntaur-verify CLI — Phase 1 entry point.
//!
//!   syntaur-verify                          # auto: diff against deploy stamp,
//!                                           # sweep every impacted module
//!   syntaur-verify --module dashboard       # verify one module explicitly
//!   syntaur-verify --against <SHA>          # diff against a specific commit
//!   syntaur-verify --target-url http://...  # override where to hit (default:
//!                                           # Mac Mini staging gateway resolved
//!                                           # from the topo manifest)

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::Parser;

use syntaur_verify_core::{
    baseline::BaselineStore,
    browser::{Browser, Viewport},
    changeset::{deploy_stamp_head, resolve_against},
    corpus::Corpus,
    fix::{apply_edits, archive_accepted_fix, Budgets, FixAttempt},
    flow::{run_flow, FlowFile},
    module_map::{Module, ModuleMap},
    opus::OpusClient,
    persona::{AuthSource, Persona, PersonaCatalog},
    persona_tone,
    run::{Finding, FindingEdit, FindingKind, Severity, VerifyRun},
    visual_diff::diff_pngs,
};

#[derive(Parser, Debug)]
#[command(
    name = "syntaur-verify",
    version,
    about = "Autonomous post-build visual + functional audit for the Syntaur gateway",
    long_about = None
)]
struct Cli {
    /// Override: verify only this module (slug).
    #[arg(long)]
    module: Option<String>,

    /// Git revision to diff against. Default: the deploy stamp's git_head.
    #[arg(long)]
    against: Option<String>,

    /// Workspace path. Default: ~/openclaw-workspace.
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Module impact map path. Default: crates/syntaur-verify/module-map.yaml
    /// inside the workspace.
    #[arg(long)]
    module_map: Option<PathBuf>,

    /// Target base URL to hit. Default: Mac Mini staging gateway (from
    /// topo manifest) — matches where syntaur-ship rsyncs before canary.
    #[arg(long)]
    target_url: Option<String>,

    /// Runs directory (run artifacts + screenshots). Default: ~/.syntaur-verify/runs
    #[arg(long)]
    runs_dir: Option<PathBuf>,

    /// Bearer token to inject into every verify-driven request so the
    /// dashboard + widgets render their signed-in state (not the
    /// anonymous 401 empty-state). Accepts either the raw value or
    /// `env:NAME` to read from the environment.
    ///
    /// Setup: issue a long-lived verify-only API token via
    /// `/settings/api-tokens` (scope: read-only, no mutation) and
    /// export it to `SYNTAUR_VERIFY_AUTH_TOKEN` for headless runs.
    #[arg(long, env = "SYNTAUR_VERIFY_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Also run Claude Opus (via OpenRouter) over each screenshot for
    /// visual regression + UX improvement findings. Requires the
    /// `openrouter` key in syntaur-vault. Off by default because it
    /// hits a paid API; Phase 6 will flip this on automatically when
    /// syntaur-verify runs inside syntaur-ship.
    #[arg(long)]
    with_opus: bool,

    /// Phase 2b — if Opus returns structured edits for a regression,
    /// apply them, rebuild, reload the gateway, and re-verify the
    /// module. Revert on re-regression. Implies `--with-opus`.
    #[arg(long)]
    auto_fix: bool,

    /// Max auto-fix iterations per module before giving up +
    /// reverting. Default 3 (Sean's call — "if we can't fix it in 3
    /// tries the human should see it").
    #[arg(long, default_value_t = 3)]
    max_iter: usize,

    /// Max total lines-of-code changed across all iterations per
    /// module before the budget forces revert. Default 200 — a
    /// single-module autofix is not a refactor.
    #[arg(long, default_value_t = 200)]
    max_loc: usize,

    /// Max bytes of source code attached to each Opus call as code
    /// context (for proposing precise edits). Default 40960.
    #[arg(long, default_value_t = 40_960)]
    max_source_bytes: usize,

    /// Shell command to rebuild the gateway after applying edits.
    /// Runs in the workspace dir. Default: the release build that
    /// syntaur-ship itself uses.
    #[arg(long, default_value = "cargo build --release -p syntaur-gateway")]
    rebuild_cmd: String,

    /// Shell command to reload/restart the live gateway at the
    /// target URL between rebuild and re-verify. REQUIRED when
    /// `--auto-fix` is set — otherwise we'd be re-verifying the
    /// old binary and the loop can't converge. Example:
    ///   --reload-cmd 'ssh sean@mac-mini "systemctl --user restart syntaur-gateway"'
    #[arg(long)]
    reload_cmd: Option<String>,

    /// Seconds to wait after reload before re-verifying. Default 3.
    #[arg(long, default_value_t = 3)]
    reload_wait_secs: u64,

    // ── Phase 3: baselines + visual diff + regression corpus ────
    /// Comma-separated list of viewports to sweep per module.
    /// Accepts: `desktop`, `tablet`, `mobile`. Default: all three.
    #[arg(long, default_value = "desktop,tablet,mobile")]
    viewports: String,

    /// Override the baseline root. Default: ~/.syntaur-verify/baselines
    #[arg(long)]
    baseline_dir: Option<PathBuf>,

    /// Override the corpus root. Default: ~/.syntaur-verify/corpus
    #[arg(long)]
    corpus_dir: Option<PathBuf>,

    /// Percent-pixels threshold above which a visual diff is flagged
    /// as a regression. Default 1.0 (i.e. >1% of pixels changed).
    #[arg(long, default_value_t = 1.0)]
    diff_threshold_pct: f64,

    /// Perceptual-hash Hamming distance threshold (0-64) above which
    /// a visual diff is flagged as a regression. Default 5.
    #[arg(long, default_value_t = 5)]
    phash_threshold: u32,

    /// Replace any existing baselines with the current-run screenshots
    /// instead of diffing against them. Use after an intentional UX
    /// change: human-review the new shots, then run with this flag to
    /// lock them in.
    #[arg(long)]
    update_baselines: bool,

    /// Skip the baseline diff step entirely — Phase 1/2 compat mode.
    /// Useful when iterating on Opus prompts and the baseline noise
    /// is getting in the way.
    #[arg(long)]
    no_diff: bool,

    // ── Phase 4: user-flow YAML interpreter ────────────────────
    /// Run a single flow YAML file. Repeatable — pass `--flow X --flow Y`
    /// to run multiple. Flows run IN ADDITION to the per-module
    /// screenshot sweep and are scoped to the flow's `module:` field.
    #[arg(long = "flow", value_name = "PATH")]
    flow: Vec<PathBuf>,

    /// Directory to glob `*.yaml` flows from. Flows from `--flow` and
    /// `--flows-dir` are both executed (de-duplicated by path). Default
    /// is `<workspace>/crates/syntaur-verify/flows/`; pass a directory
    /// that exists to enable auto-discovery.
    #[arg(long)]
    flows_dir: Option<PathBuf>,

    /// Override the viewport list used for flows only. Same syntax as
    /// `--viewports`. Default: the first viewport in `--viewports`
    /// (typically `desktop`) — flows are interactive and rarely need
    /// a full cross-device sweep.
    #[arg(long)]
    flow_viewports: Option<String>,

    // ── Phase 4b: persona POV coverage ────────────────────────
    /// Verify under a specific persona's session. Repeatable —
    /// pass `--persona peter --persona silvr` to loop both through
    /// the screenshot sweep. Findings tag the persona slug in
    /// `detail` + in the new `persona` field on each Finding so the
    /// report reads as `[dashboard · peter · tablet] Visual diff…`.
    ///
    /// Session bootstrap is per `personas.yaml` — today only the
    /// `auth_token_env` form is wired up. Missing env vars →
    /// persona SKIPPED with a warn; the run continues with the rest.
    #[arg(long = "persona", value_name = "SLUG")]
    persona: Vec<String>,

    /// Override the persona catalog path. Default:
    /// `<workspace>/crates/syntaur-verify/personas.yaml`.
    #[arg(long)]
    personas_file: Option<PathBuf>,

    /// Sweep every persona declared in the catalog. Mutually inclusive
    /// with `--persona X` (union). Personas whose bootstrap can't be
    /// resolved (missing env var, flow-only path) are SKIPPED with a
    /// warn — this is the ops-friendly default so "only have one
    /// token handy" runs still work end-to-end.
    #[arg(long)]
    all_personas: bool,

    // ── Phase 4c: persona-tone audit ──────────────────────────
    /// Run the persona-tone audit phase. POSTs each persona's prompt
    /// set to /api/message and scores replies against the persona's
    /// tone rules in `persona-tone.yaml`. Complements the Phase 4b
    /// visual sweep — catches register drift (butler ack phrases
    /// dropped, hype words leaking in, etc.) that screenshots can't.
    #[arg(long)]
    persona_tone: bool,

    /// Override the persona-tone matrix path. Default:
    /// `<workspace>/crates/syntaur-verify/persona-tone.yaml`.
    #[arg(long)]
    persona_tone_file: Option<PathBuf>,

    /// Max retry count per prompt when the chat API times out.
    #[arg(long, default_value_t = 2)]
    persona_tone_retries: u32,

    /// Seconds to back off between retries.
    #[arg(long, default_value_t = 5)]
    persona_tone_retry_backoff: u64,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_secs()
    .init();

    let cli = Cli::parse();

    // tokio current-thread runtime — all chromiumoxide work is
    // single-connection. We don't need a multi-thread pool.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt");

    match rt.block_on(run(cli)) {
        Ok(run) => {
            print_report(&run);
            if run.is_clean() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("syntaur-verify: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn run(cli: Cli) -> Result<VerifyRun> {
    let home = std::env::var("HOME").context("$HOME not set")?;
    let home = PathBuf::from(home);

    // Extract owned fields up-front so later code doesn't hit
    // partial-move issues when it uses `cli.*` twice.
    let Cli {
        module: module_arg,
        against: against_arg,
        workspace: workspace_arg,
        module_map: module_map_arg,
        target_url: target_url_arg,
        runs_dir: runs_dir_arg,
        with_opus: with_opus_flag,
        auto_fix,
        max_iter,
        max_loc,
        max_source_bytes,
        rebuild_cmd,
        reload_cmd,
        reload_wait_secs,
        viewports: viewports_arg,
        baseline_dir: baseline_dir_arg,
        corpus_dir: corpus_dir_arg,
        diff_threshold_pct,
        phash_threshold,
        update_baselines,
        no_diff,
        auth_token,
        // Phase 4 flow runner fields.
        flow: flow_arg,
        flows_dir: flows_dir_arg,
        flow_viewports: flow_viewports_arg,
        // Phase 4b persona fields.
        persona: persona_slugs,
        personas_file: personas_file_arg,
        all_personas,
        // Phase 4c persona-tone fields.
        persona_tone,
        persona_tone_file: persona_tone_file_arg,
        persona_tone_retries,
        persona_tone_retry_backoff,
    } = cli;
    // Auto-fix implies Opus vision (can't fix without findings).
    let with_opus = with_opus_flag || auto_fix;
    if auto_fix && reload_cmd.is_none() {
        return Err(anyhow!(
            "--auto-fix requires --reload-cmd — otherwise re-verify hits the OLD binary \
             and the loop can't converge. Set --reload-cmd 'true' if the target already \
             auto-reloads (e.g. cargo-watch)."
        ));
    }
    let budgets = Budgets {
        max_iterations: max_iter,
        max_loc,
    };

    let workspace = workspace_arg.unwrap_or_else(|| home.join("openclaw-workspace"));
    let module_map_path = module_map_arg
        .unwrap_or_else(|| workspace.join("crates/syntaur-verify/module-map.yaml"));
    let runs_dir = runs_dir_arg.unwrap_or_else(|| home.join(".syntaur-verify/runs"));

    // Phase 3 stores. Both default under ~/.syntaur-verify, but
    // each is independently overridable so e.g. CI can point at a
    // shared NFS path for the corpus while keeping per-worker
    // baselines local.
    let baseline_store = match baseline_dir_arg {
        Some(p) => BaselineStore::with_root(p),
        None => BaselineStore::new()?,
    };
    let corpus = match corpus_dir_arg {
        Some(p) => Corpus::with_root(p),
        None => Corpus::new()?,
    };
    log::info!("[verify] baselines at {}", baseline_store.root().display());
    log::info!("[verify] corpus at {}", corpus.root().display());

    // Parse --viewports into the typed enum. Invalid tokens are a
    // hard fail — don't silently fall back to desktop, since someone
    // typing `--viewports moble` would assume they're sweeping all
    // three.
    let viewports = parse_viewports(&viewports_arg)?;
    log::info!(
        "[verify] sweeping viewports: {}",
        viewports
            .iter()
            .map(|v| v.slug())
            .collect::<Vec<_>>()
            .join(",")
    );

    let map = ModuleMap::load(&module_map_path)?;

    // Target URL — Mac Mini staging by default. Resolved via topo
    // manifest so the hardcoded IP stays out of this file.
    let target_url = match target_url_arg {
        Some(u) => u,
        None => resolve_mac_mini_staging_url()
            .unwrap_or_else(|_| "http://192.168.1.58:18789".to_string()),
    };

    // Figure out which modules need verifying. `current_changeset`
    // is passed through to Opus so the model sees the same "what
    // changed" context the impact-map used. None when `--module`
    // skips the changeset pass.
    let against_label = against_arg
        .clone()
        .unwrap_or_else(|| "(deploy stamp)".to_string());
    let mut current_changeset: Option<Vec<String>> = None;
    let modules: Vec<Module> = if let Some(slug) = &module_arg {
        let m = map
            .module(slug)
            .ok_or_else(|| anyhow!("unknown module `{slug}` — try `syntaur-verify list-modules`"))?;
        vec![m.clone()]
    } else {
        let against = match against_arg {
            Some(s) => s,
            None => {
                let stamp = home.join(".syntaur/ship/deploy-stamp.json");
                deploy_stamp_head(&stamp).with_context(|| {
                    format!("reading {} for default `--against`", stamp.display())
                })?
            }
        };

        let changeset = resolve_against(&workspace, &against)?;
        log::info!(
            "[verify] {} path(s) changed since {}",
            changeset.paths.len(),
            &changeset.against[..changeset.against.len().min(10)]
        );
        let impacted = map.impacted_by(&changeset.paths);
        if impacted.is_empty() {
            log::info!("[verify] no modules impacted — nothing to verify");
            return Ok(empty_run(
                workspace_rev(&workspace)?,
                against_label,
                &runs_dir,
                changeset.paths,
            ));
        }
        log::info!(
            "[verify] {} module(s) impacted: {}",
            impacted.len(),
            impacted.iter().cloned().collect::<Vec<_>>().join(",")
        );
        current_changeset = Some(changeset.paths.clone());
        impacted
            .iter()
            .filter_map(|slug| map.module(slug).cloned())
            .collect()
    };

    // Start a run.
    let run_id = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let run_dir = runs_dir.join(&run_id);
    std::fs::create_dir_all(&run_dir).ok();

    let head_rev = workspace_rev(&workspace)?;
    let against_rev_s = against_label;

    let mut findings: Vec<Finding> = Vec::new();

    let opus = if with_opus {
        match OpusClient::from_vault() {
            Ok(c) => {
                log::info!("[verify] Opus client armed — vision findings enabled");
                Some(c)
            }
            Err(e) => {
                log::warn!("[verify] Opus disabled ({e:#}); heuristic-only run");
                None
            }
        }
    } else {
        None
    };

    let mut covered: Vec<String> = Vec::new();

    // ── Phase 4b: build the list of POVs to sweep ───────────────
    // Each POV is one (optional persona, auth token) pair. The
    // "anonymous" POV (persona = None, token from `--auth-token` /
    // SYNTAUR_VERIFY_AUTH_TOKEN) is what runs when neither
    // `--persona` nor `--all-personas` is passed — i.e. unchanged
    // pre-4b behaviour. When persona flags are set, EACH resolved
    // persona adds one more POV and the anonymous one is dropped
    // (the caller has chosen to verify under specific identities).
    //
    // Personas whose bootstrap can't be resolved (env var missing,
    // flow-based login punted) become warn + SKIP, not hard errors.
    // The run continues with whatever did resolve — matches the
    // ops-friendly "only have one token handy" posture.
    let povs = resolve_povs(
        &workspace,
        personas_file_arg.as_deref(),
        &persona_slugs,
        all_personas,
        auth_token.clone(),
    )?;
    if povs.is_empty() {
        findings.push(Finding {
            module_slug: "(run)".to_string(),
            kind: FindingKind::Other,
            severity: Severity::Suggestion,
            title: "No persona POVs usable — run skipped".into(),
            detail: "All requested personas lacked a resolvable session \
                (auth_token_env unset/empty or only login_flow set, which is \
                deferred to Phase 4c). Set the relevant SYNTAUR_VERIFY_PERSONA_* \
                env vars and rerun."
                .into(),
            artifact: None,
            captured_at: Utc::now(),
            edits: None,
            persona: None,
        });
        let run = VerifyRun {
            run_id: run_id.clone(),
            started_at: parse_run_id_ts(&run_id),
            finished_at: Some(Utc::now()),
            against_rev: against_rev_s,
            head_rev,
            changed_paths: Vec::new(),
            modules_covered: Vec::new(),
            findings,
            run_dir: run_dir.clone(),
        };
        let report_path = run_dir.join("report.json");
        std::fs::write(&report_path, serde_json::to_vec_pretty(&run)?)
            .with_context(|| format!("writing {}", report_path.display()))?;
        return Ok(run);
    }
    let multi_pov = povs.len() > 1;
    log::info!(
        "[verify] sweeping {} POV(s): {}",
        povs.len(),
        povs.iter()
            .map(|p| p.slug.as_deref().unwrap_or("anonymous"))
            .collect::<Vec<_>>()
            .join(",")
    );

    // Phase 3: inner loop is viewport. One browser per (POV,
    // viewport) — chromiumoxide sets device metrics at launch AND
    // auth is injected at launch via `with_auth_token`, so switching
    // either dimension mid-run would require a relaunch regardless.
    //
    // Opus + auto-fix run on the PRIMARY viewport only (first in the
    // list, conventionally desktop) to keep costs bounded; tablet +
    // mobile are visual-diff-only. Auto-fix is additionally disabled
    // whenever we're running multi-POV — rewriting source from a
    // per-persona regression risks tuning the code to one identity
    // at the expense of the others. When exactly one POV runs
    // (anonymous OR a single `--persona`), auto-fix keeps its
    // Phase 2b behaviour untouched.
    let primary_viewport = *viewports.first().expect("parse_viewports rejects empty lists");

    for pov in &povs {
        let pov_label = pov.slug.as_deref().unwrap_or("anonymous");
        let pov_token = pov.token.clone();
        let auto_fix_for_pov = auto_fix && !multi_pov;
        log::info!("[verify] ── POV: {} ──", pov_label);

    for (vp_idx, viewport) in viewports.iter().copied().enumerate() {
        let is_primary = vp_idx == 0;
        log::info!(
            "[verify] launching headless Chromium ({}) for POV {}",
            viewport.slug(),
            pov_label
        );
        let browser = Browser::launch_with_viewport(viewport)
            .await?
            .with_auth_token(pov_token.clone());
        log::info!(
            "[verify] Chromium up; target_url={} viewport={} pov={}",
            target_url,
            viewport.slug(),
            pov_label
        );

        // Screenshot filename stem — includes the POV so a
        // multi-POV run doesn't clobber sibling shots in the same
        // run dir. `anonymous` stays plain (no suffix) to keep
        // single-POV runs byte-compatible with pre-4b artifact paths.
        let pov_suffix: String = match &pov.slug {
            Some(s) => format!("_{}", s),
            None => String::new(),
        };

        for module in &modules {
            let url = format!("{}{}", target_url.trim_end_matches('/'), module.url);
            log::info!(
                "[verify] {} [{} · {}] → {}",
                module.slug,
                pov_label,
                viewport.slug(),
                url
            );

            let capture_slug = format!("{}{}", module.slug, pov_suffix);
            let cap = match browser
                .capture_with_viewport(&url, &capture_slug, &run_dir)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    findings.push(Finding {
                        module_slug: module.slug.clone(),
                        kind: FindingKind::BootFailure,
                        severity: Severity::Regression,
                        title: format!(
                            "Failed to render {} [{} · {}]",
                            module.slug,
                            pov_label,
                            viewport.slug()
                        ),
                        detail: format!("{e:#}"),
                        artifact: None,
                        captured_at: Utc::now(),
                        edits: None,
                        persona: pov.slug.clone(),
                    });
                    continue;
                }
            };

            // Only count coverage once per module, not once per
            // (POV × viewport × module). Anchor it to the first POV
            // + primary viewport so the number lines up with
            // single-POV runs.
            let is_first_pov =
                std::ptr::eq(pov as *const _, &povs[0] as *const _);
            if is_primary && is_first_pov {
                covered.push(module.slug.clone());
            }
            log::info!(
                "  ↳ {} ({}ms) — screenshot {} — {} console msg(s)",
                cap.http_status.map(|s| s.to_string()).unwrap_or("?".into()),
                cap.elapsed_ms,
                cap.screenshot_path.display(),
                cap.console_messages.len()
            );

            // ── Heuristic findings — primary viewport only ──────
            // HTTP + console + perf are not meaningfully different
            // across viewports for the same backend response. Only
            // flag them once (on primary) to avoid triplicate reports.
            if is_primary {
                if let Some(status) = cap.http_status {
                    if !(200..300).contains(&status) {
                        findings.push(Finding {
                            module_slug: module.slug.clone(),
                            kind: FindingKind::BootFailure,
                            severity: Severity::Regression,
                            title: format!(
                                "HTTP {} on {} [{}]",
                                status, url, pov_label
                            ),
                            detail: format!("expected 2xx, got {}", status),
                            artifact: Some(cap.screenshot_path.clone()),
                            captured_at: Utc::now(),
                            edits: None,
                            persona: pov.slug.clone(),
                        });
                    }
                }
                // Console-error heuristic. Two nuances:
                //   1) 401 "Failed to load resource" lines are 99% the
                //      widget-fetch-without-auth noise syntaur-ship
                //      hits (ship has no --auth-token by default). We
                //      demote to Suggestion when the operator hasn't
                //      passed an auth token — they're not actionable
                //      without auth infrastructure. With an auth token,
                //      same line IS a regression (widget broke).
                //   2) Generic lower-case contains("error") is noisy —
                //      anything with the word "error" in a normal log
                //      line (e.g. "errorsCount: 0") would flip this. We
                //      require a standalone ": Error" / "Uncaught" /
                //      "SEVERE" token to be more surgical.
                // Auth-present check now uses the POV's token, so
                // anonymous POVs still get the 401-demotion behaviour
                // while persona POVs (which always carry a token) do
                // NOT — a 401 under an authenticated persona IS a
                // regression.
                let unauth = pov_token.is_none();
                for msg in &cap.console_messages {
                    let is_error = msg.contains(": Error")
                        || msg.contains("Uncaught")
                        || msg.starts_with("SEVERE")
                        || msg.to_lowercase().contains("syntaxerror")
                        || msg.to_lowercase().contains("referenceerror");
                    if !is_error {
                        continue;
                    }
                    let is_unauth_401 = unauth
                        && msg.contains("status of 401")
                        && msg.contains("Failed to load resource");
                    let severity = if is_unauth_401 {
                        Severity::Suggestion
                    } else {
                        Severity::Regression
                    };
                    findings.push(Finding {
                        module_slug: module.slug.clone(),
                        kind: FindingKind::ConsoleError,
                        severity,
                        title: if is_unauth_401 {
                            "Console 401 during unauth render (expected; pass --auth-token to audit signed-in state)".into()
                        } else {
                            format!("Console error during page load [{}]", pov_label)
                        },
                        detail: msg.clone(),
                        artifact: Some(cap.screenshot_path.clone()),
                        captured_at: Utc::now(),
                        edits: None,
                        persona: pov.slug.clone(),
                    });
                }
                if cap.elapsed_ms > 5000 {
                    findings.push(Finding {
                        module_slug: module.slug.clone(),
                        kind: FindingKind::Improvement,
                        severity: Severity::Suggestion,
                        title: format!("Slow page render [{}]", pov_label),
                        detail: format!("{}ms to render (target <5s)", cap.elapsed_ms),
                        artifact: Some(cap.screenshot_path.clone()),
                        captured_at: Utc::now(),
                        edits: None,
                        persona: pov.slug.clone(),
                    });
                }
            }

            // ── Phase 3: baseline diff ──────────────────────────
            // Skipped when `--no-diff`. First run of a module/viewport
            // with no existing baseline: save current as baseline,
            // emit an Other/Suggestion "baseline captured" finding.
            // Phase 4b: baseline path is now keyed on the POV too —
            // the store call takes `pov.slug.as_deref()` so each
            // persona gets its own reference image.
            if !no_diff {
                handle_baseline(
                    &baseline_store,
                    &module.slug,
                    pov.slug.as_deref(),
                    pov_label,
                    viewport,
                    &cap.screenshot_path,
                    &run_dir,
                    diff_threshold_pct,
                    phash_threshold,
                    update_baselines,
                    &mut findings,
                );
            }

            // ── Phase 2 + 2b: Opus + auto-fix — primary only ───
            if is_primary {
                if let Some(client) = &opus {
                    let changed = current_changeset.as_deref().unwrap_or(&[]);
                    let sources = if auto_fix_for_pov {
                        collect_source_context(&workspace, &map, &module.slug, max_source_bytes)
                    } else {
                        Vec::new()
                    };

                    let opus_result = client
                        .analyze_module_with_source(
                            &module.slug,
                            &url,
                            &cap.screenshot_path,
                            changed,
                            &sources,
                            /* request_edits = */ auto_fix_for_pov,
                        )
                        .await;

                    match opus_result {
                        Ok(mut opus_findings) => {
                            log::info!(
                                "  ↳ Opus returned {} finding(s) for {} [{}]",
                                opus_findings.len(),
                                module.slug,
                                pov_label
                            );
                            // Phase 4b — stamp the persona on every
                            // finding Opus returns so the report lines
                            // read `[module · persona · viewport]`.
                            for f in opus_findings.iter_mut() {
                                if f.persona.is_none() {
                                    f.persona = pov.slug.clone();
                                }
                            }
                            if auto_fix_for_pov
                                && opus_findings
                                    .iter()
                                    .any(|f| f.severity == Severity::Regression)
                            {
                                match try_autofix(
                                    &workspace,
                                    &map,
                                    &module.slug,
                                    &url,
                                    &run_dir,
                                    &browser,
                                    client,
                                    &opus_findings,
                                    &rebuild_cmd,
                                    reload_cmd.as_deref(),
                                    reload_wait_secs,
                                    budgets,
                                    max_source_bytes,
                                    changed,
                                )
                                .await
                                {
                                    Ok(AutoFixOutcome::Clean {
                                        iterations,
                                        loc,
                                        final_findings,
                                        after_png,
                                        applied_edits,
                                        trigger,
                                    }) => {
                                        log::info!(
                                            "  ↳ auto-fix CLEAN after {iterations} iter, {loc} LoC"
                                        );
                                        // Corpus hook — archive the
                                        // evidence so Phase 4+ can
                                        // cross-reference historical
                                        // fixes at the same site.
                                        if let Err(e) = archive_accepted_fix(
                                            &corpus,
                                            &trigger,
                                            &cap.screenshot_path,
                                            &after_png,
                                            &applied_edits,
                                            &head_rev,
                                        ) {
                                            log::warn!(
                                                "  ↳ corpus archive failed: {e:#} \
                                                 (fix still accepted, just not archived)"
                                            );
                                        }
                                        opus_findings = final_findings;
                                        // Post-fix findings come back
                                        // untagged; re-stamp persona.
                                        for f in opus_findings.iter_mut() {
                                            if f.persona.is_none() {
                                                f.persona = pov.slug.clone();
                                            }
                                        }
                                        opus_findings.push(Finding {
                                            module_slug: module.slug.clone(),
                                            kind: FindingKind::Other,
                                            severity: Severity::Suggestion,
                                            title: format!(
                                                "auto-fix applied ({iterations} iter, {loc} LoC)"
                                            ),
                                            detail:
                                                "regressions were auto-fixed and re-verified clean"
                                                    .to_string(),
                                            artifact: None,
                                            captured_at: Utc::now(),
                                            edits: None,
                                            persona: pov.slug.clone(),
                                        });
                                    }
                                    Ok(AutoFixOutcome::Reverted { iterations, reason }) => {
                                        log::warn!(
                                            "  ↳ auto-fix REVERTED after {iterations} iter: {reason}"
                                        );
                                        opus_findings.push(Finding {
                                            module_slug: module.slug.clone(),
                                            kind: FindingKind::Other,
                                            severity: Severity::Suggestion,
                                            title: format!(
                                                "auto-fix reverted ({iterations} iter): {reason}"
                                            ),
                                            detail:
                                                "regressions persist; human review needed"
                                                    .to_string(),
                                            artifact: None,
                                            captured_at: Utc::now(),
                                            edits: None,
                                            persona: pov.slug.clone(),
                                        });
                                    }
                                    Err(e) => {
                                        log::error!("  ↳ auto-fix crashed: {e:#}");
                                    }
                                }
                            }
                            findings.extend(opus_findings);
                        }
                        Err(e) => {
                            log::warn!("  ↳ Opus failed for {}: {e:#}", module.slug);
                        }
                    }
                }
            }
        }

        // Drop the browser (closes Chromium) before launching the
        // next viewport. Explicit drop would be a no-op; the `let
        // browser = ...` above goes out of scope here.
        drop(browser);
    }
    } // end for pov in &povs

    let _ = primary_viewport; // reserved for future primary-specific reporting

    // ── Phase 4: user-flow interpreter ──────────────────────────
    // Flows run AFTER the per-module screenshot sweep so their
    // findings slot into the same report.json. Viewport selection is
    // independent of the module viewports list (see --flow-viewports).
    let flow_files = discover_flows(&workspace, &flow_arg, flows_dir_arg.as_deref())?;
    if !flow_files.is_empty() {
        let flow_viewports = match &flow_viewports_arg {
            Some(s) => parse_viewports(s)?,
            None => vec![*viewports.first().expect("viewports non-empty")],
        };
        log::info!(
            "[verify] running {} flow(s) across viewports: {}",
            flow_files.len(),
            flow_viewports
                .iter()
                .map(|v| v.slug())
                .collect::<Vec<_>>()
                .join(",")
        );
        for flow_viewport in flow_viewports {
            let fbrowser = Browser::launch_with_viewport(flow_viewport)
                .await?
                .with_auth_token(auth_token.clone());
            for ff in &flow_files {
                match run_flow(&fbrowser, ff, &target_url, &run_dir).await {
                    Ok(outcome) => {
                        log::info!(
                            "[verify] flow {} on {} produced {} finding(s)",
                            ff.name,
                            flow_viewport.slug(),
                            outcome.findings.len()
                        );
                        findings.extend(outcome.findings);
                    }
                    Err(e) => {
                        // Fatal flow-level error — couldn't even open
                        // the page. Emit one regression so the run
                        // isn't silently clean.
                        log::warn!("[verify] flow {} crashed: {e:#}", ff.name);
                        findings.push(Finding {
                            module_slug: ff.module.clone(),
                            kind: FindingKind::BootFailure,
                            severity: Severity::Regression,
                            title: format!("Flow `{}` failed to start", ff.name),
                            detail: format!("{e:#}"),
                            artifact: None,
                            captured_at: Utc::now(),
                            edits: None,
                            // Flows are persona-agnostic today
                            // (Phase 4b): they use the CLI's
                            // `--auth-token`, not a persona's. Leave
                            // untagged so the report groups them
                            // separately from persona findings.
                            persona: None,
                        });
                    }
                }
            }
            drop(fbrowser);
        }
    }

    // ── Phase 4c — persona-tone audit ────────────────────────────────
    // Runs AFTER the visual sweep + flows so the tone findings land in
    // the same report.json. Per-persona POSTs, rule-scored, one finding
    // per failing rule.
    if persona_tone {
        let tone_path = persona_tone_file_arg
            .clone()
            .unwrap_or_else(|| persona_tone::default_matrix_path(&workspace));
        log::info!("[verify] persona-tone: loading matrix {}", tone_path.display());
        let matrix = persona_tone::PersonaToneMatrix::load(&tone_path)?;
        let tone_cfg = persona_tone::ToneRunConfig {
            retries: persona_tone_retries,
            retry_backoff: std::time::Duration::from_secs(persona_tone_retry_backoff),
            timeout: std::time::Duration::from_secs(45),
        };
        // Filter: if --persona X was passed, limit the tone sweep to
        // the same set. Otherwise sweep every slug in the matrix.
        let tone_filter: Vec<String> = if persona_slugs.is_empty() {
            Vec::new()
        } else {
            persona_slugs.clone()
        };
        // Token resolution re-uses the Phase 4b Persona catalog so a
        // single `personas.yaml` maps slug → env-var → bearer token.
        let token_catalog_path = personas_file_arg
            .clone()
            .unwrap_or_else(|| workspace.join("crates/syntaur-verify/personas.yaml"));
        let token_catalog = PersonaCatalog::load(&token_catalog_path).ok();
        let tone_findings = persona_tone::run_tone_audit(
            &matrix,
            &tone_filter,
            &target_url,
            &tone_cfg,
            |slug| match token_catalog
                .as_ref()
                .and_then(|c| c.get(slug))
                .map(|p| p.auth_token())
            {
                Some(Ok(AuthSource::Env { token, .. })) => Some(token),
                _ => None,
            },
        )
        .await?;
        log::info!(
            "[verify] persona-tone: {} finding(s) from tone audit",
            tone_findings.len()
        );
        findings.extend(tone_findings);
    }

    let run = VerifyRun {
        run_id: run_id.clone(),
        started_at: parse_run_id_ts(&run_id),
        finished_at: Some(Utc::now()),
        against_rev: against_rev_s,
        head_rev,
        changed_paths: Vec::new(), // we don't always have the changeset (--module path skips it)
        modules_covered: covered,
        findings,
        run_dir: run_dir.clone(),
    };

    // Persist the full run JSON so Phase 2/3 can consume it + Stop
    // hook (Phase 6) can check for newer-than-deploy report.json.
    let report_path = run_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&run)?)
        .with_context(|| format!("writing {}", report_path.display()))?;
    log::info!("[verify] wrote {}", report_path.display());

    Ok(run)
}

fn print_report(r: &VerifyRun) {
    println!();
    println!("─── syntaur-verify {} ───", r.run_id);
    println!("  modules covered:  {}", r.modules_covered.join(", "));
    println!(
        "  regressions:      {}    suggestions: {}",
        r.regressions(),
        r.suggestions()
    );
    if r.findings.is_empty() {
        println!("  ✓ clean");
        return;
    }
    for f in &r.findings {
        let icon = match f.severity {
            Severity::Regression => "✗",
            Severity::Suggestion => "·",
        };
        println!("  {} [{}] {}: {}", icon, f.module_slug, f.title, f.detail);
    }
}

fn empty_run(head: String, against: String, runs_dir: &std::path::Path, paths: Vec<String>) -> VerifyRun {
    let run_id = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    VerifyRun {
        run_id: run_id.clone(),
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        against_rev: against,
        head_rev: head,
        changed_paths: paths,
        modules_covered: Vec::new(),
        findings: Vec::new(),
        run_dir: runs_dir.join(run_id),
    }
}

fn workspace_rev(workspace: &std::path::Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("git rev-parse HEAD")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn parse_run_id_ts(id: &str) -> chrono::DateTime<Utc> {
    chrono::NaiveDateTime::parse_from_str(id, "%Y%m%d-%H%M%S")
        .map(|n| n.and_utc())
        .unwrap_or_else(|_| Utc::now())
}

// ── Phase 2b: source context gathering + auto-fix loop ──────────

/// For one module, read workspace source files the impact map
/// associates with that module slug (module-specific first, cross-
/// cutting second) and pack them into `(path, body)` tuples for the
/// Opus prompt. Per-file contents are truncated at `max_source_bytes
/// / 2` and the total is capped at `max_source_bytes` so one huge
/// file can't starve the others.
fn collect_source_context(
    workspace: &std::path::Path,
    map: &syntaur_verify_core::module_map::ModuleMap,
    slug: &str,
    max_source_bytes: usize,
) -> Vec<(String, String)> {
    let per_file_cap = max_source_bytes / 2;
    let mut budget = max_source_bytes;
    let mut out: Vec<(String, String)> = Vec::new();
    for rel in map.paths_for(slug) {
        if budget < 256 {
            break;
        }
        let abs = workspace.join(&rel);
        let body = match std::fs::read_to_string(&abs) {
            Ok(b) => b,
            Err(_) => {
                // Mapped path may not exist (e.g. YAML ahead of
                // repo). Skip silently — impact map is source of
                // truth for what MIGHT matter, not what DOES exist.
                continue;
            }
        };
        let trimmed = truncate_source(&body, per_file_cap.min(budget));
        budget = budget.saturating_sub(trimmed.len());
        out.push((rel, trimmed));
    }
    out
}

fn truncate_source(body: &str, max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return body.to_string();
    }
    // Keep the top (usually imports + struct defs) and the first
    // portion of implementation. Tag the truncation so Opus knows
    // it's not a complete view and won't fabricate edits past the
    // cutoff.
    let head = &body[..max_bytes.min(body.len())];
    // Snap to the last newline to avoid splitting a line.
    let snap = head.rfind('\n').unwrap_or(head.len());
    format!(
        "{}\n// ... (truncated {} bytes — do not propose edits below this point) ...\n",
        &head[..snap],
        body.len() - snap
    )
}

#[derive(Debug)]
enum AutoFixOutcome {
    Clean {
        iterations: usize,
        loc: usize,
        final_findings: Vec<Finding>,
        /// Post-fix screenshot — archived to the corpus alongside the
        /// pre-fix shot the outer loop already has in hand.
        after_png: PathBuf,
        /// Every edit that was ACCEPTED (not reverted) across all
        /// iterations — what actually fixed the bug.
        applied_edits: Vec<FindingEdit>,
        /// The first regression finding that triggered the auto-fix.
        /// Used as the corpus entry's `meta.kind` + title so Phase 4
        /// lookups can find it by category.
        trigger: Finding,
    },
    Reverted {
        iterations: usize,
        reason: String,
    },
}

#[allow(clippy::too_many_arguments)]
async fn try_autofix(
    workspace: &std::path::Path,
    map: &syntaur_verify_core::module_map::ModuleMap,
    module_slug: &str,
    url: &str,
    run_dir: &std::path::Path,
    browser: &Browser,
    client: &OpusClient,
    initial_findings: &[Finding],
    rebuild_cmd: &str,
    reload_cmd: Option<&str>,
    reload_wait_secs: u64,
    budgets: Budgets,
    max_source_bytes: usize,
    changed: &[String],
) -> Result<AutoFixOutcome> {
    // Capture the first regression that triggered the auto-fix — it
    // becomes the corpus entry's meta when/if we succeed. Pulled out
    // here (before iteration) so we preserve the ORIGINAL symptom,
    // not whatever the re-verify pass happens to report.
    let trigger: Finding = initial_findings
        .iter()
        .find(|f| f.severity == Severity::Regression)
        .cloned()
        .unwrap_or_else(|| Finding {
            module_slug: module_slug.to_string(),
            kind: FindingKind::Other,
            severity: Severity::Regression,
            title: "Unknown regression".into(),
            detail: "try_autofix called with no regression — should not happen".into(),
            artifact: None,
            captured_at: Utc::now(),
            edits: None,
            persona: None,
        });

    // Collect edits from the initial pass. If none, nothing to do.
    let mut pending_edits = collect_edits(initial_findings);
    if pending_edits.is_empty() {
        return Ok(AutoFixOutcome::Reverted {
            iterations: 0,
            reason: "no edits proposed".into(),
        });
    }

    let mut attempts: Vec<FixAttempt> = Vec::new();
    let mut loc_total: usize = 0;
    let mut iter: usize = 0;
    // Track all edits applied across iterations — what survives into
    // the corpus on success is the union of edits we actually kept,
    // not just the last batch.
    let mut all_applied_edits: Vec<FindingEdit> = Vec::new();

    let revert_all = |attempts: &[FixAttempt]| {
        for a in attempts.iter().rev() {
            if let Err(e) = a.revert() {
                log::error!("[autofix] revert iter {} failed: {e:#}", a.iteration);
            }
        }
    };

    loop {
        iter += 1;
        if iter > budgets.max_iterations {
            revert_all(&attempts);
            return Ok(AutoFixOutcome::Reverted {
                iterations: iter - 1,
                reason: format!("max_iter={} exhausted", budgets.max_iterations),
            });
        }

        // Budget check — refuse to START an iter that would bust
        // the LoC cap (cheap pre-apply estimate via edit diffs).
        let projected: usize = pending_edits
            .iter()
            .map(|e| syntaur_verify_core::fix::count_loc_delta(&e.old_string, &e.new_string))
            .sum();
        if loc_total + projected > budgets.max_loc {
            revert_all(&attempts);
            return Ok(AutoFixOutcome::Reverted {
                iterations: iter - 1,
                reason: format!(
                    "LoC budget: {} applied + {} projected > {}",
                    loc_total, projected, budgets.max_loc
                ),
            });
        }

        log::info!(
            "[autofix] iter {} — applying {} edit(s)",
            iter,
            pending_edits.len()
        );
        let attempt = match apply_edits(workspace, iter, &pending_edits) {
            Ok(a) => a,
            Err(e) => {
                log::warn!("[autofix] apply failed: {e:#}");
                revert_all(&attempts);
                return Ok(AutoFixOutcome::Reverted {
                    iterations: iter,
                    reason: format!("apply failed: {e}"),
                });
            }
        };
        loc_total += attempt.loc_applied;
        attempts.push(attempt);
        // Remember what we just applied — on success these go into
        // the corpus; on revert they vanish with the rest.
        all_applied_edits.extend(pending_edits.iter().cloned());

        // Rebuild. If this fails the new code is syntactically or
        // semantically invalid — revert everything and bail.
        log::info!("[autofix] iter {iter} — rebuilding with `{rebuild_cmd}`");
        if let Err(e) = run_shell(rebuild_cmd, workspace) {
            log::warn!("[autofix] rebuild failed: {e:#}");
            revert_all(&attempts);
            return Ok(AutoFixOutcome::Reverted {
                iterations: iter,
                reason: format!("rebuild failed: {e}"),
            });
        }

        // Reload target. Reload command is required (validated
        // upstream) so no branching here.
        if let Some(cmd) = reload_cmd {
            log::info!("[autofix] iter {iter} — reloading target with `{cmd}`");
            if let Err(e) = run_shell(cmd, workspace) {
                log::warn!("[autofix] reload failed: {e:#}");
                revert_all(&attempts);
                return Ok(AutoFixOutcome::Reverted {
                    iterations: iter,
                    reason: format!("reload failed: {e}"),
                });
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(reload_wait_secs)).await;

        // Re-verify this module only. Fresh screenshot + fresh Opus
        // call with updated source context (the edits are now in
        // the working tree). Iteration suffix so we don't clobber
        // the initial screenshot (useful for post-mortem).
        let reverify_slug = format!("{}-iter{}", module_slug, iter);
        let cap = browser
            .capture_with_viewport(url, &reverify_slug, run_dir)
            .await?;
        let sources = collect_source_context(workspace, map, module_slug, max_source_bytes);
        let new_findings = client
            .analyze_module_with_source(
                module_slug,
                url,
                &cap.screenshot_path,
                changed,
                &sources,
                /* request_edits = */ true,
            )
            .await?;

        let regressions: Vec<&Finding> = new_findings
            .iter()
            .filter(|f| f.severity == Severity::Regression)
            .collect();

        if regressions.is_empty() {
            log::info!("[autofix] iter {iter} — CLEAN");
            // Success — keep the attempts (don't revert). Return
            // the final (non-regression) findings so the outer run
            // report can include Opus's post-fix suggestions too.
            return Ok(AutoFixOutcome::Clean {
                iterations: iter,
                loc: loc_total,
                final_findings: new_findings,
                after_png: cap.screenshot_path.clone(),
                applied_edits: all_applied_edits,
                trigger,
            });
        }

        // Still regressions — if any NEW edits were proposed, try
        // another iteration. Otherwise revert + give up.
        pending_edits = collect_edits(&new_findings);
        if pending_edits.is_empty() {
            revert_all(&attempts);
            return Ok(AutoFixOutcome::Reverted {
                iterations: iter,
                reason: format!(
                    "{} regression(s) remain, no further edits proposed",
                    regressions.len()
                ),
            });
        }
    }
}

fn collect_edits(findings: &[Finding]) -> Vec<FindingEdit> {
    let mut out = Vec::new();
    for f in findings {
        if f.severity != Severity::Regression {
            continue;
        }
        if let Some(edits) = &f.edits {
            out.extend(edits.iter().cloned());
        }
    }
    out
}

/// Run a shell command in `cwd`. Inherits stdout/stderr so the
/// user sees cargo progress. Returns an error on non-zero exit.
fn run_shell(cmd: &str, cwd: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("spawning `{cmd}`"))?;
    if !status.success() {
        anyhow::bail!("`{cmd}` exited with {status}");
    }
    Ok(())
}

/// Parse `--viewports` into typed enums. Rejects empty lists + unknown
/// tokens — better a hard error than a silent fallback to desktop.
fn parse_viewports(s: &str) -> Result<Vec<Viewport>> {
    let mut out: Vec<Viewport> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for raw in s.split(',') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let vp = match t.to_ascii_lowercase().as_str() {
            "desktop" => Viewport::Desktop,
            "tablet" => Viewport::Tablet,
            "mobile" => Viewport::Mobile,
            other => {
                anyhow::bail!(
                    "unknown viewport `{other}` — expected one or more of desktop,tablet,mobile"
                );
            }
        };
        if seen.insert(vp.slug()) {
            out.push(vp);
        }
    }
    if out.is_empty() {
        anyhow::bail!("--viewports produced an empty list — pass at least one of desktop,tablet,mobile");
    }
    Ok(out)
}

/// Baseline save-or-diff step for one (module, persona?, viewport) capture.
/// Pushes Findings into the run accumulator. Errors are logged but
/// never propagated — a corrupt baseline shouldn't take down the run,
/// it should just surface as a regression / warning.
///
/// Phase 4b adds the `persona` + `pov_label` args:
///   * `persona` is the baseline-path segment (`None` == anonymous).
///   * `pov_label` is the human-readable tag for log + Finding text
///     (`"anonymous"` / `"peter"` / …) so a regression reads as
///     `[module · persona · viewport]`.
#[allow(clippy::too_many_arguments)]
fn handle_baseline(
    store: &BaselineStore,
    module_slug: &str,
    persona: Option<&str>,
    pov_label: &str,
    viewport: Viewport,
    current_path: &std::path::Path,
    run_dir: &std::path::Path,
    threshold_pct: f64,
    phash_threshold: u32,
    update_baselines: bool,
    findings: &mut Vec<Finding>,
) {
    let current = match std::fs::read(current_path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!(
                "[verify] can't re-read current screenshot {} for diff: {e:#}",
                current_path.display()
            );
            return;
        }
    };

    // Force re-baseline mode: overwrite any existing baseline with the
    // current shot and emit a suggestion so the run report makes the
    // action visible.
    if update_baselines {
        match store.save_for(module_slug, persona, viewport, &current) {
            Ok(()) => {
                log::info!(
                    "[verify] baseline updated for {} [{} · {}]",
                    module_slug,
                    pov_label,
                    viewport.slug()
                );
                findings.push(Finding {
                    module_slug: module_slug.to_string(),
                    kind: FindingKind::Other,
                    severity: Severity::Suggestion,
                    title: format!(
                        "Baseline updated ({} · {})",
                        pov_label,
                        viewport.slug()
                    ),
                    detail: format!(
                        "overwrote {} with current capture",
                        store.path_for(module_slug, persona, viewport).display()
                    ),
                    artifact: Some(current_path.to_path_buf()),
                    captured_at: Utc::now(),
                    edits: None,
                    persona: persona.map(|s| s.to_string()),
                });
            }
            Err(e) => {
                log::warn!(
                    "[verify] baseline save failed for {} [{} · {}]: {e:#}",
                    module_slug,
                    pov_label,
                    viewport.slug()
                );
            }
        }
        return;
    }

    // First run of this (module, persona, viewport): save as baseline +
    // emit an advisory. NOT a regression — nothing's wrong, we just
    // didn't have a reference to compare to.
    if !store.exists_for(module_slug, persona, viewport) {
        match store.save_for(module_slug, persona, viewport, &current) {
            Ok(()) => {
                log::info!(
                    "[verify] baseline captured for {} [{} · {}]",
                    module_slug,
                    pov_label,
                    viewport.slug()
                );
                findings.push(Finding {
                    module_slug: module_slug.to_string(),
                    kind: FindingKind::Other,
                    severity: Severity::Suggestion,
                    title: format!(
                        "Baseline captured ({} · {})",
                        pov_label,
                        viewport.slug()
                    ),
                    detail: format!(
                        "no prior baseline; saved current capture to {}",
                        store.path_for(module_slug, persona, viewport).display()
                    ),
                    artifact: Some(current_path.to_path_buf()),
                    captured_at: Utc::now(),
                    edits: None,
                    persona: persona.map(|s| s.to_string()),
                });
            }
            Err(e) => {
                log::warn!(
                    "[verify] baseline save failed for {} [{} · {}]: {e:#}",
                    module_slug,
                    pov_label,
                    viewport.slug()
                );
            }
        }
        return;
    }

    // Baseline exists — run the diff.
    let baseline = match store.load_for(module_slug, persona, viewport) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("[verify] {e:#}");
            return;
        }
    };
    let diff = match diff_pngs(&baseline, &current, /* emit_diff_image = */ true) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "[verify] visual diff failed for {} [{} · {}]: {e:#}",
                module_slug,
                pov_label,
                viewport.slug()
            );
            return;
        }
    };

    // Persist the diff overlay PNG alongside the current shot so the
    // run report links the user straight to the visual.
    let diff_path = if diff.diff_image.is_some() {
        // Include persona in the filename so multi-POV runs don't
        // stomp each other's diff overlays in the same run dir.
        let pov_in_name = persona.map(|s| format!("_{}", s)).unwrap_or_default();
        let name = format!("{}{}_{}_diff.png", module_slug, pov_in_name, viewport.slug());
        let p = run_dir.join(&name);
        if let Some(bytes) = &diff.diff_image {
            if let Err(e) = std::fs::write(&p, bytes) {
                log::warn!("[verify] writing diff overlay {}: {e:#}", p.display());
            }
        }
        Some(p)
    } else {
        None
    };

    let over_pixel = diff.pixel_delta_pct > threshold_pct;
    let over_phash = diff.phash_distance > phash_threshold;

    if over_pixel || over_phash {
        findings.push(Finding {
            module_slug: module_slug.to_string(),
            kind: FindingKind::VisualDiff,
            severity: Severity::Regression,
            title: format!(
                "Visual diff vs baseline ({} · {}): {:.2}% pixels, phash {}",
                pov_label,
                viewport.slug(),
                diff.pixel_delta_pct,
                diff.phash_distance
            ),
            detail: format!(
                "baseline {}x{} vs current {}x{} — \
                 {:.2}% pixels differ (threshold {:.2}%), \
                 phash distance {} (threshold {}). \
                 Review {} and if the change is intentional, \
                 re-run with --update-baselines to accept it.",
                diff.baseline_dims.0,
                diff.baseline_dims.1,
                diff.current_dims.0,
                diff.current_dims.1,
                diff.pixel_delta_pct,
                threshold_pct,
                diff.phash_distance,
                phash_threshold,
                diff_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "the screenshot".into()),
            ),
            artifact: diff_path.clone().or_else(|| Some(current_path.to_path_buf())),
            captured_at: Utc::now(),
            edits: None,
            persona: persona.map(|s| s.to_string()),
        });
    } else {
        log::info!(
            "[verify] {} [{} · {}] baseline clean ({:.2}% pixels, phash {})",
            module_slug,
            pov_label,
            viewport.slug(),
            diff.pixel_delta_pct,
            diff.phash_distance
        );
    }
}

/// Collect flow files from explicit `--flow` paths plus `--flows-dir`
/// glob. De-duplicates by canonical path so `--flow a.yaml --flows-dir .`
/// doesn't run the same flow twice. Unreadable files surface as an
/// error — better than silently skipping them, because flows are
/// load-bearing regression coverage.
fn discover_flows(
    workspace: &std::path::Path,
    explicit: &[PathBuf],
    flows_dir_override: Option<&std::path::Path>,
) -> Result<Vec<FlowFile>> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut flows: Vec<FlowFile> = Vec::new();

    for p in explicit {
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        if seen.insert(canon.clone()) {
            let ff = FlowFile::load(&canon)
                .with_context(|| format!("loading --flow {}", p.display()))?;
            flows.push(ff);
        }
    }

    // Flow auto-discovery is STRICTLY OPT-IN via --flows-dir. The prior
    // "auto-sweep <workspace>/crates/syntaur-verify/flows/" default bit
    // us on the first ship-time run: the auto-discovered flow required
    // an authenticated session to hit the todo endpoint, and
    // syntaur-ship runs verify without --auth-token, so the flow
    // always failed and blocked every deploy. Callers that want flows
    // now have to opt in explicitly (either --flow PATH or
    // --flows-dir DIR).
    let _ = workspace; // silence unused arg (kept in signature for future opt-in default)
    let dir = flows_dir_override.map(PathBuf::from);
    if let Some(dir) = dir {
        if !dir.is_dir() {
            anyhow::bail!(
                "--flows-dir {} doesn't exist or isn't a directory",
                dir.display()
            );
        }
        // Non-recursive scan is fine for v1 — callers who want
        // nested organisation can pass multiple --flow paths.
        // TODO(phase-5): recursive glob if Sean ends up with enough
        // flows to want subfolders.
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading --flows-dir {}", dir.display()))?
        {
            let entry = entry.ok();
            let Some(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or(path.clone());
            if seen.insert(canon.clone()) {
                match FlowFile::load(&canon) {
                    Ok(ff) => flows.push(ff),
                    Err(e) => {
                        log::warn!("[verify] skipping flow {}: {e:#}", path.display());
                    }
                }
            }
        }
    }

    Ok(flows)
}

/// One point-of-view for the run: an optional persona slug plus the
/// bearer token that should be injected into its browser sessions.
///
/// `slug == None` marks the anonymous POV — the default when no
/// `--persona` / `--all-personas` flag is passed. Its token comes
/// from the CLI's top-level `--auth-token` / `SYNTAUR_VERIFY_AUTH_TOKEN`
/// (may also be `None`, i.e. truly unauthenticated).
#[derive(Debug, Clone)]
struct Pov {
    slug: Option<String>,
    token: Option<String>,
}

/// Resolve the set of POVs to sweep this run.
///
/// Precedence:
///   * Neither `--persona` nor `--all-personas` set → one anonymous
///     POV carrying `auth_token` (pre-4b behaviour, byte-compatible).
///   * Either flag set → the anonymous POV is DROPPED and each
///     resolved persona becomes its own POV. A persona whose
///     `auth_token_env` is unset/empty (or is `login_flow`-only, which
///     is deferred) is WARNED + SKIPPED. Rest of the run continues.
///
/// Returns `Err` only for:
///   * catalog file unreadable / malformed
///   * `--persona X` where `X` isn't in the catalog (hard error —
///     user asked for something specific, we owe them a clear
///     "catalog knows these slugs: …" message rather than silent skip)
fn resolve_povs(
    workspace: &std::path::Path,
    personas_file: Option<&std::path::Path>,
    requested: &[String],
    all_personas: bool,
    anon_auth_token: Option<String>,
) -> Result<Vec<Pov>> {
    if requested.is_empty() && !all_personas {
        return Ok(vec![Pov {
            slug: None,
            token: anon_auth_token,
        }]);
    }

    let catalog_path = personas_file
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("crates/syntaur-verify/personas.yaml"));
    let catalog = PersonaCatalog::load(&catalog_path).with_context(|| {
        format!(
            "loading persona catalog {} — pass --personas-file to override",
            catalog_path.display()
        )
    })?;

    // Assemble the catalog-ordered slug list we actually want to cover.
    let slugs: Vec<String> = if all_personas {
        // --persona X + --all-personas is union (additive), but since
        // --all-personas subsumes everything, the requested list is a
        // no-op when --all-personas is set. We still accept it so the
        // CLI doesn't penalise a redundant but harmless combination.
        catalog.slugs()
    } else {
        requested.to_vec()
    };

    // `catalog.select` validates every requested slug exists + returns
    // the `&Persona` handles in catalog order. When `--all-personas`,
    // we feed it the full catalog so the same code path runs.
    let personas: Vec<&Persona> = catalog.select(&slugs)?;

    let mut out: Vec<Pov> = Vec::new();
    for p in personas {
        match p.auth_token()? {
            AuthSource::Env { var: _, token } => {
                log::info!(
                    "[verify] persona {} ({}) → resolved via env",
                    p.slug(),
                    p.display_name()
                );
                out.push(Pov {
                    slug: Some(p.slug().to_string()),
                    token: Some(token),
                });
            }
            AuthSource::EnvMissing { var } => {
                log::warn!(
                    "[verify] persona {} SKIPPED — env var {} is unset or empty. \
                     Set it to a read-only API token for that user and rerun.",
                    p.slug(),
                    var
                );
            }
            AuthSource::FlowPunted { flow } => {
                log::warn!(
                    "[verify] persona {} SKIPPED — catalog declares login_flow \
                     {} but flow-based login is deferred to Phase 4c. \
                     Add an auth_token_env entry to include this persona today.",
                    p.slug(),
                    flow.display()
                );
            }
            AuthSource::NoneConfigured => {
                log::warn!(
                    "[verify] persona {} SKIPPED — no auth_token_env (or login_flow) \
                     declared in the catalog.",
                    p.slug()
                );
            }
        }
    }

    Ok(out)
}

fn resolve_mac_mini_staging_url() -> Result<String> {
    use syntaur_topo_core::{default_manifest_path, manifest::Manifest};
    let m = Manifest::load(&default_manifest_path())?;
    let host = m
        .hosts
        .get("mac-mini")
        .ok_or_else(|| anyhow!("topo manifest missing mac-mini"))?;
    // Mac Mini runs the gateway on the same port as prod (18789)
    // per the deploy pipeline.
    Ok(format!("http://{}:18789", host.address))
}
