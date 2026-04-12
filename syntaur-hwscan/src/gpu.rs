//! GPU detection — NVIDIA, AMD, Apple Silicon, Intel Arc.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub name: String,
    /// VRAM in megabytes (0 for shared memory architectures).
    pub vram_mb: u64,
    /// For Apple Silicon, this is the unified memory available for GPU.
    pub shared_memory_mb: u64,
    /// Whether this GPU can run local LLM inference.
    pub inference_capable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    AppleSilicon,
    IntelArc,
    IntelIntegrated,
    Unknown,
}

impl std::fmt::Display for GpuVendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nvidia => write!(f, "NVIDIA"),
            Self::Amd => write!(f, "AMD"),
            Self::AppleSilicon => write!(f, "Apple Silicon"),
            Self::IntelArc => write!(f, "Intel Arc"),
            Self::IntelIntegrated => write!(f, "Intel (integrated)"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detect all available GPUs.
pub async fn detect_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    // NVIDIA via nvidia-smi
    if let Some(nvidia_gpus) = detect_nvidia() {
        gpus.extend(nvidia_gpus);
    }

    // AMD via rocm-smi
    if let Some(amd_gpus) = detect_amd() {
        gpus.extend(amd_gpus);
    }

    // Apple Silicon
    if let Some(apple_gpu) = detect_apple_silicon() {
        gpus.push(apple_gpu);
    }

    // Intel Arc via sysfs (Linux) — basic detection
    if let Some(intel_gpus) = detect_intel() {
        gpus.extend(intel_gpus);
    }

    gpus
}

fn detect_nvidia() -> Option<Vec<GpuInfo>> {
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, ',').map(|s| s.trim()).collect();
        if parts.len() == 2 {
            let name = parts[0].to_string();
            let vram_mb = parts[1].parse::<u64>().unwrap_or(0);
            gpus.push(GpuInfo {
                vendor: GpuVendor::Nvidia,
                name,
                vram_mb,
                shared_memory_mb: 0,
                inference_capable: vram_mb >= 4096, // 4GB minimum for useful inference
            });
        }
    }

    if gpus.is_empty() { None } else { Some(gpus) }
}

fn detect_amd() -> Option<Vec<GpuInfo>> {
    // Try rocm-smi first
    let output = Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--csv"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();

    // Parse rocm-smi CSV output
    for line in stdout.lines().skip(1) {
        // Format varies, try to extract GPU name and VRAM
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 2 {
            let vram_bytes: u64 = parts.last()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let vram_mb = vram_bytes / (1024 * 1024);
            gpus.push(GpuInfo {
                vendor: GpuVendor::Amd,
                name: format!("AMD GPU {}", gpus.len()),
                vram_mb,
                shared_memory_mb: 0,
                inference_capable: vram_mb >= 4096,
            });
        }
    }

    if gpus.is_empty() { None } else { Some(gpus) }
}

fn detect_apple_silicon() -> Option<GpuInfo> {
    // Check if we're on macOS with Apple Silicon
    #[cfg(target_os = "macos")]
    {
        // sysctl hw.memsize gives total unified memory
        let output = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let total_bytes: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        let total_mb = total_bytes / (1024 * 1024);

        // Check for Apple Silicon via uname
        let arch = Command::new("uname")
            .arg("-m")
            .output()
            .ok()?;
        let arch_str = String::from_utf8_lossy(&arch.stdout);
        if !arch_str.trim().starts_with("arm64") {
            return None;
        }

        // Try to get chip name
        let chip = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "Apple Silicon".to_string());

        // Apple Silicon can use ~75% of unified memory for GPU/ML
        let gpu_available_mb = (total_mb as f64 * 0.75) as u64;

        return Some(GpuInfo {
            vendor: GpuVendor::AppleSilicon,
            name: chip,
            vram_mb: 0,
            shared_memory_mb: gpu_available_mb,
            inference_capable: gpu_available_mb >= 4096,
        });
    }

    #[cfg(not(target_os = "macos"))]
    None
}

fn detect_intel() -> Option<Vec<GpuInfo>> {
    // Check for Intel Arc via lspci (Linux)
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("lspci")
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut gpus = Vec::new();

        for line in stdout.lines() {
            let lower = line.to_lowercase();
            if lower.contains("vga") || lower.contains("3d") || lower.contains("display") {
                if lower.contains("intel") && lower.contains("arc") {
                    let name = line.split(':').last().unwrap_or("Intel Arc").trim().to_string();
                    gpus.push(GpuInfo {
                        vendor: GpuVendor::IntelArc,
                        name,
                        vram_mb: 0, // Hard to detect without specific tools
                        shared_memory_mb: 0,
                        inference_capable: true, // Arc GPUs can run inference via SYCL/OneAPI
                    });
                }
            }
        }

        if gpus.is_empty() { return None; }
        return Some(gpus);
    }

    #[cfg(not(target_os = "linux"))]
    None
}
