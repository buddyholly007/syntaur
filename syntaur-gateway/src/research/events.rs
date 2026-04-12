//! Research session events for SSE streaming.
//!
//! Each session has an associated `tokio::sync::broadcast` channel. The
//! orchestrator emits events at every phase boundary. Subscribers (SSE
//! handlers) read from the channel and convert events to SSE messages.
//!
//! Sessions are identified by id; when a session starts, a fresh channel
//! is created and stored in `AppState::research_events` keyed by session id.
//! When the session completes (or errors), a final terminal event is
//! emitted, after which subscribers can close the connection.

use serde::Serialize;

/// One event in a research session's lifecycle. The `event` field carries
/// the variant tag so SSE consumers can dispatch on it.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ResearchEvent {
    /// Session started, before any LLM calls
    Started {
        session_id: String,
        query: String,
        agent: String,
    },
    /// Cache hit — session will be returned immediately, no work done
    CacheHit {
        session_id: String,
        cached_age_secs: i64,
    },
    /// Plan phase complete
    PlanGenerated {
        session_id: String,
        steps: usize,
        plan_titles: Vec<String>,
    },
    /// One subtask was spawned and is now running
    SubtaskStarted {
        session_id: String,
        step_index: usize,
        task: String,
    },
    /// One subtask finished (success or partial)
    SubtaskCompleted {
        session_id: String,
        step_index: usize,
        rounds_used: usize,
        citations: usize,
        duration_ms: u64,
        error: Option<String>,
    },
    /// Report synthesis phase started
    ReportStarted { session_id: String },
    /// Report synthesis complete — also signals session end on success
    Complete {
        session_id: String,
        duration_ms: u64,
        report_chars: usize,
    },
    /// Session ended with an error — terminal event
    Error {
        session_id: String,
        message: String,
    },
}

impl ResearchEvent {
    /// True if this is a terminal event (Complete or Error). SSE handlers
    /// should close the stream after seeing one.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete { .. } | Self::Error { .. })
    }
}
