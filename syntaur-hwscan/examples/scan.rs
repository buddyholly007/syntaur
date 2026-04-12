use syntaur_hwscan;
use syntaur_hwscan::recommend::*;

#[tokio::main]
async fn main() {
    println!("=== Syntaur Hardware Scan ===\n");
    let (hw, net, rec) = syntaur_hwscan::full_scan().await;

    // Hardware summary
    println!("CPU: {} ({} cores / {} threads)", hw.cpu.model, hw.cpu.cores, hw.cpu.threads);
    println!("RAM: {:.1} GB total, {:.1} GB available",
        hw.ram.total_mb as f64 / 1024.0, hw.ram.available_mb as f64 / 1024.0);

    if hw.gpus.is_empty() {
        println!("GPU: None detected");
    } else {
        for gpu in &hw.gpus {
            let mem = if gpu.vram_mb > 0 {
                format!("{:.1} GB VRAM", gpu.vram_mb as f64 / 1024.0)
            } else if gpu.shared_memory_mb > 0 {
                format!("{:.1} GB shared", gpu.shared_memory_mb as f64 / 1024.0)
            } else {
                "unknown".to_string()
            };
            println!("GPU: {} {} ({})", gpu.vendor, gpu.name, mem);
        }
    }

    if !net.llm_services.is_empty() {
        println!("\nNetwork LLM services found:");
        for svc in &net.llm_services {
            let models = if svc.models.is_empty() {
                String::new()
            } else {
                format!(" [{}]", svc.models.join(", "))
            };
            println!("  {} — {}{}", svc.service_type, svc.url, models);
        }
    }

    // Recommendation
    println!("\n=== Recommendation ===");
    println!("Hardware tier: {}\n", rec.hardware_tier);
    println!("{}\n", rec.explanation);

    println!("Primary: {}", format_backend(&rec.primary));
    if !rec.fallbacks.is_empty() {
        println!("Fallbacks ({}):", rec.fallbacks.len());
        for (i, fb) in rec.fallbacks.iter().enumerate() {
            println!("  {}. {}", i + 1, format_backend(fb));
        }
    }

    println!("\n--- Resilience ---");
    println!("{}", rec.resilience_note);

    if let Some(hint) = &rec.upgrade_hint {
        println!("\n--- Upgrade suggestion ---");
        println!("{}", hint);
    }

    println!("\n=== Setup Steps ===\n");
    for (i, step) in rec.setup_steps.iter().enumerate() {
        let auto = if step.automatable { " [auto]" } else { "" };
        println!("{}. {}{}", i + 1, step.title, auto);
        println!("   {}", step.description);
        if let Some(cmd) = &step.command {
            println!("   $ {}", cmd);
        }
        if let Some(url) = &step.url {
            println!("   -> {}", url);
        }
        println!();
    }

    // Show download info if applicable
    match &rec.primary {
        LlmBackend::LocalGpu { download, .. } | LlmBackend::LocalCpu { download, .. } => {
            println!("=== Download Guide ===\n");
            println!("Server: {}", download.server);
            println!("Model: {}", download.model_id);
            println!("Size: {:.1} GB\n", download.download_size_gb);
            for step in &download.install_steps {
                println!("  {}", step);
            }
        }
        _ => {}
    }
}

fn format_backend(b: &LlmBackend) -> String {
    match b {
        LlmBackend::LocalGpu { model_suggestion, quantization, estimated_vram_mb, .. } =>
            format!("Local GPU: {} {} (~{:.1} GB VRAM)", model_suggestion, quantization, *estimated_vram_mb as f64 / 1024.0),
        LlmBackend::LocalCpu { model_suggestion, quantization, estimated_ram_mb, .. } =>
            format!("Local CPU: {} {} (~{:.1} GB RAM)", model_suggestion, quantization, *estimated_ram_mb as f64 / 1024.0),
        LlmBackend::NetworkService { service_name, url, .. } =>
            format!("Network: {} at {}", service_name, url),
        LlmBackend::CloudApi { provider, suggested_model, has_free_tier, .. } => {
            let free = if *has_free_tier { " (free tier)" } else { "" };
            format!("Cloud: {} — {}{}", provider, suggested_model, free)
        }
    }
}
