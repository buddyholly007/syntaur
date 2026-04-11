//! LLM setup recommendation engine.
//!
//! Given hardware and network scan results, recommends the best
//! LLM configuration — including download instructions, cloud API
//! guidance, and honest hardware upgrade suggestions.

use serde::{Deserialize, Serialize};

use crate::gpu::GpuVendor;
use crate::{HardwareScan, NetworkScan};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRecommendation {
    pub primary: LlmBackend,
    /// Fallback backends tried in order if primary is unavailable.
    /// There should ALWAYS be at least one fallback — no single point of failure.
    pub fallbacks: Vec<LlmBackend>,
    /// Plain-language explanation of why this was recommended.
    pub explanation: String,
    /// Hardware capability tier (affects UI messaging).
    pub hardware_tier: HardwareTier,
    /// Actionable steps the user should take.
    pub setup_steps: Vec<SetupStep>,
    /// Optional hardware upgrade suggestion.
    pub upgrade_hint: Option<String>,
    /// Why having a backup matters — shown in the UI.
    pub resilience_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HardwareTier {
    /// GPU with 16GB+ VRAM — can run large models locally.
    Powerful,
    /// GPU with 8-16GB VRAM — can run medium models locally.
    Capable,
    /// GPU with 4-8GB VRAM — limited local inference.
    Limited,
    /// No usable GPU but decent RAM — CPU inference possible.
    CpuOnly,
    /// Not enough for useful local inference.
    Minimal,
}

impl std::fmt::Display for HardwareTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Powerful => write!(f, "Powerful (large local models)"),
            Self::Capable => write!(f, "Capable (medium local models)"),
            Self::Limited => write!(f, "Limited (small local models)"),
            Self::CpuOnly => write!(f, "CPU-only (slow local inference)"),
            Self::Minimal => write!(f, "Minimal (cloud recommended)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmBackend {
    /// Run a local model on this machine's GPU.
    LocalGpu {
        model_suggestion: String,
        quantization: String,
        estimated_vram_mb: u64,
        download: DownloadGuide,
    },
    /// Run a local model on CPU (slower).
    LocalCpu {
        model_suggestion: String,
        quantization: String,
        estimated_ram_mb: u64,
        download: DownloadGuide,
    },
    /// Use an LLM service found on the network.
    NetworkService {
        url: String,
        service_name: String,
        models: Vec<String>,
    },
    /// Use a cloud API (OpenRouter, OpenAI, Anthropic).
    CloudApi {
        provider: String,
        suggested_model: String,
        signup_url: String,
        has_free_tier: bool,
        estimated_cost: String,
    },
}

/// How to download and set up a local model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadGuide {
    /// Which inference server to install.
    pub server: InferenceServer,
    /// The exact model identifier to download.
    pub model_id: String,
    /// Estimated download size in GB.
    pub download_size_gb: f64,
    /// Step-by-step install instructions.
    pub install_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceServer {
    /// Ollama — easiest install, one command.
    Ollama,
    /// llama.cpp server — more control, manual setup.
    LlamaCpp,
}

impl std::fmt::Display for InferenceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ollama => write!(f, "Ollama"),
            Self::LlamaCpp => write!(f, "llama.cpp"),
        }
    }
}

/// A step the user should take during setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStep {
    pub title: String,
    pub description: String,
    /// If this step involves running a command.
    pub command: Option<String>,
    /// If this step involves visiting a URL.
    pub url: Option<String>,
    /// Whether Syntaur can do this step automatically.
    pub automatable: bool,
}

// ── Cloud provider info ─────────────────────────────────────────────────

fn openrouter_backend() -> LlmBackend {
    LlmBackend::CloudApi {
        provider: "OpenRouter".to_string(),
        suggested_model: "nvidia/llama-3.3-nemotron-super-49b-v1:free".to_string(),
        signup_url: "https://openrouter.ai/keys".to_string(),
        has_free_tier: true,
        estimated_cost: "Free tier: 1,000 req/day with $10 credit. Many free models available.".to_string(),
    }
}

