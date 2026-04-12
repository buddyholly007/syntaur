use log::{debug, info};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize)]
pub enum HookEvent {
    SessionStart { agent_id: String, session_id: String },
    SessionEnd { agent_id: String, session_id: String },
    MessageReceived { agent_id: String, user_id: i64, text_preview: String },
    MessageSent { agent_id: String, text_preview: String },
    ToolCallStart { agent_id: String, tool: String },
    ToolCallEnd { agent_id: String, tool: String, success: bool, duration_ms: u64 },
    CronJobStart { job_id: String, agent_id: String },
    CronJobEnd { job_id: String, success: bool, duration_ms: u64 },
    LlmCallStart { agent_id: String, provider: String, model: String },
    LlmCallEnd { agent_id: String, provider: String, success: bool, latency_ms: u64 },
    Error { source: String, message: String },
}

pub struct HookBus {
    sender: broadcast::Sender<HookEvent>,
}

impl HookBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self { sender }
    }

    pub fn emit(&self, event: HookEvent) {
        debug!("Hook: {:?}", event);
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HookEvent> {
        self.sender.subscribe()
    }
}

/// Background task that logs hook events for auditing
pub async fn hook_logger(bus: Arc<HookBus>) {
    let mut rx = bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                match &event {
                    HookEvent::CronJobEnd { job_id, success, duration_ms } => {
                        if *success {
                            info!("[hook] Cron {} completed in {}ms", job_id, duration_ms);
                        } else {
                            log::warn!("[hook] Cron {} failed after {}ms", job_id, duration_ms);
                        }
                    }
                    HookEvent::Error { source, message } => {
                        log::error!("[hook] Error from {}: {}", source, message);
                    }
                    HookEvent::LlmCallEnd { agent_id, provider, success, latency_ms } => {
                        if !success {
                            log::warn!("[hook] LLM call failed: agent={}, provider={}, latency={}ms", agent_id, provider, latency_ms);
                        }
                    }
                    _ => {}
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("[hook] Dropped {} events (receiver lagged)", n);
            }
            Err(_) => break,
        }
    }
}
