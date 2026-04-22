//! One-click local inference runtime install.
//!
//! Downloads the latest llama.cpp Vulkan release from GitHub, extracts it
//! under `~/.syntaur/llama-vulkan/`, writes a systemd user unit that serves
//! an OpenAI-compatible endpoint on `127.0.0.1:1235`, and auto-pulls a
//! default model on first run via `llama-server --hf-repo …`.
//!
//! Used by AMD-host setup flows where manual llama.cpp install is too
//! much friction for non-technical users (Radeon owners whose only
//! cross-platform inference path is Vulkan today — Ollama Vulkan is
//! preview-only, ROCm's supported-card matrix is too narrow to default).
//!
//! The install runs in a background task. UI polls `GET
//! /api/setup/install-llama-vulkan/status` until `phase == "done"`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::Json;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::AppState;

const SERVICE_NAME: &str = "syntaur-llama-vulkan.service";
const SERVICE_PORT: u16 = 1235;

/// Default model — small enough to fit ~4 GB VRAM, good-enough quality
/// that most users don't need to swap it. Users override by picking a
/// different repo/file at install time or editing the unit afterwards.
const DEFAULT_HF_REPO: &str = "Qwen/Qwen2.5-3B-Instruct-GGUF";
const DEFAULT_HF_FILE: &str = "qwen2.5-3b-instruct-q4_k_m.gguf";

#[derive(Clone, Serialize, Debug)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum InstallPhase {
    Idle,
    FetchingRelease,
    Downloading { mb_done: u64, mb_total: u64 },
    Extracting,
    WritingService,
    StartingService,
    WaitingForModel,
    Done { url: String },
    Error { message: String },
}

#[derive(Deserialize, Default)]
pub struct InstallRequest {
    /// Override the default Hugging Face repo (e.g. user with 16+ GB
    /// VRAM might want a 7B). Optional.
    pub hf_repo: Option<String>,
    pub hf_file: Option<String>,
}

struct Shared {
    phase: InstallPhase,
    running: bool,
}

fn shared() -> &'static Arc<Mutex<Shared>> {
    static CELL: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();
    CELL.get_or_init(|| {
        Arc::new(Mutex::new(Shared {
            phase: InstallPhase::Idle,
            running: false,
        }))
    })
}

async fn set_phase(p: InstallPhase) {
    shared().lock().await.phase = p;
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
}

fn install_dir() -> PathBuf { home_dir().join(".syntaur/llama-vulkan") }
fn models_cache_dir() -> PathBuf { home_dir().join(".syntaur/models") }
fn systemd_user_dir() -> PathBuf { home_dir().join(".config/systemd/user") }

/// Detect whether we're running inside a container. Local install is
/// meaningless inside Docker — the runtime would target the container's
/// filesystem, not the user's host. Refuse with a clear message.
fn in_container() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|s| s.contains("docker") || s.contains("containerd") || s.contains("kubepods"))
            .unwrap_or(false)
}

/// Detect the right llama.cpp Vulkan asset for this OS+arch.
fn asset_pattern() -> Option<&'static str> {
    // Today we only auto-install on Linux x86_64. Windows users can
    // install manually; Apple has Metal; 32-bit is not a target.
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("ubuntu-vulkan-x64")
    } else {
        None
    }
}

// ─── HTTP handlers ─────────────────────────────────────────────────────────

pub async fn handle_install_start(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    _body: Option<Json<InstallRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    crate::setup::require_first_run_loopback(&state, peer)
        .await
        .map_err(|s| (s, "first-run endpoint requires loopback".into()))?;

    if in_container() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Local install isn't available when Syntaur runs in a container — \
             install manually on the host instead, or use a network LLM."
                .into(),
        ));
    }
    if asset_pattern().is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Auto-install is Linux x86_64 only today. Use the manual install \
             link for your platform."
                .into(),
        ));
    }

    // Refuse to start a second install if one is mid-flight.
    {
        let mut s = shared().lock().await;
        if s.running {
            return Ok(Json(serde_json::json!({
                "ok": true,
                "already_running": true,
            })));
        }
        s.running = true;
        s.phase = InstallPhase::FetchingRelease;
    }

    let body = _body.map(|Json(b)| b).unwrap_or_default();
    let hf_repo = body.hf_repo.unwrap_or_else(|| DEFAULT_HF_REPO.to_string());
    let hf_file = body.hf_file.unwrap_or_else(|| DEFAULT_HF_FILE.to_string());

    tokio::spawn(async move {
        let result = run_install(&hf_repo, &hf_file).await;
        let mut s = shared().lock().await;
        s.running = false;
        match result {
            Ok(url) => s.phase = InstallPhase::Done { url },
            Err(e) => {
                warn!("[setup/install-llama-vulkan] failed: {e}");
                s.phase = InstallPhase::Error { message: e };
            }
        }
    });

    Ok(Json(serde_json::json!({ "ok": true, "started": true })))
}

