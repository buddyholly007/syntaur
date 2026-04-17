//! Context budget manager — ensures ALL users get a good experience
//! regardless of their model's context window size.
//!
//! Allocates the available context across: persona template, memories,
//! personality doc, module context, tax context, and conversation history.
//! On small models, lower-priority components get truncated or dropped.
//! On large models, everything loads generously.

/// Priority-ordered allocation of context budget.
///
/// The budget manager works top-down: highest-priority components get
/// their minimum allocation first, then remaining space is distributed
/// to lower-priority components.
///
/// Priority (highest to lowest):
///   1. User's current message (always fits)
///   2. Core persona identity (minimum ~200 tokens)
///   3. Recent conversation history (at least 2 turns)
///   4. Most important memories
///   5. Full persona template
///   6. Personality doc
///   7. Module-specific context
///   8. Extended conversation history
pub struct ContextBudget {
    /// Total context window in tokens
    pub context_window: usize,
    /// Tokens reserved for the response
    pub response_reserve: usize,
    /// Budget for the persona system prompt
    pub persona_tokens: usize,
    /// Number of memories to inject
    pub memory_count: usize,
    /// Budget for personality doc
    pub personality_tokens: usize,
    /// Budget for module-specific context (tax, calendar, etc.)
    pub module_context_tokens: usize,
    /// Budget for conversation history
    pub history_tokens: usize,
    /// Whether to inject tax context
    pub include_tax_context: bool,
    /// Tier for persona detail
    pub persona_tier: PersonaTier,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PersonaTier {
    /// ~200 tokens: name + role + 3 key rules
    Minimal,
    /// ~500 tokens: core personality + key behaviors
    Standard,
    /// Full template: all rules, memory protocol, edge cases
    Full,
}

impl ContextBudget {
    /// Calculate a context budget for the given model configuration.
    pub fn for_context_window(context_window_tokens: u64, max_response_tokens: u64) -> Self {
        let total = context_window_tokens as usize;
        let response = max_response_tokens.min(context_window_tokens / 4) as usize;
        let available = total.saturating_sub(response).saturating_sub(500); // 500 for user message

        match available {
            // Tiny model (2-4K available): bare essentials only
            0..=2000 => Self {
                context_window: total,
                response_reserve: response,
                persona_tokens: 200,
                memory_count: 1,
                personality_tokens: 0,
                module_context_tokens: 0,
                history_tokens: available.saturating_sub(250),
                include_tax_context: false,
                persona_tier: PersonaTier::Minimal,
            },
            // Small model (4-8K available): minimal but functional
            2001..=6000 => Self {
                context_window: total,
                response_reserve: response,
                persona_tokens: 400,
                memory_count: 3,
                personality_tokens: 200,
                module_context_tokens: 0,
                history_tokens: available.saturating_sub(700),
                include_tax_context: false,
                persona_tier: PersonaTier::Minimal,
            },
            // Medium model (8-16K available): good experience
            6001..=14000 => Self {
                context_window: total,
                response_reserve: response,
                persona_tokens: 800,
                memory_count: 6,
                personality_tokens: 500,
                module_context_tokens: 200,
                history_tokens: available.saturating_sub(1800),
                include_tax_context: true,
                persona_tier: PersonaTier::Standard,
            },
            // Large model (16-64K available): comfortable
            14001..=60000 => Self {
                context_window: total,
                response_reserve: response,
                persona_tokens: 1500,
                memory_count: 10,
                personality_tokens: 1000,
                module_context_tokens: 500,
                history_tokens: available.saturating_sub(4000),
                include_tax_context: true,
                persona_tier: PersonaTier::Full,
            },
            // Very large model (64K+): load everything
            _ => Self {
                context_window: total,
                response_reserve: response,
                persona_tokens: 3000,
                memory_count: 20,
                personality_tokens: 4000,
                module_context_tokens: 1000,
                history_tokens: available.saturating_sub(10000),
                include_tax_context: true,
                persona_tier: PersonaTier::Full,
            },
        }
    }

    /// Truncate text to fit within a token budget.
    /// Cuts at the last complete sentence before the budget.
    pub fn truncate_to_budget(text: &str, max_tokens: usize) -> String {
        if max_tokens == 0 { return String::new(); }
        let max_chars = max_tokens * 4; // ~4 chars per token
        if text.len() <= max_chars { return text.to_string(); }

        let truncated = &text[..max_chars];
        // Try to cut at last sentence boundary
        if let Some(pos) = truncated.rfind(". ") {
            format!("{}.", &truncated[..pos])
        } else if let Some(pos) = truncated.rfind('\n') {
            truncated[..pos].to_string()
        } else {
            format!("{}...", &truncated[..max_chars.saturating_sub(3)])
        }
    }

    /// Truncate conversation history to fit within the history budget.
    /// Keeps the most recent messages, drops oldest first.
    pub fn truncate_history(messages: &[(String, String)], max_tokens: usize) -> Vec<(String, String)> {
        if max_tokens == 0 { return vec![]; }

        let max_chars = max_tokens * 4;
        let mut total = 0;
        let mut kept = Vec::new();

        // Walk from newest to oldest, accumulate until budget
        for msg in messages.iter().rev() {
            let msg_chars = msg.0.len() + msg.1.len() + 10; // role + content + overhead
            if total + msg_chars > max_chars && !kept.is_empty() {
                break;
            }
            total += msg_chars;
            kept.push(msg.clone());
        }

        kept.reverse();
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_model_gets_minimal() {
        let b = ContextBudget::for_context_window(4096, 1024);
        assert_eq!(b.persona_tier, PersonaTier::Minimal);
        assert!(b.memory_count <= 2);
        assert!(!b.include_tax_context);
    }

    #[test]
    fn large_model_gets_full() {
        let b = ContextBudget::for_context_window(131072, 4096);
        assert_eq!(b.persona_tier, PersonaTier::Full);
        assert!(b.memory_count >= 10);
        assert!(b.include_tax_context);
    }

    #[test]
    fn truncate_preserves_sentences() {
        let text = "First sentence. Second sentence. Third sentence. Fourth sentence.";
        let truncated = ContextBudget::truncate_to_budget(text, 5); // ~20 chars
        assert!(truncated.ends_with('.'));
        assert!(truncated.len() < text.len());
    }

    #[test]
    fn history_keeps_recent() {
        let msgs = vec![
            ("user".to_string(), "old message".to_string()),
            ("assistant".to_string(), "old reply".to_string()),
            ("user".to_string(), "recent message".to_string()),
            ("assistant".to_string(), "recent reply".to_string()),
        ];
        let kept = ContextBudget::truncate_history(&msgs, 20); // ~80 chars
        assert!(kept.len() <= msgs.len());
        // Should keep the most recent
        if !kept.is_empty() {
            assert_eq!(kept.last().unwrap().1, "recent reply");
        }
    }
}