fn openai_backend() -> LlmBackend {
    LlmBackend::CloudApi {
        provider: "OpenAI".to_string(),
        suggested_model: "gpt-4o-mini".to_string(),
        signup_url: "https://platform.openai.com/api-keys".to_string(),
        has_free_tier: false,
        estimated_cost: "$0.15 per 1M input tokens. ~$5-15/month for moderate use.".to_string(),
    }
}

fn anthropic_backend() -> LlmBackend {
    LlmBackend::CloudApi {
        provider: "Anthropic".to_string(),
        suggested_model: "claude-sonnet-4-6".to_string(),
        signup_url: "https://console.anthropic.com/settings/keys".to_string(),
        has_free_tier: false,
        estimated_cost: "$3 per 1M input tokens. Best reasoning but higher cost.".to_string(),
    }
}

const RESILIENCE_CLOUD_PRIMARY: &str =
    "Cloud APIs can have outages. Always configure a backup — either a local model \
     (even a small one) or a second cloud provider. Syntaur will automatically \
     switch to the fallback if the primary fails.";

const RESILIENCE_LOCAL_PRIMARY: &str =
    "Local inference is reliable, but it's good to have a cloud fallback for when you \
     need a larger model or your machine is under heavy load. Syntaur will automatically \
     route to the fallback when needed.";

const RESILIENCE_NETWORK_PRIMARY: &str =
    "Network services depend on the other machine being online. Configure a cloud API \
     or local model as backup so Syntaur keeps working if the network service goes down.";

// ── OpenRouter free model discovery ─────────────────────────────────────

/// Models known to support tool/function calling on OpenRouter's free tier.
/// Updated at build time. The installer can refresh this list live via
/// `fetch_free_tool_models()`.
pub const KNOWN_FREE_TOOL_MODELS: &[(&str, &str)] = &[
    ("nvidia/llama-3.3-nemotron-super-49b-v1:free", "Nemotron Super 49B — strong reasoning + tools"),
    ("qwen/qwen3-235b-a22b:free", "Qwen3 235B MoE — large, fast, good tool use"),
    ("qwen/qwen3-32b:free", "Qwen3 32B — balanced quality and speed"),
    ("deepseek/deepseek-chat-v3-0324:free", "DeepSeek V3 — strong coding + tools"),
    ("google/gemini-2.5-pro-exp-03-25:free", "Gemini 2.5 Pro — Google's latest"),
    ("meta-llama/llama-4-maverick:free", "Llama 4 Maverick — Meta's newest"),
];

/// Fetch the current list of free, tool-capable models from OpenRouter.
/// Falls back to KNOWN_FREE_TOOL_MODELS if the API call fails.
pub async fn fetch_free_tool_models() -> Vec<(String, String)> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return known_models_owned(),
    };

    let resp = match client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return known_models_owned(),
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return known_models_owned(),
    };

    let mut models = Vec::new();
    if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
        for model in data {
            let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let name = model.get("name").and_then(|v| v.as_str()).unwrap_or("");

            // Check if free (pricing.prompt == "0")
            let is_free = model.get("pricing")
                .and_then(|p| p.get("prompt"))
                .and_then(|p| p.as_str())
                .map(|p| p == "0")
                .unwrap_or(false);

            if !is_free { continue; }

            // Check if it supports tool calling
            let supports_tools = model.get("supported_parameters")
                .and_then(|sp| sp.as_array())
                .map(|params| params.iter().any(|p| p.as_str() == Some("tools")))
                .unwrap_or(false);

            if !supports_tools { continue; }

            // Skip tiny models (< 7B) — not useful for tool calling
            let param_count = model.get("architecture")
                .and_then(|a| a.get("parameters"))
                .and_then(|p| p.as_str())
                .and_then(|s| s.replace("B", "").parse::<f64>().ok())
                .unwrap_or(0.0);

            if param_count > 0.0 && param_count < 7.0 { continue; }

            let desc = format!("{} — free, supports tool calling", name);
            models.push((id.to_string(), desc));
        }
    }

    if models.is_empty() {
        known_models_owned()
    } else {
        // Sort by parameter count (larger first) — best models on top
        models
    }
}

