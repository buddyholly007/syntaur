//! Plan generation + parallel subtask execution + final report synthesis.
//!
//! The orchestrator is plain Rust workflow code (not an LLM-driven loop).
//! It makes one LLM call to produce the plan, runs subtasks in parallel
//! with a `Semaphore(3)` cap, then makes one LLM call to synthesize the
//! final report from the collected evidence.

use std::sync::Arc;
use std::time::Instant;

use log::{error, info, warn};
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::llm::{ChatMessage, LlmChain};
use crate::tools::ToolRegistry;

use super::agent::run_subtask;
use super::evidence::{EvidenceItem, EvidenceStore, Plan, ResearchReport};
use super::prompts::{PLAN_SYSTEM_PROMPT, REPORT_SYSTEM_PROMPT};
use super::events::ResearchEvent;
use super::store::SessionStore;

const MAX_PLAN_STEPS: usize = 6;
const MAX_PARALLEL_SUBTASKS: usize = 3;
const DEFAULT_TIME_BUDGET_SECS: u64 = 300; // 5 minutes
const MAX_TIME_BUDGET_SECS: u64 = 1800; // 30 minutes

/// Parameters for one research session.
pub struct ResearchRequest {
    pub query: String,
    pub agent_id: String,
    pub time_budget_secs: Option<u64>,
    /// Maximum age in seconds for cached results to be returned. 0 disables cache.
    pub cache_max_age_secs: Option<i64>,
    /// Optional pre-existing session id (used when /api/research/start has
    /// already created the row so the id can be returned to the caller before
    /// run_research even begins). If None, run_research creates its own.
    pub session_id_override: Option<String>,
    /// Optional clarification answers from a prior /api/research/clarify call.
    /// When present, prepended to the query before planning so the planner has
    /// the user's clarifications in context.
    pub clarification_answers: Option<String>,
    /// Owning user_id for this session (v5 Item 3). 0 for legacy admin.
    /// Stamped on the session row and used as the cache scope key so
    /// cached results never bleed across users.
    pub user_id: i64,
}

