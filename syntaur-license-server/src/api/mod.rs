use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use log::info;
use serde::{Deserialize, Serialize};

use crate::backend::stream::{stream_completion, StreamChunk};
use crate::config::BackendProvider;
use crate::conversation::profile::{ProfileStore, UserProfile};
use crate::conversation::{trim_to_budget, ContextBudget, ConversationStore};
use crate::devices::registry::DeviceRegistry;
use crate::devices::{Device, DeviceCommand};
use crate::orchestrator::Orchestrator;
use crate::task::{Message, TaskCategory, TaskPayload};

/// Shared state for the agent API.
pub struct ApiState {
    pub orchestrator: Arc<Orchestrator>,
    pub conversations: Arc<ConversationStore>,
    pub devices: Arc<DeviceRegistry>,
    pub profile: Arc<ProfileStore>,
}

pub fn agent_routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/chat", post(handle_chat))
        .route("/chat/stream", post(handle_chat_stream))
        .route("/plan", post(handle_plan))
        .route("/conversations", post(handle_create_conversation))
        .route("/conversations", get(handle_list_conversations))
        .route("/conversations/{id}", get(handle_get_conversation))
        .route("/conversations/search", post(handle_search_conversations))
        .route("/profile", get(handle_get_profile).post(handle_update_profile))
        // Device management
        .route("/devices", get(handle_list_devices))
        .route("/devices", post(handle_register_device))
        .route("/devices/{id}", axum::routing::delete(handle_remove_device))
        .route("/devices/{id}/command", post(handle_device_command))
        .route("/devices/rooms", get(handle_list_rooms))
        .route("/devices/rooms/{room}/command", post(handle_room_command))
        .route("/devices/discover", post(handle_discover_devices))
        .route("/devices/matter/nodes", get(handle_matter_nodes))
        .route("/devices/matter/import", post(handle_matter_import))
        .route("/agents", get(handle_list_agents))
        .route("/backends", get(handle_list_backends))
        .route("/health", get(handle_health))
}

// ── Request/Response types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    history: Vec<MessageDto>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
struct MessageDto {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatResponse {
    content: String,
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_title: Option<String>,
    agent_id: String,
    backend_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<TokensDto>,
    duration_ms: u64,
    /// Sub-agents that were invoked during this request.
    #[serde(skip_serializing_if = "Option::is_none")]
    agents_used: Option<Vec<AgentActivity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_results: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
struct AgentActivity {
    agent: String,
    summary: String,
    duration_ms: u64,
}

#[derive(Serialize)]
struct TokensDto {
    prompt: u32,
    completion: u32,
    total: u32,
}

#[derive(Deserialize)]
struct PlanRequest {
    instruction: String,
    #[serde(default)]
    history: Vec<MessageDto>,
    #[serde(default = "default_true")]
    execute: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
struct PlanResponse {
    summary: String,
    steps: Vec<PlanStepDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<PlanExecutionResultDto>,
}

#[derive(Serialize)]
struct PlanStepDto {
    description: String,
    category: String,
    agent_id: Option<String>,
    parallel_group: Option<usize>,
}

#[derive(Serialize)]
struct PlanExecutionResultDto {
    content: String,
    steps_completed: usize,
    agents_used: Vec<String>,
    duration_ms: u64,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    backends: usize,
    agents: usize,
    active_tasks: usize,
}

#[derive(Deserialize)]
struct CreateConversationRequest {
    #[serde(default = "default_category")]
    category: String,
}

fn default_category() -> String {
    "conversation".into()
}

// ── Handlers ────────────────────────────────────────────────────────────

fn parse_category(s: &str) -> TaskCategory {
    match s {
        "search" => TaskCategory::Search,
        "coding" | "code" => TaskCategory::Coding,
        "research" => TaskCategory::Research,
        "planning" => TaskCategory::Planning,
        "voice" => TaskCategory::Conversation, // voice uses conversation + special budget
        _ => TaskCategory::Conversation,
    }
}

async fn handle_chat(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, String)> {
    if req.message.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message must not be empty".into()));
    }

    let category = req
        .category
        .as_deref()
        .map(parse_category)
        .unwrap_or(TaskCategory::Conversation);