fn known_models_owned() -> Vec<(String, String)> {
    KNOWN_FREE_TOOL_MODELS.iter()
        .map(|(id, desc)| (id.to_string(), desc.to_string()))
        .collect()
}

// ── Download guides ─────────────────────────────────────────────────────

fn ollama_download(model: &str, quant: &str, size_gb: f64) -> DownloadGuide {
    let ollama_model = model_to_ollama_id(model);
    DownloadGuide {
        server: InferenceServer::Ollama,
        model_id: ollama_model.clone(),
        download_size_gb: size_gb,
        install_steps: vec![
            "Install Ollama:".to_string(),
            "  Linux/Mac: curl -fsSL https://ollama.com/install.sh | sh".to_string(),
            "  Windows: Download from https://ollama.com/download".to_string(),
            format!("Download the model: ollama pull {}", ollama_model),
            format!("Ollama will automatically serve it on port 11434."),
            "Syntaur will auto-detect it — no manual config needed.".to_string(),
        ],
    }
}

fn llamacpp_download(model: &str, quant: &str, size_gb: f64) -> DownloadGuide {
    let hf_url = model_to_hf_url(model, quant);
    DownloadGuide {
        server: InferenceServer::LlamaCpp,
        model_id: format!("{}.{}.gguf", model, quant),
        download_size_gb: size_gb,
        install_steps: vec![
            format!("Download the model from HuggingFace:"),
            format!("  {}", hf_url),
            "Install llama.cpp:".to_string(),
            "  git clone https://github.com/ggml-org/llama.cpp && cd llama.cpp".to_string(),
            "  cmake -B build -DGGML_CUDA=ON && cmake --build build -j".to_string(),
            format!(
                "Start the server: ./build/bin/llama-server -m {}.{}.gguf --port 1235 -ngl 99",
                model, quant
            ),
            "Syntaur will auto-detect it on port 1235.".to_string(),
        ],
    }
}

fn model_to_ollama_id(model: &str) -> String {
    match model {
        "Qwen3-72B" => "qwen3:72b-q4_K_M".to_string(),
        "Qwen3-32B" => "qwen3:32b-q4_K_M".to_string(),
        "Qwen3-14B" => "qwen3:14b".to_string(),
        "Qwen3-8B" => "qwen3:8b".to_string(),
        "Qwen3-4B" => "qwen3:4b".to_string(),
        "Qwen3-1.7B" => "qwen3:1.7b".to_string(),
        _ => format!("qwen3:8b"),
    }
}

fn model_to_hf_url(model: &str, quant: &str) -> String {
    let repo = match model {
        "Qwen3-72B" => "Qwen/Qwen3-72B-GGUF",
        "Qwen3-32B" => "Qwen/Qwen3-32B-GGUF",
        "Qwen3-14B" => "Qwen/Qwen3-14B-GGUF",
        "Qwen3-8B" => "Qwen/Qwen3-8B-GGUF",
        "Qwen3-4B" => "Qwen/Qwen3-4B-GGUF",
        "Qwen3-1.7B" => "Qwen/Qwen3-1.7B-GGUF",
        _ => "Qwen/Qwen3-8B-GGUF",
    };
    format!("https://huggingface.co/{}", repo)
}

// ── Main recommendation logic ───────────────────────────────────────────

