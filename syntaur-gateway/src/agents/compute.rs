//! Resource Budget — "what compute is available, what's allocated, what fits".
//!
//! Backs the always-visible bar at the top of the agent settings card flip
//! (Phase 0 of vault/projects/syntaur_per_chat_settings.md). The pill shows
//! every detected GPU + CPU/RAM pool + cloud bucket with `total → used → free`,
//! rolled up by source so the user sees what's eating each byte.
//!
//! Surfaces:
//! - GET /api/compute/state — JSON snapshot of pools + per-source allocations
//! - estimate_footprint(provider, model_id) → Footprint — peek the model_footprint
//!   table; caller can ask "if I add this model, will it fit?"
//! - check_conflict(brain, tts, stt, hardware) → Vec<Conflict> — driven by the
//!   live pool state; returns warnings for the front-end to render amber chips
//!   plus 1–3 ranked alternative suggestions.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::AppState;

// ── Public types (also serialized into the /api/compute/state response) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuPool {
    pub name: String,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
    /// Per-source breakdown. Each item carries a label like "brain: Qwen-27B-Q4"
    /// and the cost it claims on this pool. Sum of `mb` should equal `vram_used_mb`.
    pub used_by: Vec<UsedBy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuRamPool {
    pub cpu_cores: u32,
    pub ram_total_mb: u64,
    pub ram_used_mb: u64,
    pub used_by: Vec<UsedBy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudPool {
    /// Active cloud providers (any chain currently has at least one entry on them).
    pub providers: Vec<String>,
    pub used_by: Vec<UsedBy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsedBy {
    /// "brain: Qwen3.5-27B-Q4_K_M" / "TTS: Orpheus" / "STT: Parakeet"
    pub label: String,
    pub mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeState {
    pub gpus: Vec<GpuPool>,
    pub cpu_ram: CpuRamPool,
    pub cloud: CloudPool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Footprint {
    pub vram_mb: u64,
    pub ram_mb: u64,
    /// Where the row came from — 'seeded' / 'measured' / 'estimated' / 'manual' / 'unknown'.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    /// "rtx-3090" / "cpu" / "cloud"
    pub pool: String,
    /// 'overflow' (would not fit), 'unknown' (footprint missing), 'privacy' (flag)
    pub kind: String,
    pub message: String,
    /// 1–3 ranked alternative actions the user could take.
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub label: String,
    pub action: String, // 'move_to_cpu' | 'switch_model' | 'go_cloud' | 'remove'
    pub detail: serde_json::Value,
}

// ── DB helpers ──────────────────────────────────────────────────────────

/// Look up the footprint of a known model. Falls back to `Footprint { 0, 0,
/// "unknown" }` when the row is missing — caller decides whether to render
/// an "Estimating…" badge or treat it as zero-cost (cloud).
pub fn estimate_footprint(
    conn: &Connection,
    provider: &str,
    model_id: &str,
) -> Footprint {
    let row: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT vram_mb, ram_mb, source FROM model_footprint
             WHERE provider = ? AND model_id = ?",
            params![provider, model_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    match row {
        Some((v, r, src)) => Footprint {
            vram_mb: v.max(0) as u64,
            ram_mb: r.max(0) as u64,
            source: src,
        },
        None => Footprint {
            vram_mb: 0,
            ram_mb: 0,
            source: "unknown".to_string(),
        },
    }
}

/// Persist a measured/estimated footprint so the next page-load sees a real
/// number instead of "Estimating…". Call with source="measured" after
/// loading a local model and observing actual VRAM, or "estimated" after
/// reading GGUF metadata.
pub fn record_footprint(
    conn: &Connection,
    provider: &str,
    model_id: &str,
    vram_mb: u64,
    ram_mb: u64,
    source: &str,
) -> rusqlite::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO model_footprint (provider, model_id, vram_mb, ram_mb, source, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(provider, model_id) DO UPDATE SET
             vram_mb = excluded.vram_mb,
             ram_mb = excluded.ram_mb,
             source = excluded.source,
             updated_at = excluded.updated_at",
        params![provider, model_id, vram_mb as i64, ram_mb as i64, source, now],
    )?;
    Ok(())
}

/// Given the active brain + tts + stt selections (provider + model id pairs)
/// and the current pool state, return any conflicts (overflow, unknown
/// footprint, etc.) so the front-end can amber-flag the chip and show a
/// "Suggest fixes" drawer.
pub fn check_conflict(
    state: &ComputeState,
    selections: &[(&str, &str, &str)], // (label, provider, model_id)
    conn: &Connection,
) -> Vec<Conflict> {
    let mut out = Vec::new();
    for (label, provider, model) in selections {
        let fp = estimate_footprint(conn, provider, model);
        // Cloud entries (vram=0, ram=0, seeded) — flag privacy only when the
        // user picked them explicitly; conflict reporter doesn't decide that.
        if fp.vram_mb == 0 && fp.ram_mb == 0 && fp.source == "seeded" {
            continue;
        }
        if fp.source == "unknown" {
            out.push(Conflict {
                pool: "unknown".into(),
                kind: "unknown".into(),
                message: format!(
                    "Footprint for {label} ({provider} · {model}) not yet known — \
                     it will be measured on first load."
                ),
                suggestions: Vec::new(),
            });
            continue;
        }
        // VRAM check: pick the largest free GPU; if the model wouldn't fit
        // there, flag overflow.
        if fp.vram_mb > 0 {
            let best = state
                .gpus
                .iter()
                .map(|g| (g, g.vram_total_mb.saturating_sub(g.vram_used_mb)))
                .max_by_key(|(_, free)| *free);
            match best {
                Some((gpu, free)) if free < fp.vram_mb => {
                    out.push(Conflict {
                        pool: gpu.name.clone(),
                        kind: "overflow".into(),
                        message: format!(
                            "{label} needs {} MB VRAM but {} only has {} MB free.",
                            fp.vram_mb, gpu.name, free
                        ),
                        suggestions: build_overflow_suggestions(label, fp.vram_mb),
                    });
                }
                None if fp.vram_mb > 0 => {
                    out.push(Conflict {
                        pool: "no-gpu".into(),
                        kind: "overflow".into(),
                        message: format!(
                            "{label} needs {} MB VRAM but no GPU is available on this gateway.",
                            fp.vram_mb
                        ),
                        suggestions: build_no_gpu_suggestions(label),
                    });
                }
                _ => {}
            }
        }
        // RAM check: only flag if the model claims significant RAM AND the
        // host doesn't have it free.
        if fp.ram_mb > 1024 {
            let free = state.cpu_ram.ram_total_mb.saturating_sub(state.cpu_ram.ram_used_mb);
            if free < fp.ram_mb {
                out.push(Conflict {
                    pool: "cpu".into(),
                    kind: "overflow".into(),
                    message: format!(
                        "{label} needs {} MB RAM but only {} MB is free.",
                        fp.ram_mb, free
                    ),
                    suggestions: build_ram_overflow_suggestions(label),
                });
            }
        }
    }
    out
}

fn build_overflow_suggestions(label: &str, vram_mb: u64) -> Vec<Suggestion> {
    vec![
        Suggestion {
            label: format!("Switch {label} to a smaller local model"),
            action: "switch_model".into(),
            detail: serde_json::json!({ "needs_vram_mb": vram_mb }),
        },
        Suggestion {
            label: format!("Move {label} to cloud (frees {vram_mb} MB VRAM)"),
            action: "go_cloud".into(),
            detail: serde_json::json!({ "frees_vram_mb": vram_mb }),
        },
        Suggestion {
            label: format!("Remove {label} from the chain"),
            action: "remove".into(),
            detail: serde_json::Value::Null,
        },
    ]
}

fn build_no_gpu_suggestions(label: &str) -> Vec<Suggestion> {
    vec![
        Suggestion {
            label: format!("Switch {label} to a CPU-friendly model"),
            action: "switch_model".into(),
            detail: serde_json::Value::Null,
        },
        Suggestion {
            label: format!("Send {label} to cloud"),
            action: "go_cloud".into(),
            detail: serde_json::Value::Null,
        },
    ]
}

fn build_ram_overflow_suggestions(label: &str) -> Vec<Suggestion> {
    vec![Suggestion {
        label: format!("Send {label} to cloud (frees host RAM)"),
        action: "go_cloud".into(),
        detail: serde_json::Value::Null,
    }]
}

// ── Live state collector ────────────────────────────────────────────────

/// Snapshot of current compute pools. Reads `/proc/meminfo`, queries any
/// available GPUs, and reads the active chains from each user's
/// `agent_settings` (or, as a fallback, the global config) to compute the
/// `used_by` rollup.
///
/// Phase 0 implementation: detects GPU + RAM only. The chain rollup is
/// intentionally conservative — when we don't know what's loaded right now
/// we leave `used_by` empty rather than fabricating numbers; the bar still
/// shows total/free which is the most important info.
pub async fn collect_state(state: &AppState) -> ComputeState {
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0);
    let (ram_total_mb, ram_used_mb) = read_meminfo();

    let gpus = detect_gpus().await;

    // Cloud bucket: every provider in the gateway's configured chain that
    // is reachable over the network (vs. localhost) is considered "in use"
    // when an entry actually points at it. Phase 0 just enumerates.
    let cloud = collect_cloud_pool(state).await;

    ComputeState {
        gpus,
        cpu_ram: CpuRamPool {
            cpu_cores,
            ram_total_mb,
            ram_used_mb,
            used_by: Vec::new(),
        },
        cloud,
    }
}

/// Read /proc/meminfo on Linux. Returns (total_mb, used_mb).
/// On other platforms returns (0, 0) — Resource Budget will render
/// "RAM unknown" and skip the conflict check.
fn read_meminfo() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        let s = match std::fs::read_to_string("/proc/meminfo") {
            Ok(s) => s,
            Err(_) => return (0, 0),
        };
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = rest
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail_kb = rest
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            }
        }
        let total_mb = total_kb / 1024;
        let avail_mb = avail_kb / 1024;
        let used_mb = total_mb.saturating_sub(avail_mb);
        (total_mb, used_mb)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (0, 0)
    }
}