    let budget = ContextBudget::for_category(&category);

    // Load conversation history — from server-side store if conversation_id given,
    // otherwise from client-provided history
    let mut conv_id = req.conversation_id.clone();
    let mut messages: Vec<Message> = if let Some(ref cid) = conv_id {
        // Server-side conversation
        state.conversations.messages(cid).await
    } else if !req.history.is_empty() {
        // Client-provided history
        req.history
            .iter()
            .map(|m| match m.role.as_str() {
                "system" => Message::system(&m.content),
                "assistant" => Message::assistant(&m.content),
                _ => Message::user(&m.content),
            })
            .collect()
    } else {
        Vec::new()
    };

    // Trim to budget
    messages = trim_to_budget(&messages, &budget);

    // Auto-create conversation if none provided
    if conv_id.is_none() {
        let cid = state
            .conversations
            .create(&format!("{}", category))
            .await;
        conv_id = Some(cid);
    }

    // Save user message to conversation
    if let Some(ref cid) = conv_id {
        state.conversations.append(cid, "user", &req.message).await;
    }

    // Inject user profile context into the task so the system prompt knows who's talking
    let profile = state.profile.get().await;
    let mut task = TaskPayload::new(category, &req.message)
        .with_messages(messages);
    if let Some(ctx) = profile.as_context() {
        task.metadata.insert("user_context".into(), ctx);
    }

    // Auto-title on first user message in a new conversation
    if let Some(ref cid) = conv_id {
        let conv = state.conversations.get(cid).await;
        if conv.as_ref().and_then(|c| c.title.as_ref()).is_none() {
            let title = ConversationStore::auto_title(&req.message);
            state.conversations.set_title(cid, &title).await;
        }
    }

    // Create event channel to capture sub-agent activity
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);

    let (result, _) = state
        .orchestrator
        .submit_with_events(task, Some(event_tx))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Collect any events that were emitted
    let mut activities: Vec<AgentActivity> = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        match event {
            crate::task::TaskEvent::AgentDone { agent_id, duration_ms } => {
                // Find the matching start event's summary
                let summary = activities
                    .iter()
                    .rev()
                    .find(|a| a.agent == agent_id && a.duration_ms == 0)
                    .map(|a| a.summary.clone())
                    .unwrap_or_default();
                if let Some(a) = activities.iter_mut().rev().find(|a| a.agent == agent_id && a.duration_ms == 0) {
                    a.duration_ms = duration_ms;
                }
            }
            crate::task::TaskEvent::AgentStart { agent_id, task_summary } => {
                activities.push(AgentActivity {
                    agent: agent_id,
                    summary: task_summary,
                    duration_ms: 0,
                });
            }
            _ => {}
        }
    }

    let content = result.output_text().unwrap_or("").to_string();

    if let Some(ref cid) = conv_id {
        state.conversations.append(cid, "assistant", &content).await;
    }

    let sub_results = result
        .output
        .get("sub_results")
        .and_then(|v| v.as_array())
        .cloned();

    let tokens = result.tokens_used.as_ref().map(|t| TokensDto {
        prompt: t.prompt_tokens,
        completion: t.completion_tokens,
        total: t.total_tokens,
    });

    // Get title for response
    let title = if let Some(ref cid) = conv_id {
        state.conversations.get(cid).await.and_then(|c| c.title)
    } else {
        None
    };

    Ok(Json(ChatResponse {
        content,
        conversation_id: conv_id,
        conversation_title: title,
        agent_id: result.agent_id,
        backend_id: result.backend_id,
        tokens,
        duration_ms: result.duration.as_millis() as u64,
        agents_used: if activities.is_empty() { None } else { Some(activities) },
        sub_results,
    }))
}