pub async fn handle_install_status(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<InstallPhase>, StatusCode> {
    crate::setup::require_first_run_loopback(&state, peer).await?;
    let phase = shared().lock().await.phase.clone();
    Ok(Json(phase))
}

// ─── Install pipeline ──────────────────────────────────────────────────────

async fn run_install(hf_repo: &str, hf_file: &str) -> Result<String, String> {
    let pattern = asset_pattern().ok_or("unsupported platform")?;

    set_phase(InstallPhase::FetchingRelease).await;
    let (asset_url, asset_name) = fetch_latest_asset(pattern).await?;
    info!("[install-llama-vulkan] resolved asset: {asset_name}");

    set_phase(InstallPhase::Downloading { mb_done: 0, mb_total: 0 }).await;
    let zip_path = download_asset(&asset_url).await?;

    set_phase(InstallPhase::Extracting).await;
    let bin_path = extract_and_find_binary(&zip_path).await?;
    info!("[install-llama-vulkan] llama-server at {}", bin_path.display());

    set_phase(InstallPhase::WritingService).await;
    write_systemd_unit(&bin_path, hf_repo, hf_file).await?;

    set_phase(InstallPhase::StartingService).await;
    start_systemd_unit().await?;

    set_phase(InstallPhase::WaitingForModel).await;
    wait_for_ready(Duration::from_secs(600)).await?;

    Ok(format!("http://127.0.0.1:{}", SERVICE_PORT))
}

async fn fetch_latest_asset(pattern: &str) -> Result<(String, String), String> {
    let client = reqwest::Client::builder()
        .user_agent("syntaur-gateway-setup/1.0")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get("https://api.github.com/repos/ggerganov/llama.cpp/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("fetch release list: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse release JSON: {e}"))?;

    let assets = json
        .get("assets")
        .and_then(|v| v.as_array())
        .ok_or("no assets in release")?;
    for a in assets {
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.contains(pattern) && name.ends_with(".zip") {
            let url = a
                .get("browser_download_url")
                .and_then(|v| v.as_str())
                .ok_or("asset missing download URL")?;
            return Ok((url.to_string(), name.to_string()));
        }
    }
    Err(format!(
        "no asset matching '{pattern}' in latest llama.cpp release"
    ))
}

async fn download_asset(url: &str) -> Result<PathBuf, String> {
    let tmp_dir = std::env::temp_dir();
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| format!("tmp dir: {e}"))?;
    let dest = tmp_dir.join("syntaur-llama-vulkan.zip");
    // If a previous install left a stale zip, remove it.
    let _ = tokio::fs::remove_file(&dest).await;

    let client = reqwest::Client::builder()
        .user_agent("syntaur-gateway-setup/1.0")
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download returned {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let total_mb = total / (1024 * 1024);

    let mut file = tokio::fs::File::create(&dest)
        .await
        .map_err(|e| format!("create zip: {e}"))?;
    use tokio::io::AsyncWriteExt;

    let mut done: u64 = 0;
    let mut last_report_mb: u64 = 0;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("download chunk: {e}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| format!("write zip: {e}"))?;
        done += bytes.len() as u64;
        let mb = done / (1024 * 1024);
        // Publish progress every 2 MB to avoid thrashing the lock.
        if mb >= last_report_mb + 2 {
            last_report_mb = mb;
            set_phase(InstallPhase::Downloading {
                mb_done: mb,
                mb_total: total_mb,
            })
            .await;
        }
    }
    file.flush().await.ok();
    Ok(dest)
}

async fn extract_and_find_binary(zip_path: &Path) -> Result<PathBuf, String> {
    let dest = install_dir();
    // Wipe any prior install so a retry doesn't half-overlay an old build.
    if dest.exists() {
        tokio::fs::remove_dir_all(&dest)
            .await
            .map_err(|e| format!("wipe {}: {e}", dest.display()))?;
    }
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| format!("create {}: {e}", dest.display()))?;

    // zip crate is sync; move extraction to a blocking task.
    let zip_path = zip_path.to_path_buf();
    let dest_clone = dest.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("open zip: {e}"))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("parse zip: {e}"))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| format!("zip entry: {e}"))?;
            let Some(entry_path) = entry.enclosed_name() else { continue };
            let out_path = dest_clone.join(entry_path);
            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|e| format!("mkdir {}: {e}", out_path.display()))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
                }
                let mut out = std::fs::File::create(&out_path)
                    .map_err(|e| format!("create {}: {e}", out_path.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("extract {}: {e}", out_path.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = entry.unix_mode() {
                        let _ = std::fs::set_permissions(
                            &out_path,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("extract join: {e}"))??;

    // Find llama-server by walking. The zip puts it under build/bin or bin
    // depending on release layout — walk rather than hardcode.
    let found = walk_for("llama-server", &dest)
        .ok_or("llama-server binary not found in extracted archive")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(
            &found,
            std::fs::Permissions::from_mode(0o755),
        )
        .await;
    }
    Ok(found)
}

fn walk_for(name: &str, root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.file_name().map(|n| n == name).unwrap_or(false) {
                return Some(path);
            }
        }
    }
    None
}