/// Run `nvidia-smi --query-gpu=name,memory.total,memory.used --format=csv,noheader,nounits`
/// to enumerate NVIDIA GPUs. AMD/Apple/Intel detection deferred to a
/// future pass — in Sean's setup we only have NVIDIA on the gaming PC,
/// and the TrueNAS Apps host has no GPU to enumerate.
///
/// Errors are silent — a missing nvidia-smi just yields an empty list,
/// which the front-end renders as "No GPU detected" rather than an error.
async fn detect_gpus() -> Vec<GpuPool> {
    let output = match tokio::process::Command::new("nvidia-smi")
        .arg("--query-gpu=name,memory.total,memory.used")
        .arg("--format=csv,noheader,nounits")
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            if parts.len() < 3 {
                return None;
            }
            let total_mb: u64 = parts[1].parse().ok()?;
            let used_mb: u64 = parts[2].parse().ok()?;
            Some(GpuPool {
                name: parts[0].to_string(),
                vram_total_mb: total_mb,
                vram_used_mb: used_mb,
                used_by: Vec::new(),
            })
        })
        .collect()
}

/// Phase 0 cloud pool: enumerate the providers the gateway's LLM chain has
/// configured. The `used_by` rollup is empty until later phases attribute
/// per-agent allocations against per-provider quotas.
async fn collect_cloud_pool(_state: &AppState) -> CloudPool {
    // Provider list derived from llm config — kept in sync with
    // syntaur-gateway/src/llm.rs. We keep this list small and explicit
    // because the front-end uses these strings as keys for the privacy
    // tooltip + provider-catalog endpoints (Phase 2).
    CloudPool {
        providers: vec![
            "openrouter".into(),
            "openai".into(),
            "anthropic".into(),
            "groq".into(),
            "cerebras".into(),
            "together".into(),
            "fireworks".into(),
            "elevenlabs".into(),
            "deepgram".into(),
            "edge-tts".into(),
        ],
        used_by: Vec::new(),
    }
}

// ── HTTP handler (wired by main.rs) ─────────────────────────────────────

pub async fn handle_compute_state(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ComputeState>, StatusCode> {
    // Auth: any logged-in user can read their compute state.
    let token = crate::security::extract_session_token(&headers);
    if token.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if matches!(state.users.resolve_token(&token).await, Ok(None) | Err(_)) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(collect_state(&state).await))
}