/// Run a complete research session: plan → orchestrate → report.
/// Errors during any phase are captured in `ResearchReport.error` rather
/// than propagated, so callers always get a structured response.
///
/// `session_store` is optional — when present, the session is persisted to
/// SQLite with checkpoints between phases, and the cache is consulted before
/// the planner runs.
pub async fn run_research(
    req: ResearchRequest,
    llm_chain: Arc<LlmChain>,
    llm_chain_fast: Arc<LlmChain>,
    tool_registry: Arc<ToolRegistry>,
    session_store: Option<Arc<SessionStore>>,
    events: Option<tokio::sync::broadcast::Sender<ResearchEvent>>,
) -> ResearchReport {
    let started = Instant::now();
    let time_budget = req
        .time_budget_secs
        .unwrap_or(DEFAULT_TIME_BUDGET_SECS)
        .min(MAX_TIME_BUDGET_SECS);

    info!(
        "[research] starting session for agent={} query={:?} time_budget={}s",
        req.agent_id, req.query, time_budget
    );

    // Helper to emit an event into the broadcast channel if present.
    // Errors (no subscribers) are ignored — events are best-effort.
    let emit = |ev: ResearchEvent| {
        if let Some(tx) = &events {
            let _ = tx.send(ev);
        }
    };

    // Cache lookup — return cached result if a recent successful session
    // exists for the same (agent, query, user_id) tuple. The user_id
    // filter means Alice never sees Bob's cached result (v5 Item 3). Pass
    // None scope only when the caller is legacy admin (user_id = 0).
    let cache_scope = if req.user_id == 0 {
        None
    } else {
        Some(req.user_id)
    };
    if let (Some(store), Some(max_age)) = (&session_store, req.cache_max_age_secs) {
        if max_age > 0 {
            if let Some(cached) = store
                .find_cached_recent(&req.agent_id, &req.query, max_age, cache_scope)
                .await
            {
                info!(
                    "[research] cache HIT — returning prior session ({}ms duration)",
                    cached.total_duration_ms
                );
                return cached;
            }
        }
    }

    // Use a pre-existing session id if provided, otherwise create one.
    let session_id: Option<String> = if let Some(id) = req.session_id_override.clone() {
        info!("[research] using pre-existing session id={}", id);
        Some(id)
    } else if let Some(store) = &session_store {
        match store.create(&req.agent_id, &req.query, req.user_id).await {
            Ok(id) => {
                info!("[research] persisted session id={}", id);
                Some(id)
            }
            Err(e) => {
                warn!("[research] failed to create session: {}", e);
                None
            }
        }
    } else {
        None
    };
    let store_ref = session_store.as_ref();
    if let (Some(s), Some(id)) = (store_ref, session_id.as_deref()) {
        let _ = s.update_status(id, "planning").await;
    }
    if let Some(id) = session_id.as_deref() {
        emit(ResearchEvent::Started {
            session_id: id.to_string(),
            query: req.query.clone(),
            agent: req.agent_id.clone(),
        });
    }

    // If the caller provided clarification answers, prepend them to the
    // query so the planner sees both the user's intent and the clarifications.
    let planner_input = if let Some(ans) = &req.clarification_answers {
        format!("{}\n\nClarifications from the user:\n{}", req.query, ans)
    } else {
        req.query.clone()
    };

    // ── Phase 1: Plan ─────────────────────────────────────────────────
    let plan = match generate_plan(&planner_input, &llm_chain_fast).await {
        Ok(p) => p,
        Err(e) => {
            error!("[research] plan generation failed: {}", e);
            return ResearchReport {
                query: req.query,
                plan: vec![],
                evidence: vec![],
                report: String::new(),
                total_duration_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("plan generation failed: {}", e)),
            };
        }
    };

    if let (Some(s), Some(id)) = (store_ref, session_id.as_deref()) {
        let _ = s.store_plan(id, &plan).await;
        let _ = s.update_status(id, "orchestrating").await;
    }
    if let Some(id) = session_id.as_deref() {
        let titles: Vec<String> = plan
            .plan
            .iter()
            .map(|s| s.description.chars().take(140).collect())
            .collect();
        emit(ResearchEvent::PlanGenerated {
            session_id: id.to_string(),
            steps: plan.len(),
            plan_titles: titles,
        });
    }
    info!("[research] plan: {} step(s)", plan.len());
    for (i, step) in plan.plan.iter().enumerate() {
        info!(
            "[research]   step {}: {}",
            i + 1,
            &step.description[..step.description.len().min(120)]
        );
    }

    if plan.is_empty() {
        return ResearchReport {
            query: req.query,
            plan: vec![],
            evidence: vec![],
            report: "Planner produced no steps.".to_string(),
            total_duration_ms: started.elapsed().as_millis() as u64,
            error: Some("empty plan".to_string()),
        };
    }

    // ── Phase 2: Orchestrate (parallel subtasks) ──────────────────────
    let mut evidence = EvidenceStore::new();
    let semaphore = Arc::new(Semaphore::new(MAX_PARALLEL_SUBTASKS));
    let mut join_set: JoinSet<EvidenceItem> = JoinSet::new();

    for (i, step) in plan.plan.iter().enumerate() {
        let step_index = i + 1;
        let task = step.description.clone();
        if let Some(id) = session_id.as_deref() {
            emit(ResearchEvent::SubtaskStarted {
                session_id: id.to_string(),
                step_index,
                task: task.chars().take(200).collect(),
            });
        }
        let chain = Arc::clone(&llm_chain);
        let registry = Arc::clone(&tool_registry);
        let sem = Arc::clone(&semaphore);
        join_set.spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return EvidenceItem {
                        step_index,
                        task,
                        summary: String::new(),
                        citations: vec![],
                        tools_used: vec![],
                        duration_ms: 0,
                        rounds_used: 0,
                        error: Some("semaphore closed".to_string()),
                    };
                }
            };
            run_subtask(step_index, task, chain, registry).await
        });
    }

    // Collect with overall time budget
    let collect_fut = async {
        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(item) => {
                    if let Some(id) = session_id.as_deref() {
                        emit(ResearchEvent::SubtaskCompleted {
                            session_id: id.to_string(),
                            step_index: item.step_index,
                            rounds_used: item.rounds_used,
                            citations: item.citations.len(),
                            duration_ms: item.duration_ms,
                            error: item.error.clone(),
                        });
                    }
                    evidence.push(item);
                }
                Err(e) => warn!("[research] subtask join error: {}", e),
            }
        }
    };
    if tokio::time::timeout(
        std::time::Duration::from_secs(time_budget),
        collect_fut,
    )
    .await
    .is_err()
    {
        warn!("[research] time budget exhausted; aborting remaining subtasks");
        join_set.abort_all();
    }

    evidence.sort_by_step();
    if let (Some(s), Some(id)) = (store_ref, session_id.as_deref()) {
        let _ = s.store_evidence(id, &evidence.items).await;
        let _ = s.update_status(id, "reporting").await;
    }
    info!(
        "[research] orchestrate done: {}/{} subtasks completed in {}ms",
        evidence.len(),
        plan.len(),
        started.elapsed().as_millis()
    );

    // ── Phase 3: Report ───────────────────────────────────────────────
    if let Some(id) = session_id.as_deref() {
        emit(ResearchEvent::ReportStarted {
            session_id: id.to_string(),
        });
    }
    let report_text = match synthesize_report(&req.query, &plan, &evidence, &llm_chain_fast).await {
        Ok(t) => t,
        Err(e) => {
            error!("[research] report synthesis failed: {}", e);
            format!(
                "Report synthesis failed ({}). Raw evidence below:\n\n{}",
                e,
                evidence.render_for_report()
            )
        }
    };

    let total_duration_ms = started.elapsed().as_millis() as u64;
    if let (Some(s), Some(id)) = (store_ref, session_id.as_deref()) {
        let _ = s.store_report(id, &report_text).await;
        // Persist citation refs so the cache can be invalidated when underlying docs change
        let mut seen: std::collections::HashSet<(String, String)> = Default::default();
        let mut refs: Vec<(String, String)> = Vec::new();
        for item in &evidence.items {
            for c in &item.citations {
                let key = (c.source.clone(), c.external_id.clone());
                if seen.insert(key.clone()) {
                    refs.push(key);
                }
            }
        }
        let _ = s.store_doc_refs(id, refs).await;
        let _ = s.mark_complete(id, total_duration_ms, None).await;
    }
    if let Some(id) = session_id.as_deref() {
        emit(ResearchEvent::Complete {
            session_id: id.to_string(),
            duration_ms: total_duration_ms,
            report_chars: report_text.len(),
        });
    }
    info!(
        "[research] session complete in {}ms ({} evidence items, {} char report)",
        total_duration_ms,
        evidence.len(),
        report_text.len()
    );

    ResearchReport {
        query: req.query,
        plan: plan.plan.clone(),
        evidence: evidence.items.clone(),
        report: report_text,
        total_duration_ms,
        error: None,
    }
}