async fn write_systemd_unit(bin: &Path, hf_repo: &str, hf_file: &str) -> Result<(), String> {
    tokio::fs::create_dir_all(systemd_user_dir())
        .await
        .map_err(|e| format!("create systemd user dir: {e}"))?;
    tokio::fs::create_dir_all(models_cache_dir())
        .await
        .map_err(|e| format!("create models cache dir: {e}"))?;

    // llama-server reads HF_HOME for the model cache. Keeping it under
    // ~/.syntaur/models means Syntaur uninstall cleans the whole tree.
    let unit = format!(
        "[Unit]\n\
         Description=Syntaur local LLM (llama.cpp Vulkan)\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         Environment=HF_HOME={hf_home}\n\
         ExecStart={bin} \\\n\
         \x20   --host 127.0.0.1 --port {port} \\\n\
         \x20   --hf-repo {repo} \\\n\
         \x20   --hf-file {file} \\\n\
         \x20   --n-gpu-layers 99 \\\n\
         \x20   --jinja\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        bin = bin.display(),
        port = SERVICE_PORT,
        repo = hf_repo,
        file = hf_file,
        hf_home = models_cache_dir().display(),
    );
    let unit_path = systemd_user_dir().join(SERVICE_NAME);
    tokio::fs::write(&unit_path, unit)
        .await
        .map_err(|e| format!("write unit: {e}"))?;
    Ok(())
}

async fn start_systemd_unit() -> Result<(), String> {
    run_ok(&["systemctl", "--user", "daemon-reload"]).await?;
    run_ok(&["systemctl", "--user", "enable", SERVICE_NAME]).await?;
    // Use restart not start — if a previous install left a stale unit
    // already running, we want the new ExecStart picked up.
    run_ok(&["systemctl", "--user", "restart", SERVICE_NAME]).await?;
    Ok(())
}

async fn run_ok(argv: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .await
        .map_err(|e| format!("spawn {}: {e}", argv[0]))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{} failed: {}", argv.join(" "), stderr.trim()));
    }
    Ok(())
}

/// Poll 127.0.0.1:1235/v1/models until it answers or timeout. First start
/// can take a few minutes because llama-server downloads the model.
async fn wait_for_ready(max: Duration) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/v1/models", SERVICE_PORT);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() >= max {
            return Err(format!(
                "llama-server didn't answer on :{} within {}s. Check `journalctl \
                 --user -u {}`.",
                SERVICE_PORT,
                max.as_secs(),
                SERVICE_NAME
            ));
        }
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
