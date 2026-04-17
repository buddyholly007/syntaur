//! Background task system for async tool operations.
//!
//! When a tool (like image generation) takes too long to block the
//! conversation, it spawns a background task that:
//!   1. Returns immediately with a task_id
//!   2. Runs the actual work in a tokio::spawn
//!   3. Stores the result when done
//!   4. Optionally appends to the conversation
//!
//! Frontend polls /api/tasks/{id} to check completion.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub user_id: i64,
    pub conversation_id: Option<String>,
    pub agent_id: String,
    pub task_type: String,
    pub status: String,
    pub input_summary: String,
    pub result: Option<serde_json::Value>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

pub struct BackgroundTaskManager {
    tasks: RwLock<HashMap<String, BackgroundTask>>,
}

impl BackgroundTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new pending task. Returns the task_id.
    pub async fn create(
        &self,
        user_id: i64,
        conversation_id: Option<String>,
        agent_id: &str,
        task_type: &str,
        input_summary: &str,
    ) -> String {
        let id = format!("task-{}", uuid::Uuid::new_v4().simple());
        let task = BackgroundTask {
            id: id.clone(),
            user_id,
            conversation_id,
            agent_id: agent_id.to_string(),
            task_type: task_type.to_string(),
            status: "pending".to_string(),
            input_summary: input_summary.to_string(),
            result: None,
            created_at: chrono::Utc::now().timestamp(),
            completed_at: None,
        };
        self.tasks.write().await.insert(id.clone(), task);
        id
    }

    /// Mark a task as complete with a result.
    pub async fn complete(&self, id: &str, result: serde_json::Value) {
        if let Some(task) = self.tasks.write().await.get_mut(id) {
            task.status = "complete".to_string();
            task.result = Some(result);
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
    }

    /// Mark a task as failed.
    pub async fn fail(&self, id: &str, error: &str) {
        if let Some(task) = self.tasks.write().await.get_mut(id) {
            task.status = "failed".to_string();
            task.result = Some(serde_json::json!({"error": error}));
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
    }

    /// Get a task by id (for polling).
    pub async fn get(&self, id: &str, user_id: i64) -> Option<BackgroundTask> {
        let tasks = self.tasks.read().await;
        tasks.get(id).filter(|t| t.user_id == user_id).cloned()
    }

    /// Get pending tasks for a conversation (to include in chat response).
    pub async fn pending_for_conversation(&self, conversation_id: &str, user_id: i64) -> Vec<BackgroundTask> {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .filter(|t| {
                t.user_id == user_id
                    && t.status == "pending"
                    && t.conversation_id.as_deref() == Some(conversation_id)
            })
            .cloned()
            .collect()
    }

    /// Clean up old completed tasks (>1 hour old).
    pub async fn cleanup(&self) {
        let cutoff = chrono::Utc::now().timestamp() - 3600;
        let mut tasks = self.tasks.write().await;
        tasks.retain(|_, t| {
            t.status == "pending" || t.completed_at.map_or(true, |c| c > cutoff)
        });
    }
}
