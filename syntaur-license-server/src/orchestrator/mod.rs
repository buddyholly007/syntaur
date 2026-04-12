pub mod planner;

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use dashmap::DashMap;
use log::{info, warn};
use uuid::Uuid;

use crate::agent::registry::AgentRegistry;
use crate::agent::{AgentContext, AgentError, SubAgentRunner};
use crate::backend::router::BackendRouter;
use crate::config::ExecutorConfig;
use crate::task::executor::TaskExecutor;
use crate::task::{TaskPayload, TaskResult};

use planner::{ExecutionPlan, PlannedStep, Planner};

/// The core orchestrator that receives user requests, plans execution,
/// delegates to agents, and manages the full lifecycle.
pub struct Orchestrator {
    pub registry: AgentRegistry,
    pub backend_router: Arc<BackendRouter>,
    planner: Planner,
    executor: TaskExecutor,
    active_tasks: DashMap<Uuid, TaskHandle>,
    default_agent: String,
}

#[derive(Debug, Clone)]
struct TaskHandle {
    agent_id: String,
    started_at: Instant,
    parent_id: Option<Uuid>,
}

impl Orchestrator {
    pub fn new(
        registry: AgentRegistry,
        backend_router: Arc<BackendRouter>,
        executor_config: ExecutorConfig,
        default_agent: String,
    ) -> Self {
        let planner = Planner::new(backend_router.clone());
        let executor = TaskExecutor::new(backend_router.clone(), executor_config);

        Self {
            registry,
            backend_router,
            planner,
            executor,
            active_tasks: DashMap::new(),
            default_agent,
        }
    }

    /// Submit a single task for execution. Classifies, routes, and executes.
    pub async fn submit(
        &self,
        task: TaskPayload,
    ) -> Result<TaskResult, OrchestratorError> {
        let (result, _) = self.submit_with_events(task, None).await?;
        Ok(result)
    }

    /// Submit a task and receive real-time status events via the returned receiver.
    /// Events include sub-agent start/done notifications.
    pub async fn submit_with_events(
        &self,
        task: TaskPayload,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::task::TaskEvent>>,
    ) -> Result<(TaskResult, Vec<crate::task::TaskEvent>), OrchestratorError> {
        let task_id = task.id;
        info!("[orchestrator] submit task {} ({})", task_id, task.category);

        let agent = self
            .registry
            .find_for_category(&task.category)
            .or_else(|| self.registry.get(&self.default_agent))
            .ok_or_else(|| {
                OrchestratorError::NoAgent(format!("no agent for {}", task.category))
            })?
            .clone();

        self.active_tasks.insert(
            task_id,
            TaskHandle {
                agent_id: agent.id().to_string(),
                started_at: Instant::now(),
                parent_id: task.parent_task_id,
            },
        );

        let ctx = self.build_context_with_events(event_tx);

        let timeout = task.timeout;
        let result = tokio::time::timeout(timeout, agent.execute(task, &ctx))
            .await
            .map_err(|_| OrchestratorError::Timeout)?
            .map_err(|e| OrchestratorError::AgentError(e.to_string()))?;

        self.active_tasks.remove(&task_id);

        Ok((result, Vec::new()))
    }

    /// Plan and execute a complex instruction. Uses the planner to decompose
    /// the work, then executes steps respecting dependencies and parallelism.
    pub async fn plan_and_execute(
        &self,
        instruction: &str,
        messages: Vec<crate::task::Message>,
    ) -> Result<OrchestrationResult, OrchestratorError> {
        let start = Instant::now();

        // Generate execution plan
        let plan = self
            .planner
            .plan(instruction, &self.registry)
            .await
            .map_err(|e| OrchestratorError::PlanningFailed(e.to_string()))?;

        info!(
            "[orchestrator] executing plan: {} ({} steps)",
            plan.summary,
            plan.steps.len()
        );

        // Execute the plan
        let results = self.execute_plan(&plan, instruction, &messages).await?;

        // Synthesize a final response from all step results
        let final_output = self.synthesize_results(&results);

        Ok(OrchestrationResult {
            plan,
            step_results: results,
            final_output,
            total_duration: start.elapsed(),
        })
    }