/// Recommend the best LLM setup for this hardware.
pub fn recommend_llm_setup(hw: &HardwareScan, net: &NetworkScan) -> LlmRecommendation {
    let best_gpu = hw.gpus.iter()
        .filter(|g| g.inference_capable)
        .max_by_key(|g| effective_vram(g));

    let network_service = net.llm_services.iter()
        .find(|s| !s.models.is_empty()); // prefer services with actual models loaded

    let network_any = net.llm_services.first();

    let vram = best_gpu.map(|g| effective_vram(g)).unwrap_or(0);
    let tier = classify_tier(vram, hw.ram.available_mb);

    match tier {
        HardwareTier::Powerful => recommend_powerful(best_gpu.unwrap(), vram, network_service, network_any),
        HardwareTier::Capable => recommend_capable(best_gpu.unwrap(), vram, network_service, network_any),
        HardwareTier::Limited => recommend_limited(best_gpu.unwrap(), vram, network_service, network_any),
        HardwareTier::CpuOnly => recommend_cpu_only(hw, network_service, network_any),
        HardwareTier::Minimal => recommend_minimal(hw, network_service, network_any),
    }
}

fn classify_tier(vram_mb: u64, ram_mb: u64) -> HardwareTier {
    match vram_mb {
        v if v >= 16000 => HardwareTier::Powerful,
        v if v >= 8000 => HardwareTier::Capable,
        v if v >= 4000 => HardwareTier::Limited,
        _ => {
            if ram_mb >= 8000 {
                HardwareTier::CpuOnly
            } else {
                HardwareTier::Minimal
            }
        }
    }
}

fn recommend_powerful(
    gpu: &crate::gpu::GpuInfo, vram: u64,
    net_with_models: Option<&crate::network::LlmService>,
    net_any: Option<&crate::network::LlmService>,
) -> LlmRecommendation {
    let (model, quant, est_vram, size_gb) = model_for_vram(vram);

    let mut steps = vec![
        SetupStep {
            title: "Install Ollama".to_string(),
            description: "Ollama is the easiest way to run local models. One command install.".to_string(),
            command: Some("curl -fsSL https://ollama.com/install.sh | sh".to_string()),
            url: Some("https://ollama.com".to_string()),
            automatable: true,
        },
        SetupStep {
            title: format!("Download {}", model),
            description: format!("This will download ~{:.1} GB. Your {} has plenty of VRAM.", size_gb, gpu.name),
            command: Some(format!("ollama pull {}", model_to_ollama_id(&model))),
            url: None,
            automatable: true,
        },
    ];

    let mut fallbacks = Vec::new();
    if let Some(svc) = net_with_models {
        fallbacks.push(LlmBackend::NetworkService {
            url: svc.url.clone(), service_name: svc.name.clone(), models: svc.models.clone(),
        });
    }
    steps.push(SetupStep {
        title: "Set up a cloud API backup (recommended)".to_string(),
        description: "Even with a powerful GPU, a cloud fallback ensures you're never stuck if the local server is restarting or under load.".to_string(),
        command: None,
        url: Some("https://openrouter.ai/keys".to_string()),
        automatable: false,
    });
    fallbacks.push(openrouter_backend());

    LlmRecommendation {
        primary: LlmBackend::LocalGpu {
            model_suggestion: model.clone(),
            quantization: quant,
            estimated_vram_mb: est_vram,
            download: ollama_download(&model, "Q4_K_M", size_gb),
        },
        fallbacks,
        explanation: format!(
            "Your {} has {} GB VRAM — that's great for local AI. \
             You can run {} entirely on your GPU with full privacy and zero API costs. \
             Responses will be fast (30-60 tokens/sec).",
            gpu.name, vram / 1024, model
        ),
        hardware_tier: HardwareTier::Powerful,
        setup_steps: steps,
        upgrade_hint: None,
        resilience_note: RESILIENCE_LOCAL_PRIMARY.to_string(),
    }
}

