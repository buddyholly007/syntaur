//! Syntaur — Multi-agent AI platform with license management and voice.
//!
//! Architecture:
//! - Orchestrator: routes user requests to the right agents
//! - Major agents: top-level agents (assistant) with sub-agent trees
//! - Sub-agents: specialized workers (search, coder, researcher)
//! - Backend router: load-aware routing across local/cloud AI backends
//! - Conversation store: SQLite persistence with context budgeting
//! - Voice endpoint: OpenAI-compatible /v1/chat/completions with lean prompts
//! - License server: Stripe checkout + Ed25519 license generation

mod agent;
mod api;
mod backend;
pub mod config;
mod conversation;
mod devices;
mod license;
mod orchestrator;
mod task;
mod voice;

use std::sync::Arc;

use axum::Router;
use log::info;

use agent::builtin::{assistant, coder, researcher, search};
use agent::registry::AgentRegistry;
use api::ApiState;
use backend::cloud::CloudBackend;
use backend::local::LocalBackend;
use backend::router::BackendRouter;
use config::{BackendProvider, PlatformConfig};
use conversation::ConversationStore;
use license::LicenseState;
use orchestrator::Orchestrator;
use voice::VoiceState;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = PlatformConfig::from_env();

    info!("Syntaur platform starting on port {}", config.server.port);

    // ── Data directory ──────────────────────────────────────────────────
    let data_dir = {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
        let dir = format!("{}/.syntaur-license-server", home);
        std::fs::create_dir_all(&dir).ok();
        dir
    };

    // ── Backends ────────────────────────────────────────────────────────

    let backend_router = Arc::new(BackendRouter::new());

    for bc in &config.backends {
        let backend: Arc<dyn backend::Backend> = match bc.provider {
            BackendProvider::Local => Arc::new(LocalBackend::new(
                bc.id.clone(),
                bc.url.clone(),
                bc.model.clone(),
                bc.max_tokens,
                bc.tags.clone(),
            )),
            _ => Arc::new(CloudBackend::new(
                bc.id.clone(),
                bc.provider.clone(),
                bc.url.clone(),
                bc.api_key.clone().unwrap_or_default(),
                bc.model.clone(),
                bc.max_tokens,
                bc.tags.clone(),
            )),
        };
        backend_router.add_backend(backend).await;
    }

    info!(
        "{} backend(s) registered",
        backend_router.backend_count().await
    );

    // Start background health checks
    let health_router = backend_router.clone();
    tokio::spawn(async move {
        health_router.run_health_loop().await;
    });

    // ── Agents ──────────────────────────────────────────────────────────

    let mut registry = AgentRegistry::new();
    registry.register(Arc::new(assistant::AssistantAgent::new()));
    registry.register(Arc::new(search::SearchAgent::new()));
    registry.register(Arc::new(coder::CoderAgent::new()));
    registry.register(Arc::new(researcher::ResearcherAgent::new()));

    info!("{} agent(s) registered", registry.list().len());

    // ── Orchestrator ────────────────────────────────────────────────────

    let orchestrator = Arc::new(Orchestrator::new(
        registry,
        backend_router.clone(),
        config.executor.clone(),
        config.agents.default_major_agent.clone(),
    ));

    // ── Conversation store ──────────────────────────────────────────────

    let conversations = Arc::new(ConversationStore::open(&data_dir));

    // Background purge task (every 5 minutes, clean up expired conversations)
    let purge_store = conversations.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            purge_store
                .purge_expired(std::time::Duration::from_secs(3600))
                .await;
        }
    });

    // ── HTTP Server ─────────────────────────────────────────────────────

    // ── Device registry ��──────────────────────────────────────────────

    let device_registry = Arc::new(devices::registry::DeviceRegistry::open(&data_dir));

    // Start mDNS reflector if multiple interfaces detected (VLAN environments)
    if let Some(stats) = devices::mdns_reflector::start_if_needed() {
        info!("mDNS reflector active (cross-VLAN discovery)");
    }

    // ── HTTP state ──────────────────────────────────────────────────────

    let profile_store = Arc::new(conversation::profile::ProfileStore::open(&data_dir));

    let api_state = Arc::new(ApiState {
        orchestrator: orchestrator.clone(),
        conversations: conversations.clone(),
        devices: device_registry.clone(),
        profile: profile_store.clone(),
    });

    let voice_state = Arc::new(VoiceState {
        backend_router: backend_router.clone(),
        voice_secret: std::env::var("VOICE_SECRET").ok(),
    });

    let voice_io_config = voice::io::VoiceIoConfig::from_env();
    let has_stt = voice_io_config.has_stt();
    let has_tts = voice_io_config.has_tts();
    let voice_io_state = Arc::new(voice::io::VoiceIoState {
        config: voice_io_config,
        client: reqwest::Client::new(),
        tts_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
    });
    info!(
        "Voice I/O: stt={} tts={}",
        if has_stt { "available" } else { "not configured" },
        if has_tts { "available" } else { "not configured" },
    );

    let license_state = LicenseState::new(config.license.clone(), config.server.server_url.clone());

    // Compose routes
    let app = Router::new()
        // Agent API
        .nest("/api/v1", api::agent_routes().with_state(api_state))
        // OpenAI-compatible voice endpoint
        .route(
            "/v1/chat/completions",
            axum::routing::post(voice::handle_voice_chat).with_state(voice_state),
        )
        // Voice I/O (STT, TTS, round-trip)
        .route(
            "/api/v1/stt",
            axum::routing::post(voice::io::handle_stt).with_state(voice_io_state.clone()),
        )
        .route(
            "/api/v1/tts",
            axum::routing::post(voice::io::handle_tts).with_state(voice_io_state.clone()),
        )
        .route(
            "/api/v1/tts/audio/{filename}",
            axum::routing::get(voice::io::handle_tts_audio).with_state(voice_io_state.clone()),
        )
        // Voice browser UI
        .route("/voice", axum::routing::get(voice::ui::handle_voice_ui))
        // License server
        .merge(license::license_routes().with_state(license_state))
        // Health
        .route("/health", axum::routing::get(|| async { "ok" }));

    let addr = format!("0.0.0.0:{}", config.server.port);
    info!("Syntaur platform listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