/// Plan-phase LLM call. Returns parsed `Plan` or an error string.
async fn generate_plan(query: &str, llm_chain: &LlmChain) -> Result<Plan, String> {
    let messages = vec![
        ChatMessage::system(PLAN_SYSTEM_PROMPT),
        ChatMessage::user(query),
    ];
    let raw = llm_chain.call(&messages).await?;

    // The planner is instructed to output ONLY a JSON object. Some models
    // wrap it in ```json ... ``` markdown anyway — strip those if present.
    let cleaned = strip_code_fences(&raw);

    let mut plan: Plan = serde_json::from_str(cleaned)
        .map_err(|e| format!("plan JSON parse error: {} — raw: {}", e, &raw[..raw.len().min(500)]))?;

    if plan.is_empty() {
        return Err("plan contained no steps".to_string());
    }
    plan.truncate_to_max(MAX_PLAN_STEPS);
    Ok(plan)
}

/// Report-phase LLM call. Synthesizes evidence into a final markdown answer.
async fn synthesize_report(
    query: &str,
    plan: &Plan,
    evidence: &EvidenceStore,
    llm_chain: &LlmChain,
) -> Result<String, String> {
    let plan_str = plan
        .plan
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.description))
        .collect::<Vec<_>>()
        .join("\n");

    let user_msg = format!(
        "## Original query\n{}\n\n## Plan executed\n{}\n\n## Evidence\n{}",
        query,
        plan_str,
        evidence.render_for_report()
    );

    let messages = vec![
        ChatMessage::system(REPORT_SYSTEM_PROMPT),
        ChatMessage::user(&user_msg),
    ];

    llm_chain.call(&messages).await
}

/// Strip ```json ... ``` or ``` ... ``` code fences if the LLM wrapped its output.
fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        let rest = rest.trim_start_matches('\n');
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.trim_start_matches('\n');
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    trimmed
}

/// Parameter validation helper used by the HTTP handler.
pub fn validate_query(q: &str) -> Result<(), String> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Err("query is empty".to_string());
    }
    if trimmed.len() < 10 {
        return Err("query too short — provide a detailed question".to_string());
    }
    if trimmed.len() > 8000 {
        return Err(format!("query too long: {} chars (max 8000)", trimmed.len()));
    }
    let _ = MAX_PARALLEL_SUBTASKS; // keep used to avoid warnings
    Ok(())
}
