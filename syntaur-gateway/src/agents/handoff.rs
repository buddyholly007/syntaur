//! Handoff carrier: context transfer between main agent and specialists.
//!
//! When the user accepts an escalation offer, the handoff endpoint:
//! 1. Extracts the last N messages from the main-agent conversation
//! 2. Creates a new specialist conversation
//! 3. Seeds it with the handoff context as a system note
//! 4. Returns a greeting + the new conversation_id
//!
//! When the user returns from a specialist, the re-entry endpoint:
//! 1. Extracts the last assistant message as a summary
//! 2. Appends a system note to the main-agent conversation
//! 3. Main agent absorbs the summary on next interaction

/// Build a handoff context string from recent messages.
/// Takes the last `limit` messages and formats them as a compact summary
/// that the specialist can read on first turn.
pub fn build_handoff_context(
    messages: &[(String, String)], // (role, content) pairs
    from_agent_name: &str,
    to_agent_name: &str,
) -> String {
    if messages.is_empty() {
        return format!(
            "{} handed this conversation to you. The user wants to continue here.",
            from_agent_name
        );
    }

    let mut ctx = format!(
        "{} passed this conversation to you. Here's what was being discussed:\n\n",
        from_agent_name
    );

    for (role, content) in messages {
        let label = match role.as_str() {
            "user" => "User",
            "assistant" => from_agent_name,
            _ => "System",
        };
        // Truncate long messages to keep context compact
        let truncated = if content.len() > 300 {
            format!("{}...", &content[..297])
        } else {
            content.clone()
        };
        ctx.push_str(&format!("{}: {}\n\n", label, truncated));
    }

    ctx.push_str(&format!(
        "---\nYou are {} now. Pick up where {} left off. \
         Greet the user briefly, acknowledge what they were discussing, \
         and ask how you can help.",
        to_agent_name, from_agent_name
    ));

    ctx
}

/// Build a re-entry summary from a specialist conversation.
/// Takes the last assistant message and formats it as a one-line note
/// for the main agent to absorb.
pub fn build_reentry_summary(
    specialist_agent_name: &str,
    last_messages: &[(String, String)],
) -> String {
    // Find the last assistant message
    let last_assistant = last_messages
        .iter()
        .rev()
        .find(|(role, _)| role == "assistant")
        .map(|(_, content)| content.as_str());

    // Find the last user message for topic context
    let last_user = last_messages
        .iter()
        .rev()
        .find(|(role, _)| role == "user")
        .map(|(_, content)| content.as_str());

    match (last_assistant, last_user) {
        (Some(asst), Some(user)) => {
            let asst_short = if asst.len() > 200 {
                format!("{}...", &asst[..197])
            } else {
                asst.to_string()
            };
            let user_short = if user.len() > 100 {
                format!("{}...", &user[..97])
            } else {
                user.to_string()
            };
            format!(
                "[{} session] User asked: \"{}\". {}'s last response: \"{}\"",
                specialist_agent_name, user_short, specialist_agent_name, asst_short
            )
        }
        (Some(asst), None) => {
            let short = if asst.len() > 200 {
                format!("{}...", &asst[..197])
            } else {
                asst.to_string()
            };
            format!("[{} session] Last response: \"{}\"", specialist_agent_name, short)
        }
        _ => format!("[{} session] Conversation ended.", specialist_agent_name),
    }
}

/// Map an agent_id to its display name from user_agents or defaults.
pub fn agent_display_for_module(module: &str) -> &'static str {
    match module {
        "main" => "Kyron",
        "tax" => "Positron",
        "research" => "Cortex",
        "music" => "Silvr",
        "scheduler" => "Thaddeus",
        "coders" => "Maurice",
        "journal" => "Mushi",
        _ => "the assistant",
    }
}
