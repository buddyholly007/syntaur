use syntaur_setup::*;

fn main() {
    let choices = InstallChoices {
        agent_name: "Atlas".to_string(),
        user_name: "Sean".to_string(),
        llm_primary: LlmChoice {
            backend_type: LlmBackendType::OpenRouter,
            url: None,
            api_key: Some("sk-or-test-key".to_string()),
            model: Some("nvidia/llama-3.3-nemotron-super-49b-v1:free".to_string()),
        },
        llm_fallbacks: vec![
            LlmChoice {
                backend_type: LlmBackendType::Ollama,
                url: Some("http://127.0.0.1:11434".to_string()),
                api_key: None,
                model: Some("qwen3:4b".to_string()),
            },
            LlmChoice {
                backend_type: LlmBackendType::OpenAi,
                url: None,
                api_key: Some("sk-openai-test".to_string()),
                model: Some("gpt-4o-mini".to_string()),
            },
        ],
        voice_enabled: true,
        tts_engine: Some("piper".to_string()),
        tts_voice: Some("en_US-lessac-medium".to_string()),
        stt_engine: Some("whisper-cpu".to_string()),
        wake_word: Some("hey atlas".to_string()),
        telegram_token: Some("123456:ABC-test".to_string()),
        telegram_chat_id: Some(12345678),
        smart_home_enabled: true,
        ha_url: Some("http://192.168.1.3:8123".to_string()),
        ha_token: Some("eyJ-test-token".to_string()),
        enabled_modules: vec![],
        disabled_modules: vec!["mod-captcha".to_string(), "mod-accounts".to_string()],
        admin_username: "admin".to_string(),
        admin_password: "test-password-123".to_string(),
        data_dir: "/tmp/syntaur-test".into(),
        conversation_retention_days: Some(90),
        telemetry: false,
        gateway_port: 18789,
        timezone: "America/Los_Angeles".to_string(),
    };

    match syntaur_setup::generate(&choices) {
        Ok(()) => {
            println!("Generated successfully!\n");

            // Show what was created
            let base = std::path::Path::new("/tmp/syntaur-test");
            println!("=== syntaur.json ===");
            println!("{}", std::fs::read_to_string(base.join("syntaur.json")).unwrap());

            println!("\n=== Workspace files ===");
            for entry in std::fs::read_dir(base.join("workspace-atlas")).unwrap() {
                let entry = entry.unwrap();
                println!("  {}", entry.file_name().to_string_lossy());
            }

            println!("\n=== AGENTS.md (first 30 lines) ===");
            let agents = std::fs::read_to_string(base.join("workspace-atlas/AGENTS.md")).unwrap();
            for line in agents.lines().take(30) {
                println!("{}", line);
            }

            println!("\n=== MEMORY.md ===");
            println!("{}", std::fs::read_to_string(base.join("workspace-atlas/MEMORY.md")).unwrap());
        }
        Err(e) => eprintln!("Error: {:#}", e),
    }
}