fn recommend_capable(
    gpu: &crate::gpu::GpuInfo, vram: u64,
    net_with_models: Option<&crate::network::LlmService>,
    net_any: Option<&crate::network::LlmService>,
) -> LlmRecommendation {
    let (model, quant, est_vram, size_gb) = model_for_vram(vram);

    let steps = vec![
        SetupStep {
            title: "Install Ollama".to_string(),
            description: "One command to install the local inference server.".to_string(),
            command: Some("curl -fsSL https://ollama.com/install.sh | sh".to_string()),
            url: Some("https://ollama.com".to_string()),
            automatable: true,
        },
        SetupStep {
            title: format!("Download {}", model),
            description: format!("{:.1} GB download. Good balance of quality and speed for your GPU.", size_gb),
            command: Some(format!("ollama pull {}", model_to_ollama_id(&model))),
            url: None,
            automatable: true,
        },
        SetupStep {
            title: "Add a cloud fallback (recommended)".to_string(),
            description: "For complex tasks that benefit from a larger model, a cloud API fallback is helpful.".to_string(),
            command: None,
            url: Some("https://openrouter.ai/keys".to_string()),
            automatable: false,
        },
    ];

    let mut fallbacks = vec![openrouter_backend()];
    // Add a second cloud provider as deep backup
    fallbacks.push(openai_backend());

    LlmRecommendation {
        primary: LlmBackend::LocalGpu {
            model_suggestion: model.clone(),
            quantization: quant,
            estimated_vram_mb: est_vram,
            download: ollama_download(&model, "Q4_K_M", size_gb),
        },
        fallbacks,
        explanation: format!(
            "Your {} with {} GB VRAM can run {} locally — good for most tasks. \
             A cloud API fallback handles complex reasoning and ensures uptime. \
             OpenRouter has free models to get started.",
            gpu.name, vram / 1024, model
        ),
        hardware_tier: HardwareTier::Capable,
        setup_steps: steps,
        upgrade_hint: Some(format!(
            "For an even better experience, a GPU with 16+ GB VRAM (like an RTX 4070 Ti or used RTX 3090) \
             would let you run 14B-32B parameter models locally."
        )),
        resilience_note: RESILIENCE_LOCAL_PRIMARY.to_string(),
    }
}

