//! Clarification phase: decides whether the query needs more info before planning.

use crate::llm::{ChatMessage, LlmChain};

use super::prompts::CLARIFY_SYSTEM_PROMPT;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClarifyResult {
    Ready,
    NeedsClarification { questions: Vec<String> },
}

#[derive(serde::Deserialize)]
struct ClarifyJson {
    ready: bool,
    #[serde(default)]
    questions: Vec<String>,
}

const MAX_QUESTIONS: usize = 5;

pub async fn run_clarify(query: &str, llm: &LlmChain) -> Result<ClarifyResult, String> {
    let messages = vec![
        ChatMessage::system(CLARIFY_SYSTEM_PROMPT),
        ChatMessage::user(query),
    ];
    let raw = llm.call(&messages).await?;
    let cleaned = strip_code_fences(&raw);
    let parsed: ClarifyJson = serde_json::from_str(cleaned)
        .map_err(|e| format!("clarify JSON parse: {} — raw: {}", e, &raw[..raw.len().min(300)]))?;
    if parsed.ready {
        return Ok(ClarifyResult::Ready);
    }
    let mut q = parsed.questions;
    q.truncate(MAX_QUESTIONS);
    if q.is_empty() {
        return Ok(ClarifyResult::Ready);
    }
    Ok(ClarifyResult::NeedsClarification { questions: q })
}

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
