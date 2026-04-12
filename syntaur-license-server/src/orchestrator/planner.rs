use std::sync::Arc;

use log::info;
use serde::{Deserialize, Serialize};

use crate::agent::registry::AgentRegistry;
use crate::backend::router::BackendRouter;
use crate::backend::{CompletionRequest, RoutePreferences};
use crate::task::TaskCategory;

/// A single step in an execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStep {
    pub description: String,
    pub category: TaskCategory,
    pub instruction: String,
    /// Which agent should handle this step.
    pub agent_id: Option<String>,
    /// Preferred backend for this step.
    pub backend_hint: Option<String>,
    /// Fallback agent if primary fails.
    pub fallback_agent: Option<String>,
    /// Fallback backend if primary fails.
    pub fallback_backend: Option<String>,
    /// Indices of steps this depends on (must complete first).
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// Steps in the same parallel group can run concurrently.
    pub parallel_group: Option<usize>,
}

/// A complete execution plan with steps and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub steps: Vec<PlannedStep>,
    pub summary: String,
}

impl ExecutionPlan {
    /// Get steps that can run in a given parallel group.
    pub fn steps_in_group(&self, group: usize) -> Vec<(usize, &PlannedStep)> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.parallel_group == Some(group))
            .collect()
    }

    /// Get the maximum parallel group number.
    pub fn max_group(&self) -> usize {
        self.steps
            .iter()
            .filter_map(|s| s.parallel_group)
            .max()
            .unwrap_or(0)
    }

    /// Get steps that have no dependencies and no parallel group (run sequentially).
    pub fn sequential_steps(&self) -> Vec<(usize, &PlannedStep)> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.parallel_group.is_none())
            .collect()
    }
}

/// Agent-aware planner that analyzes user requests and produces execution plans
/// specifying which agents handle which parts, with fallback options.
pub struct Planner {
    backend_router: Arc<BackendRouter>,
}

impl Planner {
    pub fn new(backend_router: Arc<BackendRouter>) -> Self {
        Self { backend_router }
    }

    /// Analyze a user instruction and produce an execution plan.
    pub async fn plan(
        &self,
        instruction: &str,
        registry: &AgentRegistry,
    ) -> Result<ExecutionPlan, PlanError> {
        // Classify the task
        let classification = self.classify_task(instruction).await?;

        info!(
            "[planner] classified as {:?}, building plan",
            classification
        );

        let plan = match classification {
            // Simple conversation: single step to assistant
            TaskClassification::Simple(category) => {
                let agent_id = registry
                    .find_for_category(&category)
                    .map(|a| a.id().to_string())
                    .unwrap_or_else(|| "assistant".into());

                ExecutionPlan {
                    summary: format!("Direct {} task → {}", category, agent_id),
                    steps: vec![PlannedStep {
                        description: instruction.to_string(),
                        category,
                        instruction: instruction.to_string(),
                        agent_id: Some(agent_id),
                        backend_hint: None,
                        fallback_agent: Some("assistant".into()),
                        fallback_backend: None,
                        depends_on: vec![],
                        parallel_group: None,
                    }],
                }
            }

            // Multi-step task requiring coordination
            TaskClassification::MultiStep(steps) => {
                let mut planned_steps = Vec::new();
                let mut prev_idx = None;

                for (i, (cat, desc)) in steps.into_iter().enumerate() {
                    let agent_id = registry
                        .find_for_category(&cat)
                        .map(|a| a.id().to_string())
                        .unwrap_or_else(|| "assistant".into());

                    let depends = prev_idx.map(|idx| vec![idx]).unwrap_or_default();

                    planned_steps.push(PlannedStep {
                        description: desc.clone(),
                        category: cat,
                        instruction: desc,
                        agent_id: Some(agent_id),
                        backend_hint: None,
                        fallback_agent: Some("assistant".into()),
                        fallback_backend: None,
                        depends_on: depends,
                        parallel_group: None,
                    });

                    prev_idx = Some(i);
                }

                ExecutionPlan {
                    summary: format!("{}-step plan", planned_steps.len()),
                    steps: planned_steps,
                }
            }

            // Parallel tasks that can run concurrently
            TaskClassification::Parallel(tasks) => {
                let planned_steps: Vec<PlannedStep> = tasks
                    .into_iter()
                    .map(|(cat, desc)| {
                        let agent_id = registry
                            .find_for_category(&cat)
                            .map(|a| a.id().to_string())
                            .unwrap_or_else(|| "assistant".into());

                        PlannedStep {
                            description: desc.clone(),
                            category: cat,
                            instruction: desc,
                            agent_id: Some(agent_id),
                            backend_hint: None,
                            fallback_agent: Some("assistant".into()),
                            fallback_backend: None,
                            depends_on: vec![],
                            parallel_group: Some(0),
                        }
                    })
                    .collect();

                let count = planned_steps.len();
                ExecutionPlan {
                    summary: format!("{} parallel tasks", count),
                    steps: planned_steps,
                }
            }
        };

        info!(
            "[planner] plan: {} ({} steps)",
            plan.summary,
            plan.steps.len()
        );
        Ok(plan)
    }