fn recommend_limited(
    gpu: &crate::gpu::GpuInfo, vram: u64,
    net_with_models: Option<&crate::network::LlmService>,
    net_any: Option<&crate::network::LlmService>,
) -> LlmRecommendation {
    let (model, quant, est_vram, size_gb) = model_for_vram(vram);

    // For limited GPUs, cloud or network is actually the better primary
    if let Some(svc) = net_with_models {
        let local_backup = LlmBackend::LocalGpu {
            model_suggestion: model.clone(),
            quantization: quant.clone(),
            estimated_vram_mb: est_vram,
            download: ollama_download(&model, "Q4_K_M", size_gb),
        };
        return LlmRecommendation {
            primary: LlmBackend::NetworkService {
                url: svc.url.clone(), service_name: svc.name.clone(), models: svc.models.clone(),
            },
            fallbacks: vec![local_backup, openrouter_backend()],
            explanation: format!(
                "Your {} has {} GB VRAM — enough for small models, but we found {} on your network \
                 which will give you better results. Your GPU and a cloud API serve as backups.",
                gpu.name, vram / 1024, svc.name
            ),
            hardware_tier: HardwareTier::Limited,
            setup_steps: vec![
                SetupStep {
                    title: "Use the network LLM".to_string(),
                    description: format!("Syntaur detected {} — it will be configured automatically.", svc.name),
                    command: None, url: None, automatable: true,
                },
                SetupStep {
                    title: "Set up a cloud API backup".to_string(),
                    description: "If the network service goes down, a cloud API keeps you running.".to_string(),
                    command: None,
                    url: Some("https://openrouter.ai/keys".to_string()),
                    automatable: false,
                },
            ],
            upgrade_hint: Some(
                "For local-only inference, a GPU with 8+ GB VRAM (RTX 3060 12GB, ~$200 used) \
                 would be a big upgrade.".to_string()
            ),
            resilience_note: RESILIENCE_NETWORK_PRIMARY.to_string(),
        };
    }

    // Cloud primary with local GPU + second cloud as fallbacks
    let local_backup = LlmBackend::LocalGpu {
        model_suggestion: model.clone(),
        quantization: quant,
        estimated_vram_mb: est_vram,
        download: ollama_download(&model, "Q4_K_M", size_gb),
    };

    LlmRecommendation {
        primary: openrouter_backend(),
        fallbacks: vec![local_backup, openai_backend()],
        explanation: format!(
            "Your {} has {} GB VRAM — it can run {} for simple tasks, but a cloud API \
             will give much better results. OpenRouter has free models. Your GPU and a second \
             cloud provider serve as backups for maximum uptime.",
            gpu.name, vram / 1024, model
        ),
        hardware_tier: HardwareTier::Limited,
        setup_steps: vec![
            SetupStep {
                title: "Sign up for OpenRouter (free)".to_string(),
                description: "Create an account and get an API key. Free models available immediately.".to_string(),
                command: None,
                url: Some("https://openrouter.ai/keys".to_string()),
                automatable: false,
            },
            SetupStep {
                title: "Install Ollama as offline backup".to_string(),
                description: format!("Run {} locally when cloud is down or you're offline.", model),
                command: Some("curl -fsSL https://ollama.com/install.sh | sh".to_string()),
                url: None,
                automatable: true,
            },
            SetupStep {
                title: "Add a second cloud API (recommended)".to_string(),
                description: "A second provider like OpenAI ensures you're never stuck if one API has an outage.".to_string(),
                command: None,
                url: Some("https://platform.openai.com/api-keys".to_string()),
                automatable: false,
            },
        ],
        upgrade_hint: Some(
            "A GPU with 8+ GB VRAM (RTX 3060 12GB ~$200, RTX 3070 ~$250 used) would let you run \
             8B models locally with good speed. A used RTX 3090 (~$700) runs 32B models.".to_string()
        ),
        resilience_note: RESILIENCE_CLOUD_PRIMARY.to_string(),
    }
}

