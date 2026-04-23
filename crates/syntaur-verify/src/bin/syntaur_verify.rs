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
    browser::Browser,
    changeset::{deploy_stamp_head, resolve_against},
    module_map::{Module, ModuleMap},
    run::{Finding, FindingKind, Severity, VerifyRun},
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
    } = cli;

    let workspace = workspace_arg.unwrap_or_else(|| home.join("openclaw-workspace"));
    let module_map_path = module_map_arg
        .unwrap_or_else(|| workspace.join("crates/syntaur-verify/module-map.yaml"));
    let runs_dir = runs_dir_arg.unwrap_or_else(|| home.join(".syntaur-verify/runs"));

    let map = ModuleMap::load(&module_map_path)?;

    // Target URL — Mac Mini staging by default. Resolved via topo
    // manifest so the hardcoded IP stays out of this file.
    let target_url = match target_url_arg {
        Some(u) => u,
        None => resolve_mac_mini_staging_url()
            .unwrap_or_else(|_| "http://192.168.1.58:18789".to_string()),
    };

    // Figure out which modules need verifying.
    let against_label = against_arg
        .clone()
        .unwrap_or_else(|| "(deploy stamp)".to_string());
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

    log::info!("[verify] launching headless Chromium");
    let browser = Browser::launch().await?;
    log::info!("[verify] Chromium up; target_url={}", target_url);

    let mut covered: Vec<String> = Vec::new();

    for module in &modules {
        let url = format!("{}{}", target_url.trim_end_matches('/'), module.url);
        log::info!("[verify] {} → {}", module.slug, url);

        match browser.capture(&url, &module.slug, &run_dir).await {
            Ok(cap) => {
                covered.push(module.slug.clone());
                log::info!(
                    "  ↳ {} ({}ms) — screenshot {} — {} console msg(s)",
                    cap.http_status.map(|s| s.to_string()).unwrap_or("?".into()),
                    cap.elapsed_ms,
                    cap.screenshot_path.display(),
                    cap.console_messages.len()
                );

                // Phase 1 minimal findings:
                //   - any JS-level error in console = regression
                //   - HTTP non-2xx = regression
                //   - slow page (>5s) = suggestion (performance)
                if let Some(status) = cap.http_status {
                    if !(200..300).contains(&status) {
                        findings.push(Finding {
                            module_slug: module.slug.clone(),
                            kind: FindingKind::BootFailure,
                            severity: Severity::Regression,
                            title: format!("HTTP {} on {}", status, url),
                            detail: format!("expected 2xx, got {}", status),
                            artifact: Some(cap.screenshot_path.clone()),
                            captured_at: Utc::now(),
                        });
                    }
                }
                for msg in &cap.console_messages {
                    // "Error:" / "SEVERE:" / "Uncaught"
                    let lower = msg.to_lowercase();
                    if lower.contains("error")
                        || lower.contains("uncaught")
                        || lower.starts_with("severe")
                    {
                        findings.push(Finding {
                            module_slug: module.slug.clone(),
                            kind: FindingKind::ConsoleError,
                            severity: Severity::Regression,
                            title: "Console error during page load".into(),
                            detail: msg.clone(),
                            artifact: Some(cap.screenshot_path.clone()),
                            captured_at: Utc::now(),
                        });
                    }
                }
                if cap.elapsed_ms > 5000 {
                    findings.push(Finding {
                        module_slug: module.slug.clone(),
                        kind: FindingKind::Improvement,
                        severity: Severity::Suggestion,
                        title: "Slow page render".into(),
                        detail: format!("{}ms to render (target <5s)", cap.elapsed_ms),
                        artifact: Some(cap.screenshot_path.clone()),
                        captured_at: Utc::now(),
                    });
                }
            }
            Err(e) => {
                findings.push(Finding {
                    module_slug: module.slug.clone(),
                    kind: FindingKind::BootFailure,
                    severity: Severity::Regression,
                    title: format!("Failed to render {}", module.slug),
                    detail: format!("{e:#}"),
                    artifact: None,
                    captured_at: Utc::now(),
                });
            }
        }
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