    /// Classify the task by analyzing the instruction with an LLM.
    async fn classify_task(
        &self,
        instruction: &str,
    ) -> Result<TaskClassification, PlanError> {
        // First attempt: heuristic classification (fast, no LLM call)
        if let Some(classification) = self.heuristic_classify(instruction) {
            return Ok(classification);
        }

        // If heuristics are inconclusive, use LLM for classification.
        // Use a short max_tokens and low temperature to keep this fast —
        // classification should not compete heavily with real work.
        let classify_prompt = format!(
            r#"Classify this task. Respond with ONLY one of these JSON formats:

For a single task:
{{"type":"simple","category":"<conversation|search|coding|research|planning>"}}

For sequential steps:
{{"type":"multi","steps":[{{"category":"<cat>","description":"<what to do>"}}]}}

For independent parallel tasks:
{{"type":"parallel","tasks":[{{"category":"<cat>","description":"<what to do>"}}]}}

Task: {}"#,
            instruction
        );

        let request = CompletionRequest::simple(&classify_prompt)
            .with_temperature(0.0)
            .with_max_tokens(256);

        let response = self
            .backend_router
            .route(&request, &RoutePreferences::default())
            .await
            .map_err(|e| PlanError::ClassificationFailed(e.to_string()))?;

        self.parse_classification(&response.content)
    }

