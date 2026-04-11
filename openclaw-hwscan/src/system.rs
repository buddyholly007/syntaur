//! CPU, RAM, and disk detection.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub model: String,
    pub cores: u32,
    pub threads: u32,
    pub arch: String,
    pub has_avx: bool,
    pub has_avx2: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamInfo {
    /// Total RAM in megabytes.
    pub total_mb: u64,
    /// Available RAM in megabytes.
    pub available_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub mount: String,
    pub total_gb: u64,
    pub free_gb: u64,
    pub device: String,
}

pub fn detect_cpu() -> CpuInfo {
    #[cfg(target_os = "linux")]
    {
        if let Some(info) = detect_cpu_linux() {
            return info;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(info) = detect_cpu_macos() {
            return info;
        }
    }

    // Fallback
    CpuInfo {
        model: "Unknown".to_string(),
        cores: 1,
        threads: 1,
        arch: std::env::consts::ARCH.to_string(),
        has_avx: false,
        has_avx2: false,
    }
}

#[cfg(target_os = "linux")]
fn detect_cpu_linux() -> Option<CpuInfo> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;

    let model = cpuinfo.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let threads = cpuinfo.lines()
        .filter(|l| l.starts_with("processor"))
        .count() as u32;

    // Get physical cores from lscpu
    let cores = Command::new("lscpu")
        .output()
        .ok()
        .and_then(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.lines()
                .find(|l| l.starts_with("Core(s) per socket:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.trim().parse::<u32>().ok())
        })
        .unwrap_or(threads / 2);

    let flags = cpuinfo.lines()
        .find(|l| l.starts_with("flags"))
        .map(|l| l.to_lowercase())
        .unwrap_or_default();

    Some(CpuInfo {
        model,
        cores,
        threads,
        arch: std::env::consts::ARCH.to_string(),
        has_avx: flags.contains(" avx ") || flags.contains(" avx\n"),
        has_avx2: flags.contains("avx2"),
    })
}

#[cfg(target_os = "macos")]
fn detect_cpu_macos() -> Option<CpuInfo> {
    let brand = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let cores = Command::new("sysctl")
        .args(["-n", "hw.physicalcpu"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        .unwrap_or(1);

    let threads = Command::new("sysctl")
        .args(["-n", "hw.logicalcpu"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        .unwrap_or(cores);

    // Check AVX support
    let features = Command::new("sysctl")
        .args(["-n", "machdep.cpu.features"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
        .unwrap_or_default();

    Some(CpuInfo {
        model: brand,
        cores,
        threads,
        arch: std::env::consts::ARCH.to_string(),
        has_avx: features.contains("avx"),
        has_avx2: features.contains("avx2"),
    })
}

pub fn detect_ram() -> RamInfo {
    #[cfg(target_os = "linux")]
    {
        if let Some(info) = detect_ram_linux() {
            return info;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(info) = detect_ram_macos() {
            return info;
        }
    }

    RamInfo { total_mb: 0, available_mb: 0 }
}

#[cfg(target_os = "linux")]
fn detect_ram_linux() -> Option<RamInfo> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;

    let parse_kb = |prefix: &str| -> u64 {
        meminfo.lines()
            .find(|l| l.starts_with(prefix))
            .and_then(|l| {
                l.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok())
            })
            .unwrap_or(0)
    };

    let total_kb = parse_kb("MemTotal:");
    let available_kb = parse_kb("MemAvailable:");

    Some(RamInfo {
        total_mb: total_kb / 1024,
        available_mb: available_kb / 1024,
    })
}

#[cfg(target_os = "macos")]
fn detect_ram_macos() -> Option<RamInfo> {
    let total = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .unwrap_or(0);

    // macOS doesn't have a simple "available" metric like Linux
    // Use vm_stat to approximate
    let available = Command::new("vm_stat")
        .output()
        .ok()
        .and_then(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            let free = out.lines()
                .find(|l| l.contains("Pages free:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.trim().trim_end_matches('.').parse::<u64>().ok())
                .unwrap_or(0);
            let inactive = out.lines()
                .find(|l| l.contains("Pages inactive:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.trim().trim_end_matches('.').parse::<u64>().ok())
                .unwrap_or(0);
            Some((free + inactive) * 4096) // pages to bytes (4KB pages)
        })
        .unwrap_or(0);

    Some(RamInfo {
        total_mb: total / (1024 * 1024),
        available_mb: available / (1024 * 1024),
    })
}

pub fn detect_disks() -> Vec<DiskInfo> {
    let output = Command::new("df")
        .args(["-BG", "--output=target,size,avail,source"])
        .output()
        .or_else(|_| {
            // macOS df doesn't support --output, use different format
            Command::new("df").args(["-g"]).output()
        });

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut disks = Vec::new();

    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 { continue; }

        // Linux format: target size avail source
        // macOS format: filesystem 1G-blocks Used Available Capacity Mounted
        #[cfg(target_os = "linux")]
        {
            let mount = parts[0].to_string();
            let total = parts[1].trim_end_matches('G').parse::<u64>().unwrap_or(0);
            let free = parts[2].trim_end_matches('G').parse::<u64>().unwrap_or(0);
            let device = parts[3].to_string();

            // Skip virtual filesystems
            if device.starts_with("tmpfs") || device.starts_with("devtmpfs")
                || mount.starts_with("/snap") || mount.starts_with("/boot/efi")
                || total == 0
            {
                continue;
            }

            disks.push(DiskInfo { mount, total_gb: total, free_gb: free, device });
        }

        #[cfg(target_os = "macos")]
        {
            if parts.len() >= 6 {
                let device = parts[0].to_string();
                let total = parts[1].parse::<u64>().unwrap_or(0);
                let free = parts[3].parse::<u64>().unwrap_or(0);
                let mount = parts[parts.len() - 1].to_string();

                if device.starts_with("/dev/") && total > 0 {
                    disks.push(DiskInfo { mount, total_gb: total, free_gb: free, device });
                }
            }
        }
    }

    disks
}
