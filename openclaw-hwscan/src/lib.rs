//! Hardware detection and LLM service discovery for OpenClaw.
//!
//! Used by both the installer and the management dashboard to:
//! - Detect GPUs, CPU, RAM, and disk space
//! - Discover LLM services on the local network
//! - Recommend the best LLM configuration for the hardware

pub mod gpu;
pub mod system;
pub mod network;
pub mod recommend;

use serde::{Deserialize, Serialize};

/// Complete hardware scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareScan {
    pub gpus: Vec<gpu::GpuInfo>,
    pub cpu: system::CpuInfo,
    pub ram: system::RamInfo,
    pub disks: Vec<system::DiskInfo>,
}

/// Complete network scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkScan {
    pub llm_services: Vec<network::LlmService>,
}

/// Run a full hardware scan.
pub async fn scan_hardware() -> HardwareScan {
    let gpus = gpu::detect_gpus().await;
    let cpu = system::detect_cpu();
    let ram = system::detect_ram();
    let disks = system::detect_disks();

    HardwareScan { gpus, cpu, ram, disks }
}

/// Scan the local network for LLM services.
pub async fn scan_network() -> NetworkScan {
    let llm_services = network::discover_llm_services().await;
    NetworkScan { llm_services }
}

/// Run both scans and produce a recommendation.
pub async fn full_scan() -> (HardwareScan, NetworkScan, recommend::LlmRecommendation) {
    let hw = scan_hardware().await;
    let net = scan_network().await;
    let rec = recommend::recommend_llm_setup(&hw, &net);
    (hw, net, rec)
}