fn recommend_cpu_only(
    hw: &HardwareScan,
    net_with_models: Option<&crate::network::LlmService>,
    net_any: Option<&crate::network::LlmService>,
) -> LlmRecommendation {
    let ram = hw.ram.available_mb;

    // Network service is the best option for CPU-only machines
    if let Some(svc) = net_with_models {
        let (model, quant, est_ram, size_gb) = model_for_ram(ram);
        let cpu_backup = LlmBackend::LocalCpu {
            model_suggestion: model.clone(),
            quantization: quant,
            estimated_ram_mb: est_ram,
            download: ollama_download(&model, "Q4_K_M", size_gb),
        };
        return LlmRecommendation {
            primary: LlmBackend::NetworkService {
                url: svc.url.clone(), service_name: svc.name.clone(), models: svc.models.clone(),
            },
            fallbacks: vec![openrouter_backend(), cpu_backup],
            explanation: format!(
                "No GPU detected, but we found {} on your network — perfect! \
                 A cloud API and local CPU model serve as backups for maximum uptime.",
                svc.name
            ),
            hardware_tier: HardwareTier::CpuOnly,
            setup_steps: vec![
                SetupStep {
                    title: "Use the network LLM".to_string(),
                    description: format!("{} will be configured automatically.", svc.name),
                    command: None, url: None, automatable: true,
                },
                SetupStep {
                    title: "Set up a cloud API backup".to_string(),
                    description: "If the network service goes down, a cloud API keeps you running.".to_string(),
                    command: None,
                    url: Some("https://openrouter.ai/keys".to_string()),
                    automatable: false,
                },
            ],
            upgrade_hint: Some(
                "For fully local inference, add a GPU. Even a used RTX 3060 12GB (~$200) \
                 would let you run 8B models with good speed.".to_string()
            ),
            resilience_note: RESILIENCE_NETWORK_PRIMARY.to_string(),
        };
    }

    // CPU inference as deep fallback, cloud primary + second cloud backup
    let (model, quant, est_ram, size_gb) = model_for_ram(ram);
    let cpu_backup = LlmBackend::LocalCpu {
        model_suggestion: model.clone(),
        quantization: quant,
        estimated_ram_mb: est_ram,
        download: ollama_download(&model, "Q4_K_M", size_gb),
    };

    LlmRecommendation {
        primary: openrouter_backend(),
        fallbacks: vec![cpu_backup, openai_backend()],
        explanation: format!(
            "No GPU detected. Cloud API is recommended for the best experience. \
             A local {} on CPU serves as an offline backup (slow but works), and a second \
             cloud provider ensures you're covered during API outages.",
            model
        ),
        hardware_tier: HardwareTier::CpuOnly,
        setup_steps: vec![
            SetupStep {
                title: "Sign up for OpenRouter (free)".to_string(),
                description: "Get an API key for cloud models. Free tier includes tool-capable models.".to_string(),
                command: None,
                url: Some("https://openrouter.ai/keys".to_string()),
                automatable: false,
            },
            SetupStep {
                title: "Install Ollama for offline backup".to_string(),
                description: format!(
                    "Download {} ({:.1} GB) so you have a local fallback. Slow on CPU but works offline.",
                    model, size_gb
                ),
                command: Some("curl -fsSL https://ollama.com/install.sh | sh".to_string()),
                url: Some("https://ollama.com".to_string()),
                automatable: true,
            },
            SetupStep {
                title: "Add a second cloud API (recommended)".to_string(),
                description: "OpenAI or Anthropic as a second provider protects against single-API outages.".to_string(),
                command: None,
                url: Some("https://platform.openai.com/api-keys".to_string()),
                automatable: false,
            },
        ],
        upgrade_hint: Some(
            "For a great local experience, add a discrete GPU:\n\
             • Budget: RTX 3060 12GB (~$200 used) — runs 8B models at ~30 tok/s\n\
             • Mid-range: RTX 3070/4060 Ti (~$250-350) — runs 8-14B models\n\
             • Best value: RTX 3090 24GB (~$700 used) — runs 32B models\n\
             Or use another machine on your network running Ollama.".to_string()
        ),
        resilience_note: RESILIENCE_CLOUD_PRIMARY.to_string(),
    }
}