    /// Execute a plan's steps, respecting dependencies and parallelism.
    async fn execute_plan(
        &self,
        plan: &ExecutionPlan,
        original_instruction: &str,
        messages: &[crate::task::Message],
    ) -> Result<Vec<TaskResult>, OrchestratorError> {
        let mut all_results: Vec<Option<TaskResult>> = vec![None; plan.steps.len()];

        // Group steps by parallel_group, then execute groups in order
        let max_group = plan.max_group();

        // First: execute grouped (parallel) steps
        for group in 0..=max_group {
            let group_steps = plan.steps_in_group(group);
            if group_steps.is_empty() {
                continue;
            }

            // Check all dependencies are satisfied
            for (idx, step) in &group_steps {
                for dep in &step.depends_on {
                    if all_results.get(*dep).and_then(|r| r.as_ref()).is_none() {
                        return Err(OrchestratorError::DependencyFailed(format!(
                            "step {} depends on step {} which hasn't completed",
                            idx, dep
                        )));
                    }
                }
            }

            if group_steps.len() == 1 {
                // Single step, run directly
                let (idx, step) = group_steps[0];
                let result = self.execute_step(step, original_instruction, messages).await?;
                all_results[idx] = Some(result);
            } else {
                // Multiple steps, run in parallel
                let handles: Vec<_> = group_steps
                    .iter()
                    .map(|(idx, step)| {
                        let step = (*step).clone();
                        let instruction = original_instruction.to_string();
                        let msgs = messages.to_vec();
                        let idx = *idx;
                        // We need to execute in the context of self, so build tasks
                        let task = self.step_to_task(&step, &instruction, &msgs);
                        let agent = self
                            .registry
                            .find_for_category(&step.category)
                            .or_else(|| {
                                step.agent_id
                                    .as_ref()
                                    .and_then(|id| self.registry.get(id))
                            })
                            .or_else(|| self.registry.get(&self.default_agent))
                            .cloned();
                        let ctx = self.build_context();

                        tokio::spawn(async move {
                            let agent = agent.ok_or_else(|| {
                                OrchestratorError::NoAgent("no agent found".into())
                            })?;
                            let result = agent
                                .execute(task, &ctx)
                                .await
                                .map_err(|e| OrchestratorError::AgentError(e.to_string()))?;
                            Ok::<(usize, TaskResult), OrchestratorError>((idx, result))
                        })
                    })
                    .collect();

                for handle in handles {
                    match handle.await {
                        Ok(Ok((idx, result))) => {
                            all_results[idx] = Some(result);
                        }
                        Ok(Err(e)) => {
                            warn!("[orchestrator] parallel step failed: {}", e);
                        }
                        Err(e) => {
                            warn!("[orchestrator] parallel task join error: {}", e);
                        }
                    }
                }
            }
        }

        // Then: execute sequential (non-grouped) steps
        for (idx, step) in plan.sequential_steps() {
            if all_results[idx].is_some() {
                continue; // Already handled
            }

            // Check dependencies
            for dep in &step.depends_on {
                if all_results.get(*dep).and_then(|r| r.as_ref()).is_none() {
                    warn!(
                        "[orchestrator] skipping step {} — dependency {} not met",
                        idx, dep
                    );
                    continue;
                }
            }

            match self.execute_step(step, original_instruction, messages).await {
                Ok(result) => {
                    all_results[idx] = Some(result);
                }
                Err(e) => {
                    warn!("[orchestrator] step {} failed: {}", idx, e);
                    // Try fallback agent if specified
                    if let Some(ref fallback_id) = step.fallback_agent {
                        info!("[orchestrator] trying fallback agent: {}", fallback_id);
                        if let Some(fallback) = self.registry.get(fallback_id) {
                            let task =
                                self.step_to_task(step, original_instruction, messages);
                            let ctx = self.build_context();
                            if let Ok(result) = fallback.execute(task, &ctx).await {
                                all_results[idx] = Some(result);
                                continue;
                            }
                        }
                    }
                }
            }
        }

        Ok(all_results.into_iter().flatten().collect())
    }

    /// Execute a single plan step.
    async fn execute_step(
        &self,
        step: &PlannedStep,
        original_instruction: &str,
        messages: &[crate::task::Message],
    ) -> Result<TaskResult, OrchestratorError> {
        let task = self.step_to_task(step, original_instruction, messages);

        let agent = step
            .agent_id
            .as_ref()
            .and_then(|id| self.registry.get(id))
            .or_else(|| self.registry.find_for_category(&step.category))
            .or_else(|| self.registry.get(&self.default_agent))
            .ok_or_else(|| OrchestratorError::NoAgent(format!("no agent for step")))?
            .clone();

        let ctx = self.build_context();

        agent
            .execute(task, &ctx)
            .await
            .map_err(|e| OrchestratorError::AgentError(e.to_string()))
    }

    fn step_to_task(
        &self,
        step: &PlannedStep,
        original_instruction: &str,
        messages: &[crate::task::Message],
    ) -> TaskPayload {
        let instruction = if step.instruction.is_empty() {
            original_instruction.to_string()
        } else {
            step.instruction.clone()
        };

        TaskPayload::new(step.category.clone(), instruction).with_messages(messages.to_vec())
    }

