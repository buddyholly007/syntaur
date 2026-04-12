use std::collections::HashMap;
use std::sync::Arc;

use log::info;

use super::{Agent, AgentType};
use crate::task::TaskCategory;

/// Central registry of all available agents. Supports lookup by ID,
/// capability-based matching, and parent-child relationships.
pub struct AgentRegistry {
    agents: HashMap<String, Arc<dyn Agent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    pub fn register(&mut self, agent: Arc<dyn Agent>) {
        info!(
            "[registry] registered agent: {} ({}, type={})",
            agent.id(),
            agent.name(),
            agent.agent_type()
        );
        self.agents.insert(agent.id().to_string(), agent);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Agent>> {
        self.agents.get(id)
    }

    /// Find the best agent for a given task category.
    /// Prefers sub-agents that specialize in the category, then falls back to major agents.
    pub fn find_for_category(&self, category: &TaskCategory) -> Option<&Arc<dyn Agent>> {
        // First: look for a sub-agent that handles this category
        let mut sub_match = None;
        let mut major_match = None;

        for agent in self.agents.values() {
            if agent.capabilities().contains(category) {
                match agent.agent_type() {
                    AgentType::Sub => {
                        sub_match = Some(agent);
                    }
                    AgentType::Major => {
                        major_match = Some(agent);
                    }
                }
            }
        }

        sub_match.or(major_match)
    }

    /// Get all sub-agents for a given major agent.
    pub fn sub_agents_of(&self, major_agent_id: &str) -> Vec<&Arc<dyn Agent>> {
        self.agents
            .values()
            .filter(|a| a.parent_agent_id() == Some(major_agent_id))
            .collect()
    }

    /// Get all major agents.
    pub fn major_agents(&self) -> Vec<&Arc<dyn Agent>> {
        self.agents
            .values()
            .filter(|a| a.agent_type() == AgentType::Major)
            .collect()
    }

    /// List all registered agents with their metadata.
    pub fn list(&self) -> Vec<AgentInfo> {
        self.agents
            .values()
            .map(|a| AgentInfo {
                id: a.id().to_string(),
                name: a.name().to_string(),
                agent_type: a.agent_type(),
                capabilities: a.capabilities().to_vec(),
                description: a.description().to_string(),
                parent: a.parent_agent_id().map(String::from),
            })
            .collect()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub agent_type: AgentType,
    pub capabilities: Vec<TaskCategory>,
    pub description: String,
    pub parent: Option<String>,
}