fn recommend_minimal(
    hw: &HardwareScan,
    net_with_models: Option<&crate::network::LlmService>,
    net_any: Option<&crate::network::LlmService>,
) -> LlmRecommendation {
    if let Some(svc) = net_with_models {
        return LlmRecommendation {
            primary: LlmBackend::NetworkService {
                url: svc.url.clone(), service_name: svc.name.clone(), models: svc.models.clone(),
            },
            fallbacks: vec![openrouter_backend(), openai_backend()],
            explanation: format!(
                "This machine doesn't have enough resources for local inference, \
                 but we found {} on your network. Two cloud APIs serve as backup \
                 in case the network service goes down.",
                svc.name
            ),
            hardware_tier: HardwareTier::Minimal,
            setup_steps: vec![
                SetupStep {
                    title: "Use the network LLM".to_string(),
                    description: format!("{} will be configured automatically.", svc.name),
                    command: None, url: None, automatable: true,
                },
                SetupStep {
                    title: "Set up cloud API backup".to_string(),
                    description: "A cloud API ensures Syntaur keeps working if the network LLM goes offline.".to_string(),
                    command: None,
                    url: Some("https://openrouter.ai/keys".to_string()),
                    automatable: false,
                },
            ],
            upgrade_hint: Some(
                "This machine has limited RAM and no GPU. For the best experience, \
                 run Syntaur on a machine with at least 8GB RAM, or use a cloud API.".to_string()
            ),
            resilience_note: RESILIENCE_NETWORK_PRIMARY.to_string(),
        };
    }

    // Minimal hardware, no network — cloud only, but ALWAYS two providers
    LlmRecommendation {
        primary: openrouter_backend(),
        fallbacks: vec![openai_backend()],
        explanation:
            "This machine doesn't have enough resources for local AI inference. \
             A cloud API is the best option — OpenRouter offers free models to get started. \
             A second cloud provider protects against outages. You can also run Ollama \
             on another machine on your network and Syntaur will detect it automatically."
                .to_string(),
        hardware_tier: HardwareTier::Minimal,
        setup_steps: vec![
            SetupStep {
                title: "Sign up for OpenRouter (free)".to_string(),
                description: "Create an account at openrouter.ai. Free tool-capable models available immediately.".to_string(),
                command: None,
                url: Some("https://openrouter.ai/keys".to_string()),
                automatable: false,
            },
            SetupStep {
                title: "Add a backup cloud API".to_string(),
                description: "OpenAI ($5-15/mo) as a second provider means you're never stuck during an outage.".to_string(),
                command: None,
                url: Some("https://platform.openai.com/api-keys".to_string()),
                automatable: false,
            },
            SetupStep {
                title: "Or: run Ollama on another machine".to_string(),
                description: "If you have a more powerful computer, install Ollama there. \
                              Syntaur will detect it on your network automatically.".to_string(),
                command: None,
                url: Some("https://ollama.com".to_string()),
                automatable: false,
            },
        ],
        upgrade_hint: Some(
            "For local inference, you need:\n\
             • Minimum: 8GB RAM (CPU inference, slow but works)\n\
             • Recommended: NVIDIA GPU with 8+ GB VRAM\n\
             • Best: RTX 3090 24GB (~$700 used) or Apple M-series Mac with 16+ GB".to_string()
        ),
        resilience_note: RESILIENCE_CLOUD_PRIMARY.to_string(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn effective_vram(gpu: &crate::gpu::GpuInfo) -> u64 {
    if gpu.vram_mb > 0 { gpu.vram_mb } else { gpu.shared_memory_mb }
}

/// Returns (model_name, quantization, estimated_vram_mb, download_size_gb).
fn model_for_vram(vram_mb: u64) -> (String, String, u64, f64) {
    match vram_mb {
        v if v >= 48000 => ("Qwen3-72B".into(), "Q4_K_M".into(), 42000, 42.0),
        v if v >= 24000 => ("Qwen3-32B".into(), "Q4_K_M".into(), 20000, 20.0),
        v if v >= 16000 => ("Qwen3-14B".into(), "Q4_K_M".into(), 10000, 9.5),
        v if v >= 12000 => ("Qwen3-8B".into(),  "Q4_K_M".into(), 6000,  5.5),
        v if v >= 8000  => ("Qwen3-8B".into(),  "Q4_K_S".into(), 5500,  5.0),
        v if v >= 6000  => ("Qwen3-4B".into(),  "Q4_K_M".into(), 3500,  2.8),
        _               => ("Qwen3-1.7B".into(),"Q4_K_M".into(), 1500,  1.2),
    }
}

/// Returns (model_name, quantization, estimated_ram_mb, download_size_gb).
fn model_for_ram(ram_mb: u64) -> (String, String, u64, f64) {
    match ram_mb {
        v if v >= 32000 => ("Qwen3-14B".into(), "Q4_K_M".into(), 10000, 9.5),
        v if v >= 16000 => ("Qwen3-8B".into(),  "Q4_K_M".into(), 6000,  5.5),
        v if v >= 8000  => ("Qwen3-4B".into(),  "Q4_K_M".into(), 3500,  2.8),
        _               => ("Qwen3-1.7B".into(),"Q4_K_S".into(), 1200,  1.0),
    }
}