    /// Fast heuristic classification without LLM call.
    fn heuristic_classify(&self, instruction: &str) -> Option<TaskClassification> {
        let lower = instruction.to_lowercase();

        // Programming language names — strong coding signal when combined with action verbs
        let lang_names = [
            "python", "rust", "javascript", "typescript", "java", "c++", "go ",
            "golang", "ruby", "swift", "kotlin", "scala", "haskell", "elixir",
            "php", "sql", "bash", "shell", "html", "css", "react", "vue",
            "angular", "node.js", "django", "flask", "actix", "axum",
        ];

        // Action verbs that indicate coding when paired with a language or technical noun
        let coding_actions = [
            "write a ", "create a ", "build a ", "make a ", "generate ",
            "develop ", "code ", "program ",
        ];

        // Direct coding signals
        let coding_signals = [
            "write code", "implement", "function", "class ", "debug",
            "fix the bug", "fix this bug", "refactor", "code review",
            "write a program", "write a script", "create a function",
            "def ", "fn ", "```", "api endpoint", "rest api",
            "web scraper", "web crawler", "algorithm", "data structure",
            "unit test", "test case", "compile", "syntax error",
            "hello world", "fibonacci", "sort ", "parse ",
            "http server", "websocket", "database query",
        ];

        // Search signals
        let search_signals = [
            "search for", "look up", "find information",
            "what is the latest", "current price", "news about",
            "who is", "when did", "how much does", "where can i find",
        ];

        // Research signals
        let research_signals = [
            "analyze", "research", "compare", "evaluate",
            "investigate", "in-depth", "comprehensive", "deep dive",
            "pros and cons", "trade-offs", "trade offs",
            "advantages and disadvantages", "benchmark",
        ];

        // Check direct coding signals first
        if coding_signals.iter().any(|s| lower.contains(s)) {
            return Some(TaskClassification::Simple(TaskCategory::Coding));
        }

        // Check action verb + programming language combination
        let has_coding_action = coding_actions.iter().any(|a| lower.contains(a));
        let has_lang_name = lang_names.iter().any(|l| lower.contains(l));
        if has_coding_action && has_lang_name {
            return Some(TaskClassification::Simple(TaskCategory::Coding));
        }

        // Action verb + technical nouns (even without explicit language name)
        let technical_nouns = [
            "server", "client", "api", "endpoint", "scraper", "crawler",
            "parser", "compiler", "bot", "cli", "tool", "library",
            "framework", "microservice", "daemon", "proxy", "cache",
        ];
        if has_coding_action && technical_nouns.iter().any(|n| lower.contains(n)) {
            return Some(TaskClassification::Simple(TaskCategory::Coding));
        }

        if search_signals.iter().any(|s| lower.contains(s)) {
            return Some(TaskClassification::Simple(TaskCategory::Search));
        }

        if research_signals.iter().any(|s| lower.contains(s)) {
            return Some(TaskClassification::Simple(TaskCategory::Research));
        }

        // Check for multi-step indicators before defaulting to conversation
        let multi_step_signals = [" then ", " and then ", " after that ", " finally ",
                                   " next ", " first ", " second ", " step 1"];
        let has_multi_step = multi_step_signals.iter().any(|s| lower.contains(s));
        if has_multi_step {
            // Don't classify as simple conversation — let LLM handle multi-step
            return None;
        }

        // Short, simple messages with no specialized signals → conversation
        if instruction.len() < 300 {
            return Some(TaskClassification::Simple(TaskCategory::Conversation));
        }

        // Long messages without clear signals → let LLM classify
        None
    }

    fn parse_classification(&self, content: &str) -> Result<TaskClassification, PlanError> {
        // Extract JSON from the response
        let json_str = content
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let value: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| PlanError::ClassificationFailed(format!("invalid JSON: {}", e)))?;

        let task_type = value
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("simple");

        match task_type {
            "simple" => {
                let cat = parse_category(
                    value
                        .get("category")
                        .and_then(|c| c.as_str())
                        .unwrap_or("conversation"),
                );
                Ok(TaskClassification::Simple(cat))
            }
            "multi" => {
                let steps = value
                    .get("steps")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|step| {
                                let cat = parse_category(
                                    step.get("category")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("conversation"),
                                );
                                let desc = step
                                    .get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                (cat, desc)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(TaskClassification::MultiStep(steps))
            }
            "parallel" => {
                let tasks = value
                    .get("tasks")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|task| {
                                let cat = parse_category(
                                    task.get("category")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("conversation"),
                                );
                                let desc = task
                                    .get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                (cat, desc)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(TaskClassification::Parallel(tasks))
            }
            _ => Ok(TaskClassification::Simple(TaskCategory::Conversation)),
        }
    }
}

fn parse_category(s: &str) -> TaskCategory {
    match s {
        "conversation" | "chat" => TaskCategory::Conversation,
        "search" | "web_search" => TaskCategory::Search,
        "coding" | "code" => TaskCategory::Coding,
        "research" | "analysis" => TaskCategory::Research,
        "planning" | "plan" => TaskCategory::Planning,
        "tool_execution" | "tool" => TaskCategory::ToolExecution,
        other => TaskCategory::Custom(other.to_string()),
    }
}

#[derive(Debug)]
enum TaskClassification {
    Simple(TaskCategory),
    MultiStep(Vec<(TaskCategory, String)>),
    Parallel(Vec<(TaskCategory, String)>),
}

#[derive(Debug)]
pub enum PlanError {
    ClassificationFailed(String),
    NoPlan(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClassificationFailed(msg) => write!(f, "classification failed: {}", msg),
            Self::NoPlan(msg) => write!(f, "no plan generated: {}", msg),
        }
    }
}

impl std::error::Error for PlanError {}
