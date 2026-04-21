//! Natural-language → automation-AST compiler.
//!
//! Flow (lands week 7):
//!   1. User types English at `/api/smart-home/automation/compile`.
//!   2. Gateway bundles the prompt with the user's device+room inventory
//!      and hands it to the configured LLM (TurboQuant first, OpenRouter
//!      fallback per `projects/turboquant.md`).
//!   3. LLM returns a canonical AST (automation::AutomationSpec).
//!   4. Server validates every device_id/room_id against the live DB,
//!      repairs or rejects, and returns a preview.
//!   5. UI shows the preview + "Save" button — we never persist without
//!      explicit user confirmation.
//!
//! Scaffolding only: the compile entry-point is stubbed and always
//! returns a preview-not-supported error so the UI can wire up the
//! form before the LLM path is live.

use serde::{Deserialize, Serialize};

use super::automation::AutomationSpec;

#[derive(Debug, Clone, Deserialize)]
pub struct CompileRequest {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompilePreview {
    pub summary: String,
    pub spec: AutomationSpec,
    pub warnings: Vec<String>,
}

pub async fn compile(_user_id: i64, _req: CompileRequest) -> Result<CompilePreview, String> {
    Err("nl_automation::compile not yet implemented (scaffolded in week 1)".to_string())
}