    fn build_context(&self) -> AgentContext {
        self.build_context_with_events(None)
    }

    fn build_context_with_events(
        &self,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::task::TaskEvent>>,
    ) -> AgentContext {
        let router = self.backend_router.clone();
        let sub_runner: Arc<dyn SubAgentRunner> = Arc::new(OrchestratorSubRunner {
            backend_router: router.clone(),
            registry_snapshot: self.registry.list(),
        });

        AgentContext {
            backend_router: router,
            conversation_id: None,
            sub_agent_runner: Some(sub_runner),
            event_tx,
        }
    }

    /// Synthesize a final output from multiple step results.
    fn synthesize_results(&self, results: &[TaskResult]) -> serde_json::Value {
        if results.len() == 1 {
            return results[0].output.clone();
        }

        // Combine all outputs
        let mut combined_content = String::new();
        for result in results.iter() {
            if let Some(text) = result.output_text() {
                if !combined_content.is_empty() {
                    combined_content.push_str("\n\n");
                }
                if results.len() > 1 {
                    combined_content.push_str(&format!("**[{}]** ", result.agent_id));
                }
                combined_content.push_str(text);
            }
        }

        serde_json::json!({
            "content": combined_content,
            "step_count": results.len(),
            "agents_used": results.iter().map(|r| r.agent_id.as_str()).collect::<Vec<_>>(),
        })
    }

    pub fn active_task_count(&self) -> usize {
        self.active_tasks.len()
    }
}

/// Allows major agents to spawn sub-agents through a simplified interface.
struct OrchestratorSubRunner {
    backend_router: Arc<BackendRouter>,
    registry_snapshot: Vec<crate::agent::registry::AgentInfo>,
}

#[async_trait]
impl SubAgentRunner for OrchestratorSubRunner {
    async fn run_sub_agent(
        &self,
        agent_id: &str,
        task: TaskPayload,
    ) -> Result<TaskResult, AgentError> {
        // Create the appropriate built-in agent directly
        let agent: Box<dyn crate::agent::Agent> = match agent_id {
            "search" => Box::new(crate::agent::builtin::search::SearchAgent::new()),
            "coder" => Box::new(crate::agent::builtin::coder::CoderAgent::new()),
            "researcher" => Box::new(crate::agent::builtin::researcher::ResearcherAgent::new()),
            _ => return Err(AgentError::UnsupportedTask(format!("unknown sub-agent: {}", agent_id))),
        };

        let ctx = AgentContext {
            backend_router: self.backend_router.clone(),
            conversation_id: None,
            sub_agent_runner: None,
            event_tx: None,
        };

        agent.execute(task, &ctx).await
    }

    async fn run_parallel(
        &self,
        tasks: Vec<(String, TaskPayload)>,
    ) -> Vec<Result<TaskResult, AgentError>> {
        let handles: Vec<_> = tasks
            .into_iter()
            .map(|(agent_id, task)| {
                let router = self.backend_router.clone();
                tokio::spawn(async move {
                    let agent: Box<dyn crate::agent::Agent> = match agent_id.as_str() {
                        "search" => Box::new(crate::agent::builtin::search::SearchAgent::new()),
                        "coder" => Box::new(crate::agent::builtin::coder::CoderAgent::new()),
                        "researcher" => {
                            Box::new(crate::agent::builtin::researcher::ResearcherAgent::new())
                        }
                        _ => {
                            return Err(AgentError::UnsupportedTask(format!(
                                "unknown sub-agent: {}",
                                agent_id
                            )))
                        }
                    };

                    let ctx = AgentContext {
                        backend_router: router,
                        conversation_id: None,
                        sub_agent_runner: None,
                        event_tx: None,
                    };

                    agent.execute(task, &ctx).await
                })
            })
            .collect();

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(AgentError::Internal(e.to_string()))),
            }
        }
        results
    }
}

/// Full result of an orchestrated multi-step execution.
#[derive(Debug)]
pub struct OrchestrationResult {
    pub plan: ExecutionPlan,
    pub step_results: Vec<TaskResult>,
    pub final_output: serde_json::Value,
    pub total_duration: std::time::Duration,
}

#[derive(Debug)]
pub enum OrchestratorError {
    NoAgent(String),
    PlanningFailed(String),
    AgentError(String),
    DependencyFailed(String),
    Timeout,
    Internal(String),
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAgent(msg) => write!(f, "no agent: {}", msg),
            Self::PlanningFailed(msg) => write!(f, "planning failed: {}", msg),
            Self::AgentError(msg) => write!(f, "agent error: {}", msg),
            Self::DependencyFailed(msg) => write!(f, "dependency failed: {}", msg),
            Self::Timeout => write!(f, "orchestration timed out"),
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for OrchestratorError {}