/// SSE streaming chat — routes through full orchestrator pipeline with
/// interleaved status events (thinking, agent_start, agent_done) and
/// token deltas from the backend.
async fn handle_chat_stream(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    if req.message.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message must not be empty".into()));
    }

    let category = req
        .category
        .as_deref()
        .map(parse_category)
        .unwrap_or(TaskCategory::Conversation);
    let budget = ContextBudget::for_category(&category);

    // Load conversation history
    let mut conv_id = req.conversation_id.clone();
    let mut messages: Vec<Message> = if let Some(ref cid) = conv_id {
        state.conversations.messages(cid).await
    } else if !req.history.is_empty() {
        req.history
            .iter()
            .map(|m| match m.role.as_str() {
                "system" => Message::system(&m.content),
                "assistant" => Message::assistant(&m.content),
                _ => Message::user(&m.content),
            })
            .collect()
    } else {
        Vec::new()
    };

    messages = trim_to_budget(&messages, &budget);

    // Auto-create conversation + save user message
    if conv_id.is_none() {
        let cid = state.conversations.create(&format!("{}", category)).await;
        conv_id = Some(cid);
    }
    if let Some(ref cid) = conv_id {
        state.conversations.append(cid, "user", &req.message).await;
        let conv = state.conversations.get(cid).await;
        if conv.as_ref().and_then(|c| c.title.as_ref()).is_none() {
            let title = ConversationStore::auto_title(&req.message);
            state.conversations.set_title(cid, &title).await;
        }
    }

    // Build system prompt with user profile (same as non-streaming)
    let profile = state.profile.get().await;
    let user_context = profile.as_context();

    // Build the assistant's system prompt with time + user context
    let now = chrono::Local::now();
    let time_str = now.format("%I:%M %p").to_string();
    let date_str = now.format("%A, %B %e, %Y").to_string();
    let mut system_prompt = String::from(
        "You are Syntaur, an intelligent AI assistant that is genuinely helpful, \
         precise, and adapts to how each person communicates.\n\n\
         RULES:\n\
         - Be direct. Lead with the answer, not the reasoning.\n\
         - Match the user's tone.\n\
         - If a task needs specialized work, indicate [search], [code], or [research].\n\
         - If you don't know something, say so.\n\n",
    );
    if let Some(ref ctx) = user_context {
        system_prompt.push_str(&format!("About the user: {}\n\n", ctx));
    }
    system_prompt.push_str(&format!("Current time: {} on {}.\n", time_str, date_str));

    // Add the user message to the message list
    messages.push(Message::user(&req.message));

    // Resolve best backend for streaming
    let platform_backends = crate::config::PlatformConfig::from_env().backends;
    let backend_infos = state.orchestrator.backend_router.list_backends().await;
    let best_backend = backend_infos
        .first()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "no backends available".into()))?;
    let bc = platform_backends
        .iter()
        .find(|b| b.id == best_backend.id)
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "backend config not found".into()))?
        .clone();

    // Start token stream
    let client = reqwest::Client::new();
    let mut rx = stream_completion(
        &client,
        &bc.url,
        bc.api_key.as_deref().unwrap_or(""),
        &bc.provider,
        &bc.model,
        &messages,
        Some(&system_prompt),
        budget.max_gen_tokens,
        0.7,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let conv_id_clone = conv_id.clone();
    let conversations = state.conversations.clone();

    // Create SSE stream with status events interleaved
    let stream = async_stream::stream! {
        // Emit initial thinking status
        let status = serde_json::json!({"type": "status", "message": "Thinking..."});
        yield Ok(Event::default().data(status.to_string()));

        let mut full_content = String::new();
        let mut first_token = true;

        while let Some(chunk) = rx.recv().await {
            match chunk {
                StreamChunk::Delta(text) => {
                    if first_token {
                        // Emit status clear on first real token
                        let status = serde_json::json!({"type": "status", "message": ""});
                        yield Ok(Event::default().data(status.to_string()));
                        first_token = false;
                    }
                    full_content.push_str(&text);
                    let data = serde_json::json!({"type": "delta", "content": text});
                    yield Ok(Event::default().data(data.to_string()));
                }
                StreamChunk::Done(reason) => {
                    // Save to conversation
                    if let Some(ref cid) = conv_id_clone {
                        conversations.append(cid, "assistant", &full_content).await;
                    }

                    let data = serde_json::json!({
                        "type": "done",
                        "content": full_content,
                        "conversation_id": conv_id_clone,
                        "finish_reason": reason,
                    });
                    yield Ok(Event::default().data(data.to_string()));
                    return;
                }
                StreamChunk::Error(e) => {
                    let data = serde_json::json!({"type": "error", "error": e});
                    yield Ok(Event::default().data(data.to_string()));
                    return;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn handle_plan(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<PlanRequest>,
) -> Result<Json<PlanResponse>, (StatusCode, String)> {
    let messages: Vec<Message> = req
        .history
        .iter()
        .map(|m| match m.role.as_str() {
            "system" => Message::system(&m.content),
            "assistant" => Message::assistant(&m.content),
            _ => Message::user(&m.content),
        })
        .collect();

    let orchestration = state
        .orchestrator
        .plan_and_execute(&req.instruction, messages)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let steps: Vec<PlanStepDto> = orchestration
        .plan
        .steps
        .iter()
        .map(|s| PlanStepDto {
            description: s.description.clone(),
            category: format!("{}", s.category),
            agent_id: s.agent_id.clone(),
            parallel_group: s.parallel_group,
        })
        .collect();

    let result = if req.execute {
        let content = orchestration
            .final_output
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let agents_used: Vec<String> = orchestration
            .step_results
            .iter()
            .map(|r| r.agent_id.clone())
            .collect();

        Some(PlanExecutionResultDto {
            content,
            steps_completed: orchestration.step_results.len(),
            agents_used,
            duration_ms: orchestration.total_duration.as_millis() as u64,
        })
    } else {
        None
    };

    Ok(Json(PlanResponse {
        summary: orchestration.plan.summary,
        steps,
        result,
    }))
}

async fn handle_create_conversation(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateConversationRequest>,
) -> Json<serde_json::Value> {
    let id = state.conversations.create(&req.category).await;
    Json(serde_json::json!({ "id": id, "category": req.category }))
}

async fn handle_list_conversations(
    State(state): State<Arc<ApiState>>,
) -> Json<Vec<crate::conversation::Conversation>> {
    Json(state.conversations.list(50).await)
}

async fn handle_get_conversation(
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conv = state
        .conversations
        .get(&id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let messages = state.conversations.messages(&id).await;
    let msgs: Vec<MessageDto> = messages
        .iter()
        .map(|m| MessageDto {
            role: match m.role {
                crate::task::MessageRole::System => "system".into(),
                crate::task::MessageRole::User => "user".into(),
                crate::task::MessageRole::Assistant => "assistant".into(),
            },
            content: m.content.clone(),
        })
        .collect();
    Ok(Json(serde_json::json!({
        "conversation": conv,
        "messages": msgs,
    })))
}

async fn handle_list_agents(
    State(state): State<Arc<ApiState>>,
) -> Json<Vec<crate::agent::registry::AgentInfo>> {
    Json(state.orchestrator.registry.list())
}

async fn handle_list_backends(
    State(state): State<Arc<ApiState>>,
) -> Json<Vec<crate::backend::router::BackendInfo>> {
    Json(state.orchestrator.backend_router.list_backends().await)
}

async fn handle_health(
    State(state): State<Arc<ApiState>>,
) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        backends: state.orchestrator.backend_router.backend_count().await,
        agents: state.orchestrator.registry.list().len(),
        active_tasks: state.orchestrator.active_task_count(),
    })
}

// ── Device endpoints ────────────────────────────────────────────────────

async fn handle_list_devices(
    State(state): State<Arc<ApiState>>,
) -> Json<Vec<Device>> {
    Json(state.devices.list().await)
}

async fn handle_register_device(
    State(state): State<Arc<ApiState>>,
    Json(device): Json<Device>,
) -> Json<serde_json::Value> {
    state.devices.register(&device).await;
    Json(serde_json::json!({"status": "registered", "id": device.id}))
}

async fn handle_remove_device(
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let removed = state.devices.remove(&id).await;
    Json(serde_json::json!({"removed": removed}))
}

#[derive(Deserialize)]
struct DeviceCommandRequest {
    #[serde(flatten)]
    command: DeviceCommand,
}

async fn handle_device_command(
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<DeviceCommandRequest>,
) -> Json<crate::devices::DeviceState> {
    Json(state.devices.execute(&id, &req.command).await)
}

async fn handle_list_rooms(
    State(state): State<Arc<ApiState>>,
) -> Json<Vec<String>> {
    Json(state.devices.rooms().await)
}

async fn handle_room_command(
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(room): axum::extract::Path<String>,
    Json(req): Json<DeviceCommandRequest>,
) -> Json<Vec<crate::devices::DeviceState>> {
    Json(state.devices.execute_room(&room, &req.command).await)
}

/// Scan the network for smart home devices.
async fn handle_discover_devices(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<DiscoverRequest>,
) -> Json<DiscoverResponse> {
    let timeout = req.timeout_secs.unwrap_or(5);

    // Phase 1: mDNS scan for WiFi devices
    let mut found = crate::devices::discovery::scan_network(timeout).await;

    // Phase 2: MQTT scan for Zigbee/Z-Wave (if broker configured)
    if let Some(ref broker) = req.mqtt_broker {
        let mqtt_devices = crate::devices::discovery::detect_mqtt_services(broker).await;
        found.extend(mqtt_devices);
    } else if let Ok(broker) = std::env::var("MQTT_URL") {
        let mqtt_devices = crate::devices::discovery::detect_mqtt_services(&broker).await;
        found.extend(mqtt_devices);
    }

    // Check which are already registered
    let registered = state.devices.list().await;
    let registered_ips: Vec<&str> = registered.iter().map(|d| d.endpoint.as_str()).collect();

    let mut new_devices = Vec::new();
    let mut existing = Vec::new();
    for d in &found {
        let endpoint = format!("http://{}:{}", d.ip, d.port);
        if registered_ips.iter().any(|e| e.contains(&d.ip)) {
            existing.push(d.clone());
        } else {
            new_devices.push(d.clone());
        }
    }

    // Warn about mDNS reflector conflicts if multi-interface
    let warnings = if std::env::var("MDNS_REFLECT").map(|v| v == "true" || v == "1").unwrap_or(false)
        || found.iter().any(|d| d.service_type.contains("matter"))
    {
        Some(vec![
            "If your router has a built-in mDNS relay (UniFi, Mikrotik, etc.), \
             disable it to avoid conflicts with Syntaur's mDNS reflector. \
             Running two reflectors causes duplicate devices and discovery failures. \
             Set MDNS_REFLECT=false to disable Syntaur's reflector instead."
                .into(),
        ])
    } else {
        None
    };

    Json(DiscoverResponse {
        total_found: found.len(),
        new_devices,
        already_registered: existing.len(),
        warnings,
    })
}

#[derive(Deserialize)]
struct DiscoverRequest {
    timeout_secs: Option<u64>,
    mqtt_broker: Option<String>,
}

#[derive(Serialize)]
struct DiscoverResponse {
    total_found: usize,
    new_devices: Vec<crate::devices::discovery::DiscoveredDevice>,
    already_registered: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    warnings: Option<Vec<String>>,
}

/// List all Matter nodes from the fabric (live query to python-matter-server).
async fn handle_matter_nodes(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<crate::devices::matter::MatterNode>>, (StatusCode, String)> {
    state
        .devices
        .list_matter_nodes()
        .await
        .map(Json)
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))
}

/// Auto-import all Matter nodes into the device registry.
async fn handle_matter_import(
    State(state): State<Arc<ApiState>>,
) -> Json<serde_json::Value> {
    let imported = state.devices.import_matter_nodes().await;
    Json(serde_json::json!({
        "imported": imported.len(),
        "devices": imported,
    }))
}

// ── Profile endpoints ───────────────────────────────────────────────────

async fn handle_get_profile(
    State(state): State<Arc<ApiState>>,
) -> Json<UserProfile> {
    Json(state.profile.get().await)
}

async fn handle_update_profile(
    State(state): State<Arc<ApiState>>,
    Json(profile): Json<UserProfile>,
) -> Json<serde_json::Value> {
    state.profile.update(&profile).await;
    Json(serde_json::json!({"status": "updated"}))
}

// ── Conversation search ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: u32,
}

fn default_search_limit() -> u32 {
    20
}

async fn handle_search_conversations(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<SearchRequest>,
) -> Json<Vec<crate::conversation::Conversation>> {
    Json(state.conversations.search(&req.query, req.limit).await)
}
