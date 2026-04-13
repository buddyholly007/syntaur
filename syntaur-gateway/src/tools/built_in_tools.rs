//! Trait-based wrappers around the legacy free-function tool implementations.
//!
//! v5 Item 1 collapses the giant `tools/mod.rs::execute()` match into a
//! uniform funnel that runs everything through the `Tool` trait. This file
//! holds the per-tool struct shells. Each struct just delegates to the
//! existing free function in its family module so we don't rewrite business
//! logic during the migration — only the dispatch wrapper changes.
//!
//! Tools are added here family-by-family in the order specified by the v5
//! plan: memory → file_ops → shell → web → email → sms → captcha →
//! captcha_bridge → office → account → browser. After each family is
//! migrated the matching arms in `tools/mod.rs::execute()` are deleted.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};
use super::{account, browser, captcha, captcha_bridge, email, file_ops, memory, shell, sms, web};

// ── helpers ────────────────────────────────────────────────────────────────

fn arg_str<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn arg_str_or<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or(default)
}

fn arg_u64_or(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}

// ── memory family ──────────────────────────────────────────────────────────

pub struct MemoryReadTool;

#[async_trait]
impl Tool for MemoryReadTool {
    fn name(&self) -> &str {
        "memory_read"
    }
    fn description(&self) -> &str {
        "Read agent memory. With an empty `query`, lists available memory files. With a query, tries an exact filename match first, then a workspace-root file, then full-text search across all .md files in the agent's memory directory."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Filename (with or without .md), workspace file name, or substring to search for. Empty string lists all memory files."}
            }
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::default()
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = arg_str(&args, "query");
        memory::read_memory(ctx.workspace, query).map(RichToolResult::text)
    }
}

pub struct MemoryWriteTool;

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
    }
    fn description(&self) -> &str {
        "Write content to an agent memory file. The file is stored under the agent workspace's memory/ directory. Existing files are overwritten."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "Filename (with or without .md). Created under the agent's memory/ dir."},
                "content": {"type": "string", "description": "Full file content. Overwrites any existing file."}
            },
            "required": ["key", "content"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_local()
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let key = arg_str(&args, "key");
        let content = arg_str(&args, "content");
        memory::write_memory(ctx.workspace, key, content).map(RichToolResult::text)
    }
}

// ── file_ops family ────────────────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read a file from workspace. Output: file content (max ~10KB). Use for text files only."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to workspace"}
            },
            "required": ["path"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        file_ops::read_file(ctx.workspace, arg_str(&args, "path")).map(RichToolResult::text)
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Write/overwrite a file in workspace. Creates parent dirs if needed."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to workspace"},
                "content": {"type": "string", "description": "Full file content to write"}
            },
            "required": ["path", "content"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_local()
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        file_ops::write_file(
            ctx.workspace,
            arg_str(&args, "path"),
            arg_str(&args, "content"),
        )
        .map(RichToolResult::text)
    }
}

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Replace text in a file. old_string must match exactly."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string", "description": "Exact text to find and replace"},
                "new_string": {"type": "string", "description": "Replacement text"}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_local()
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        file_ops::edit_file(
            ctx.workspace,
            arg_str(&args, "path"),
            arg_str(&args, "old_string"),
            arg_str(&args, "new_string"),
        )
        .map(RichToolResult::text)
    }
}

pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }
    fn description(&self) -> &str {
        "List files in a workspace subdirectory. Returns one entry per line with [FILE]/[DIR] prefixes."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Workspace-relative path (default '.')"}
            }
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        file_ops::list_files(ctx.workspace, arg_str_or(&args, "path", ".")).map(RichToolResult::text)
    }
}

// ── shell family ───────────────────────────────────────────────────────────

pub struct ExecTool;

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }
    fn description(&self) -> &str {
        "Run a shell command. Output: stdout+stderr (max 64KB). Sandboxed to workspace scripts."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "timeout": {"type": "integer", "description": "Timeout in seconds (default 30)"}
            },
            "required": ["command"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: true,
            idempotent: false,
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let command = arg_str(&args, "command");
        let timeout = arg_u64_or(&args, "timeout", 30);
        shell::exec_sandboxed(ctx.workspace, command, timeout, ctx.allowed_scripts, "argv")
            .await
            .map(RichToolResult::text)
    }
}

// ── web family ─────────────────────────────────────────────────────────────

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web. Output: list of titles, URLs, snippets (~500 chars)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "max_results": {"type": "integer", "description": "Max results (default 5)"}
            },
            "required": ["query"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("searxng"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let query = arg_str(&args, "query");
        let max = arg_u64_or(&args, "max_results", 5) as usize;
        web::web_search(query, max).await.map(RichToolResult::text)
    }
}

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn description(&self) -> &str {
        "Fetch a URL and return text content. Output: ~2000 chars of page text. For full browser interaction, use browser tools instead."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to fetch"}
            },
            "required": ["url"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("web_fetch"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        web::web_fetch(arg_str(&args, "url"))
            .await
            .map(RichToolResult::text)
    }
}

pub struct JsonQueryTool;

#[async_trait]
impl Tool for JsonQueryTool {
    fn name(&self) -> &str {
        "json_query"
    }
    fn description(&self) -> &str {
        "Extract value from JSON by dot-path. Example: path='results.0.name'"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "data": {"type": "string", "description": "JSON string"},
                "path": {"type": "string", "description": "Dot-path like 'key.0.subkey'"}
            },
            "required": ["data"]
        })
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        web::json_query(arg_str(&args, "data"), arg_str(&args, "path")).map(RichToolResult::text)
    }
}

pub struct SendTelegramTool;

#[async_trait]
impl Tool for SendTelegramTool {
    fn name(&self) -> &str {
        "send_telegram"
    }
    fn description(&self) -> &str {
        "Send a Telegram message to a specific bot/chat"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bot_token": {"type": "string", "description": "Telegram bot token"},
                "chat_id": {"type": "string", "description": "Chat ID to send to"},
                "message": {"type": "string", "description": "Message text"}
            },
            "required": ["bot_token", "chat_id", "message"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("telegram")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        web::send_telegram(
            arg_str(&args, "bot_token"),
            arg_str(&args, "chat_id"),
            arg_str(&args, "message"),
        )
        .await
        .map(RichToolResult::text)
    }
}

// ── email family ───────────────────────────────────────────────────────────

pub struct EmailReadTool;

#[async_trait]
impl Tool for EmailReadTool {
    fn name(&self) -> &str {
        "email_read"
    }
    fn description(&self) -> &str {
        "Read emails via IMAP. Accounts: 'felix' (felixcherry1985@outlook.com) or 'crimson-lantern' (CrimsonLanternMusic@gmail.com, default). Output: headers + body preview per email."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "folder": {"type": "string", "description": "Folder (default INBOX)"},
                "count": {"type": "integer", "description": "Number of emails (default 5, max 20)"},
                "account": {"type": "string", "description": "'felix' or 'crimson-lantern' (default)"}
            }
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("email"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let folder = arg_str_or(&args, "folder", "INBOX");
        let count = arg_u64_or(&args, "count", 5) as usize;
        let account = arg_str(&args, "account");
        email::email_read_account(folder, count, account)
            .await
            .map(RichToolResult::text)
    }
}

pub struct EmailSendTool;

#[async_trait]
impl Tool for EmailSendTool {
    fn name(&self) -> &str {
        "email_send"
    }
    fn description(&self) -> &str {
        "Send an email via SMTP. Accounts: 'felix' or 'crimson-lantern' (default)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {"type": "string"},
                "subject": {"type": "string"},
                "body": {"type": "string"},
                "account": {"type": "string", "description": "'felix' or 'crimson-lantern' (default)"}
            },
            "required": ["to", "subject", "body"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("email")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        email::email_send_account(
            arg_str(&args, "to"),
            arg_str(&args, "subject"),
            arg_str(&args, "body"),
            arg_str(&args, "account"),
        )
        .await
        .map(RichToolResult::text)
    }
}

// ── sms family (Google Voice via browser) ──────────────────────────────────

pub struct SmsGetNumberTool;

#[async_trait]
impl Tool for SmsGetNumberTool {
    fn name(&self) -> &str {
        "sms_get_number"
    }
    fn description(&self) -> &str {
        "Get your Google Voice phone number for entering on verification forms. Returns the number."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("sms"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        sms::sms_get_number().await.map(RichToolResult::text)
    }
}

pub struct SmsReadTool;

#[async_trait]
impl Tool for SmsReadTool {
    fn name(&self) -> &str {
        "sms_read"
    }
    fn description(&self) -> &str {
        "Read recent SMS messages from Google Voice via browser. Navigates to voice.google.com (will leave current page). Returns message text."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer", "description": "Number of messages (default 5, max 10)"}
            }
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("sms"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let count = arg_u64_or(&args, "count", 5) as usize;
        sms::sms_read(count).await.map(RichToolResult::text)
    }
}

pub struct SmsWaitForCodeTool;

#[async_trait]
impl Tool for SmsWaitForCodeTool {
    fn name(&self) -> &str {
        "sms_wait_for_code"
    }
    fn description(&self) -> &str {
        "Wait for a verification code SMS on Google Voice. Polls every 5s until a new 4-8 digit code appears. Navigates to voice.google.com (will leave current page). Returns the code."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "timeout": {"type": "integer", "description": "Max seconds to wait (default 60, max 120)"}
            }
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("sms"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let timeout = arg_u64_or(&args, "timeout", 60);
        sms::sms_wait_for_code(timeout)
            .await
            .map(RichToolResult::text)
    }
}

// ── captcha family ─────────────────────────────────────────────────────────

pub struct SolveCaptchaTool;

#[async_trait]
impl Tool for SolveCaptchaTool {
    fn name(&self) -> &str {
        "solve_captcha"
    }
    fn description(&self) -> &str {
        "Auto-solve CAPTCHA on current page using AI vision. Handles press-and-hold, rotation, image selection. Call when you see 'prove you are human' or similar."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("captcha"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        captcha::solve_captcha(&api_key)
            .await
            .map(RichToolResult::text)
    }
}

pub struct CaptchaBridgeSolveTool;

#[async_trait]
impl Tool for CaptchaBridgeSolveTool {
    fn name(&self) -> &str {
        "captcha_bridge_solve"
    }
    fn description(&self) -> &str {
        "Run a pre-defined captcha-protected login flow end-to-end and return the extracted credential (e.g. an API key). Use this when you need to acquire a key from a service whose signup/login is gated by reCAPTCHA, hCaptcha, or Turnstile. Each supported site has its credentials pre-stored in ~/.captcha-bridge/config.toml. List supported sites with captcha_bridge_list_sites first."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "site": {"type": "string", "description": "Site name (e.g. 'cryptocompare'). Must be one of captcha_bridge_list_sites output."},
                "label": {"type": "string", "description": "Optional friendly name/label to use for the new credential, if the site asks for one"}
            },
            "required": ["site"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("captcha_bridge"),
            ..ToolCapabilities::write_external("captcha_bridge")
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let site = arg_str(&args, "site");
        let label = args.get("label").and_then(|v| v.as_str());
        captcha_bridge::solve(site, label)
            .await
            .map(RichToolResult::text)
    }
}

pub struct CaptchaBridgeBalanceTool;

#[async_trait]
impl Tool for CaptchaBridgeBalanceTool {
    fn name(&self) -> &str {
        "captcha_bridge_balance"
    }
    fn description(&self) -> &str {
        "Check the current 2Captcha account balance in USD. Each captcha solve costs ~$0.003. Top up via 2captcha.com when low."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_network()
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        captcha_bridge::balance().await.map(RichToolResult::text)
    }
}

pub struct CaptchaBridgeListSitesTool;

#[async_trait]
impl Tool for CaptchaBridgeListSitesTool {
    fn name(&self) -> &str {
        "captcha_bridge_list_sites"
    }
    fn description(&self) -> &str {
        "List the site names that captcha_bridge_solve supports. Each line is a site identifier passable to captcha_bridge_solve."
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        captcha_bridge::list_sites().await.map(RichToolResult::text)
    }
}

// ── office_cli family ──────────────────────────────────────────────────────
//
// All seven office_cli tools delegate to the existing `office_cli_exec()`
// helper in `tools::mod`. We re-expose it via `super::office_cli_exec` so
// the trait wrappers stay tiny.

macro_rules! office_tool {
    ($struct:ident, $name:literal, $desc:literal, $params:expr, $caps:expr) => {
        pub struct $struct;

        #[async_trait]
        impl Tool for $struct {
            fn name(&self) -> &str {
                $name
            }
            fn description(&self) -> &str {
                $desc
            }
            fn parameters(&self) -> Value {
                $params
            }
            fn capabilities(&self) -> ToolCapabilities {
                $caps
            }
            async fn execute(
                &self,
                args: Value,
                _ctx: &ToolContext<'_>,
            ) -> Result<RichToolResult, String> {
                super::office_cli_exec($name, &args)
                    .await
                    .map(RichToolResult::text)
            }
        }
    };
}

office_tool!(
    OfficeCreateTool,
    "office_create",
    "Create a blank document (.xlsx, .docx, or .pptx)",
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "File path with extension (.xlsx, .docx, .pptx)"}
        },
        "required": ["path"]
    }),
    ToolCapabilities::write_local()
);

office_tool!(
    OfficeViewTool,
    "office_view",
    "View document overview — sheets/rows/cols for Excel, paragraphs/words for Word, slides for PowerPoint",
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path to document file"}
        },
        "required": ["path"]
    }),
    ToolCapabilities::default()
);

office_tool!(
    OfficeGetTool,
    "office_get",
    "Read cell value or range from an Excel spreadsheet",
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path to .xlsx file"},
            "ref": {"type": "string", "description": "Cell reference, e.g. Sheet1!A1 or A1:C10"}
        },
        "required": ["path", "ref"]
    }),
    ToolCapabilities::default()
);

office_tool!(
    OfficeSetTool,
    "office_set",
    "Write a value to an Excel cell",
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path to .xlsx file"},
            "ref": {"type": "string", "description": "Cell reference, e.g. Sheet1!A1"},
            "value": {"type": "string", "description": "Value to write"},
            "type": {"type": "string", "enum": ["string", "number", "formula", "date"], "description": "Value type (default: string)"}
        },
        "required": ["path", "ref", "value"]
    }),
    ToolCapabilities::write_local()
);

office_tool!(
    OfficeBatchTool,
    "office_batch",
    "Execute multiple Excel operations in one open/save cycle. Much faster than individual set calls.",
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path to .xlsx file"},
            "ops": {"type": "array", "items": {"type": "object"}, "description": "Array of {op:'set', ref:'A1', value:'x', type:'string'} objects"}
        },
        "required": ["path", "ops"]
    }),
    ToolCapabilities::write_local()
);

office_tool!(
    OfficeMergeTool,
    "office_merge",
    "Replace {{key}} placeholders in a template document with data values. Works with .xlsx, .docx, .pptx.",
    json!({
        "type": "object",
        "properties": {
            "template": {"type": "string", "description": "Path to template file"},
            "data": {"type": "object", "description": "Key-value pairs for placeholder replacement"},
            "output": {"type": "string", "description": "Output file path (optional, defaults to template-merged.ext)"}
        },
        "required": ["template", "data"]
    }),
    ToolCapabilities::write_local()
);

office_tool!(
    OfficeSkillTool,
    "office_skill",
    "Generate a document using a built-in template. Available skills: invoice (Cherry Woodworks invoice with line items), expense_report (categorized expense spreadsheet with summary), tax_summary (annual tax report from GnuCash with Schedule C mapping and pie chart).",
    json!({
        "type": "object",
        "properties": {
            "skill": {"type": "string", "enum": ["invoice", "expense_report", "tax_summary"], "description": "Skill name"},
            "data": {"type": "object", "description": "Data for the skill. Invoice: {client, items:[{desc,qty,price}], output}. Expense: {title, transactions:[{date,desc,category,amount}], output}. Tax: {gnucash_path, year, output}."}
        },
        "required": ["skill"]
    }),
    ToolCapabilities::write_local()
);

// ── account family ─────────────────────────────────────────────────────────

pub struct CreateInstagramAccountTool;

#[async_trait]
impl Tool for CreateInstagramAccountTool {
    fn name(&self) -> &str {
        "create_instagram_account"
    }
    fn description(&self) -> &str {
        "Create an Instagram account. Handles signup form, birthday, CAPTCHA. May require phone verification (reports if needed)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "email": {"type": "string"},
                "password": {"type": "string"},
                "full_name": {"type": "string", "description": "Display name"},
                "username": {"type": "string", "description": "Instagram handle (no @)"}
            },
            "required": ["email", "password", "full_name", "username"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("account")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        account::create_instagram_account(
            arg_str(&args, "email"),
            arg_str(&args, "password"),
            arg_str(&args, "full_name"),
            arg_str(&args, "username"),
            &api_key,
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct MetaOauthTool;

#[async_trait]
impl Tool for MetaOauthTool {
    fn name(&self) -> &str {
        "meta_oauth"
    }
    fn description(&self) -> &str {
        "Run Meta OAuth flow to get Threads/Instagram API tokens. Opens auth page, logs in, authorizes app, exchanges code for long-lived token. Returns access_token + user_id."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app_id": {"type": "string", "description": "Meta App ID"},
                "app_secret": {"type": "string", "description": "Meta App Secret"},
                "redirect_uri": {"type": "string", "description": "OAuth redirect URI (default https://localhost/callback)"},
                "scopes": {"type": "string", "description": "Comma-separated scopes (default: threads_basic,threads_content_publish)"},
                "email": {"type": "string", "description": "Facebook/Instagram login email"},
                "password": {"type": "string", "description": "Facebook/Instagram login password"}
            },
            "required": ["app_id", "app_secret", "email", "password"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("meta")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        account::meta_oauth_flow(
            arg_str(&args, "app_id"),
            arg_str(&args, "app_secret"),
            arg_str_or(&args, "redirect_uri", "https://localhost/callback"),
            arg_str_or(&args, "scopes", "threads_basic,threads_content_publish"),
            arg_str(&args, "email"),
            arg_str(&args, "password"),
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct MetaRefreshTokenTool;

#[async_trait]
impl Tool for MetaRefreshTokenTool {
    fn name(&self) -> &str {
        "meta_refresh_token"
    }
    fn description(&self) -> &str {
        "Refresh a Meta long-lived token before it expires (60 days). Returns new token."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app_secret": {"type": "string"},
                "access_token": {"type": "string", "description": "Current long-lived token to refresh"}
            },
            "required": ["app_secret", "access_token"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("meta"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        account::meta_refresh_token(
            arg_str(&args, "app_secret"),
            arg_str(&args, "access_token"),
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct ThreadsPostTool;

#[async_trait]
impl Tool for ThreadsPostTool {
    fn name(&self) -> &str {
        "threads_post"
    }
    fn description(&self) -> &str {
        "Post text to Threads. Creates media container then publishes. Returns post ID."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "access_token": {"type": "string"},
                "user_id": {"type": "string"},
                "text": {"type": "string", "description": "Post content (max 500 chars)"},
                "url": {"type": "string", "description": "Optional link attachment"}
            },
            "required": ["access_token", "user_id", "text"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("threads")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let url = args.get("url").and_then(|v| v.as_str());
        account::threads_post(
            arg_str(&args, "access_token"),
            arg_str(&args, "user_id"),
            arg_str(&args, "text"),
            url,
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct CreateFacebookAccountTool;

#[async_trait]
impl Tool for CreateFacebookAccountTool {
    fn name(&self) -> &str {
        "create_facebook_account"
    }
    fn description(&self) -> &str {
        "Create a Facebook account. Handles form, dropdowns, submit, email code verification. ONE call does everything."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "email": {"type": "string"},
                "password": {"type": "string"},
                "first_name": {"type": "string"},
                "last_name": {"type": "string"},
                "birth_month": {"type": "string", "description": "e.g. 'Apr'"},
                "birth_day": {"type": "string", "description": "e.g. '1'"},
                "birth_year": {"type": "string", "description": "e.g. '1985'"},
                "email_password": {"type": "string", "description": "Password for the email account"}
            },
            "required": ["email", "password", "first_name", "last_name", "email_password"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("account")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        let password = arg_str(&args, "password");
        let email_password = arg_str_or(&args, "email_password", password);
        account::create_facebook_account(
            arg_str(&args, "email"),
            password,
            arg_str(&args, "first_name"),
            arg_str(&args, "last_name"),
            arg_str_or(&args, "birth_month", "Apr"),
            arg_str_or(&args, "birth_day", "1"),
            arg_str_or(&args, "birth_year", "1985"),
            email_password,
            &api_key,
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct CreateEmailAccountTool;

#[async_trait]
impl Tool for CreateEmailAccountTool {
    fn name(&self) -> &str {
        "create_email_account"
    }
    fn description(&self) -> &str {
        "Create a new Outlook email account. Handles ENTIRE flow: signup, form fill, CAPTCHA, verification. ONE call does everything. Returns credentials on success."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "provider": {"type": "string", "enum": ["outlook"]},
                "email": {"type": "string", "description": "e.g. felixcherry1985@outlook.com"},
                "password": {"type": "string"},
                "first_name": {"type": "string"},
                "last_name": {"type": "string"},
                "birth_month": {"type": "string", "description": "e.g. 'April'"},
                "birth_day": {"type": "string", "description": "e.g. '1'"},
                "birth_year": {"type": "string", "description": "e.g. '1985'"}
            },
            "required": ["email", "password", "first_name", "last_name"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("account")
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        let provider = arg_str_or(&args, "provider", "outlook");
        match provider {
            "outlook" | "hotmail" | "microsoft" => account::create_outlook_account(
                arg_str(&args, "email"),
                arg_str(&args, "password"),
                arg_str(&args, "first_name"),
                arg_str(&args, "last_name"),
                arg_str_or(&args, "birth_month", "4"),
                arg_str_or(&args, "birth_day", "1"),
                arg_str_or(&args, "birth_year", "1985"),
                &api_key,
            )
            .await
            .map(RichToolResult::text),
            other => Err(format!("Unsupported provider: {}", other)),
        }
    }
}

pub struct YoutubeTokenRefreshTool;

#[async_trait]
impl Tool for YoutubeTokenRefreshTool {
    fn name(&self) -> &str {
        "youtube_token_refresh"
    }
    fn description(&self) -> &str {
        "Refresh YouTube OAuth tokens via REST API (no browser). Fast and reliable. Use this FIRST when YouTube API calls fail with auth errors. Only fails if refresh token is fully expired."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("youtube"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        account::youtube_token_refresh(ctx.workspace)
            .await
            .map(RichToolResult::text)
    }
}

pub struct YoutubeReauthTool;

#[async_trait]
impl Tool for YoutubeReauthTool {
    fn name(&self) -> &str {
        "youtube_reauth"
    }
    fn description(&self) -> &str {
        "FALLBACK: Full browser OAuth re-authorization for YouTube. Only use if youtube_token_refresh fails with 'expired or revoked'. Opens Google consent in browser — may need Sean to approve 2FA via Telegram."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::write_external("youtube")
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        account::youtube_reauth(ctx.workspace)
            .await
            .map(RichToolResult::text)
    }
}

// ── browser family ─────────────────────────────────────────────────────────
//
// All browser tools share the "browser" circuit so a chromium crash trips
// the whole group at once. The legacy `_agent_id` parameter is hardcoded to
// "default" — singleton browser session per process, same as before.
//
// Most tools are read-only-from-the-network but write-from-the-page-state
// perspective. We mark them all `network: true` so they get a network
// circuit; the read_only flag is true for navigation/reads and false for
// fill/click/dropdown/etc.

const BROWSER_AGENT: &str = "default";

pub struct BrowserOpenTool;

#[async_trait]
impl Tool for BrowserOpenTool {
    fn name(&self) -> &str {
        "browser_open"
    }
    fn description(&self) -> &str {
        "Navigate to URL. Output: page title + first 2000 chars of text. Each call starts a fresh browser session — any previous session is torn down automatically. Use browser_fill/click/read after to interact with the same page."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL (https:// required)"}
            },
            "required": ["url"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_open(BROWSER_AGENT, arg_str(&args, "url"))
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserCloseTool;

#[async_trait]
impl Tool for BrowserCloseTool {
    fn name(&self) -> &str {
        "browser_close"
    }
    fn description(&self) -> &str {
        "Tear down the current browser session immediately (kill chromium, remove profile dir). Optional — every browser_open also tears down the previous session. Use this when you want to free resources without opening a new page."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_close(BROWSER_AGENT)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserOpenAndFillTool;

#[async_trait]
impl Tool for BrowserOpenAndFillTool {
    fn name(&self) -> &str {
        "browser_open_and_fill"
    }
    fn description(&self) -> &str {
        "ALL-IN-ONE: Navigate to URL + fill form + set dropdowns + click submit. Use this instead of separate browser_open/fill/click calls. Saves many tool rounds. Output: report of each action + page summary after."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to navigate to"},
                "fields": {"type": "object", "description": "Text fields: {\"first name\":\"Felix\",\"email\":\"a@b.com\"}", "additionalProperties": {"type": "string"}},
                "dropdowns": {"type": "object", "description": "Custom dropdowns: {\"Month\":\"Apr\",\"Gender\":\"Male\"}", "additionalProperties": {"type": "string"}},
                "submit": {"type": "boolean", "description": "Click submit/next button after filling (default false)"}
            },
            "required": ["url"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let url = arg_str(&args, "url");
        let fields = args.get("fields").cloned().unwrap_or_else(|| json!({}));
        let dropdowns = args
            .get("dropdowns")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let submit = args
            .get("submit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        browser::browser_open_and_fill(BROWSER_AGENT, url, &fields, &dropdowns, submit)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserFillFormTool;

#[async_trait]
impl Tool for BrowserFillFormTool {
    fn name(&self) -> &str {
        "browser_fill_form"
    }
    fn description(&self) -> &str {
        "Fill multiple form fields in ONE call on the current page. Auto-matches by name, placeholder, label, or nearby text. Handles inputs, textareas, and native selects. Use browser_open_and_fill if you also need to navigate. Output: ~500 chars."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "fields": {"type": "object", "description": "Field descriptions to values, e.g. {\"first name\":\"Felix\",\"email\":\"x@y.com\"}", "additionalProperties": {"type": "string"}}
            },
            "required": ["fields"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let fields = args.get("fields").cloned().unwrap_or_else(|| json!({}));
        browser::browser_fill_form(BROWSER_AGENT, &fields)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserFillTool;

#[async_trait]
impl Tool for BrowserFillTool {
    fn name(&self) -> &str {
        "browser_fill"
    }
    fn description(&self) -> &str {
        "Fill ONE field by selector/name/placeholder. Prefer browser_fill_form for multiple fields."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {"type": "string", "description": "CSS selector, name, placeholder, or aria-label"},
                "value": {"type": "string"}
            },
            "required": ["selector", "value"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_fill(
            BROWSER_AGENT,
            arg_str(&args, "selector"),
            arg_str(&args, "value"),
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct BrowserSelectTool;

#[async_trait]
impl Tool for BrowserSelectTool {
    fn name(&self) -> &str {
        "browser_select"
    }
    fn description(&self) -> &str {
        "Set a native HTML <select> dropdown. For custom/React dropdowns, use browser_set_dropdown instead."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {"type": "string", "description": "CSS selector, name, or id"},
                "value": {"type": "string", "description": "Option value or visible text"}
            },
            "required": ["selector", "value"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_select(
            BROWSER_AGENT,
            arg_str(&args, "selector"),
            arg_str(&args, "value"),
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct BrowserSetDropdownTool;

#[async_trait]
impl Tool for BrowserSetDropdownTool {
    fn name(&self) -> &str {
        "browser_set_dropdown"
    }
    fn description(&self) -> &str {
        "Set a custom/React dropdown (not native <select>). Clicks the dropdown trigger by label text, waits for options to appear, then clicks the matching option. Works with Facebook, Material UI, and other custom dropdowns. Use this for birthday month/day/year, gender, and similar custom UI dropdowns."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "label": {"type": "string", "description": "Text label or placeholder of the dropdown (e.g. 'Month', 'Day', 'Year', 'Gender', 'Select your gender')"},
                "value": {"type": "string", "description": "The option text to select (e.g. 'Apr', '1', '1985', 'Male')"}
            },
            "required": ["label", "value"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_set_dropdown(
            BROWSER_AGENT,
            arg_str(&args, "label"),
            arg_str(&args, "value"),
        )
        .await
        .map(RichToolResult::text)
    }
}

pub struct BrowserClickTool;

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str {
        "browser_click"
    }
    fn description(&self) -> &str {
        "Click an element in the browser by CSS selector"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {"type": "string", "description": "CSS selector for the element to click"}
            },
            "required": ["selector"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_click(BROWSER_AGENT, arg_str(&args, "selector"))
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserReadBriefTool;

#[async_trait]
impl Tool for BrowserReadBriefTool {
    fn name(&self) -> &str {
        "browser_read_brief"
    }
    fn description(&self) -> &str {
        "PREFERRED for checking page state. Returns title, URL, headings, input count, button labels, errors. Output: ~200 chars. Use this instead of browser_read to save context."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("browser"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_read_brief(BROWSER_AGENT)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserReadTool;

#[async_trait]
impl Tool for BrowserReadTool {
    fn name(&self) -> &str {
        "browser_read"
    }
    fn description(&self) -> &str {
        "Read page text content. Output: up to 2000 chars. Use browser_read_brief first — only use this if you need the actual text content."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("browser"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_read(BROWSER_AGENT)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserFindInputsTool;

#[async_trait]
impl Tool for BrowserFindInputsTool {
    fn name(&self) -> &str {
        "browser_find_inputs"
    }
    fn description(&self) -> &str {
        "List all form fields on current page: name, type, id, placeholder, options for selects. Use before browser_fill if you need exact selectors. Output: JSON."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("browser"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_find_inputs(BROWSER_AGENT)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserScreenshotTool;

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &str {
        "browser_screenshot"
    }
    fn description(&self) -> &str {
        "Save screenshot to workspace. Useful for debugging but does NOT return image content to you."
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_screenshot(BROWSER_AGENT, ctx.workspace)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserExecuteJsTool;

#[async_trait]
impl Tool for BrowserExecuteJsTool {
    fn name(&self) -> &str {
        "browser_execute_js"
    }
    fn description(&self) -> &str {
        "Run JavaScript on current page. Last resort — prefer specific browser tools. Output: JS return value."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "JavaScript expression (wrap in IIFE for complex logic)"}
            },
            "required": ["code"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        browser::browser_execute_js(BROWSER_AGENT, arg_str(&args, "code"))
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserClickAtTool;

#[async_trait]
impl Tool for BrowserClickAtTool {
    fn name(&self) -> &str {
        "browser_click_at"
    }
    fn description(&self) -> &str {
        "Click at exact x,y coordinates. Uses real X11 input (bypasses CAPTCHA detection). Find coords with browser_execute_js + getBoundingClientRect()."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": {"type": "number"},
                "y": {"type": "number"}
            },
            "required": ["x", "y"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        browser::browser_click_at(BROWSER_AGENT, x, y)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserHoldAtTool;

#[async_trait]
impl Tool for BrowserHoldAtTool {
    fn name(&self) -> &str {
        "browser_hold_at"
    }
    fn description(&self) -> &str {
        "Press and HOLD at x,y for N seconds. For CAPTCHA press-and-hold buttons. Uses real X11 input."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": {"type": "number"},
                "y": {"type": "number"},
                "duration": {"type": "integer", "description": "Seconds to hold (default 3, max 15)"}
            },
            "required": ["x", "y"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            destructive: false,
            idempotent: false,
            network: true,
            circuit_name: Some("browser"),
            ..ToolCapabilities::default()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let duration = arg_u64_or(&args, "duration", 3);
        browser::browser_hold_at(BROWSER_AGENT, x, y, duration)
            .await
            .map(RichToolResult::text)
    }
}

pub struct BrowserWaitTool;

#[async_trait]
impl Tool for BrowserWaitTool {
    fn name(&self) -> &str {
        "browser_wait"
    }
    fn description(&self) -> &str {
        "Wait for CSS selector to appear. Polls every 500ms."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {"type": "string"},
                "timeout": {"type": "integer", "description": "Max seconds (default 10)"}
            },
            "required": ["selector"]
        })
    }
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            circuit_name: Some("browser"),
            ..ToolCapabilities::read_network()
        }
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let selector = arg_str(&args, "selector");
        let timeout = arg_u64_or(&args, "timeout", 10);
        browser::browser_wait(BROWSER_AGENT, selector, timeout)
            .await
            .map(RichToolResult::text)
    }
}

// ── Todos ───────────────────────────────────────────────────────────────────

pub struct AddTodoTool;

#[async_trait]
impl Tool for AddTodoTool {
    fn name(&self) -> &str { "add_todo" }
    fn description(&self) -> &str { "Add a task to the user's to-do list. The task appears on their dashboard across all devices." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "text": { "type": "string", "description": "The task description" },
            "due_date": { "type": "string", "description": "Optional due date (YYYY-MM-DD)" }
        }, "required": ["text"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let text = arg_str(&args, "text");
        if text.is_empty() { return Err("text is required".into()); }
        let due = args.get("due_date").and_then(|v| v.as_str());
        let db = ctx.db_path.ok_or("database not available")?;
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let text_owned = text.to_string();
        let due_owned = due.map(String::from);
        let db_owned = db.to_path_buf();
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db_owned).map_err(|e| e.to_string())?;
            conn.execute("INSERT INTO todos (user_id, text, due_date, created_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![uid, &text_owned, &due_owned, now]).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;
        Ok(RichToolResult::text(format!("Added todo #{}: {}", id, text)))
    }
}

pub struct CompleteTodoTool;

#[async_trait]
impl Tool for CompleteTodoTool {
    fn name(&self) -> &str { "complete_todo" }
    fn description(&self) -> &str { "Mark a to-do item as completed." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "id": { "type": "integer", "description": "The todo ID to complete" }
        }, "required": ["id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = args.get("id").and_then(|v| v.as_i64()).ok_or("id is required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute("UPDATE todos SET done = 1, completed_at = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![now, id, uid]).map_err(|e| e.to_string())?;
            Ok(())
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;
        Ok(RichToolResult::text(format!("Completed todo #{}", id)))
    }
}

pub struct ListTodosTool;

#[async_trait]
impl Tool for ListTodosTool {
    fn name(&self) -> &str { "list_todos" }
    fn description(&self) -> &str { "List all to-do items for the current user." }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let todos = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare("SELECT id, text, done, due_date FROM todos WHERE user_id = ? ORDER BY done ASC, created_at DESC")
                .map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid], |r| {
                let id: i64 = r.get(0)?;
                let text: String = r.get(1)?;
                let done: bool = r.get::<_, i64>(2)? != 0;
                let due: Option<String> = r.get(3)?;
                let check = if done { "x" } else { " " };
                let due_str = due.map(|d| format!(" (due: {})", d)).unwrap_or_default();
                Ok(format!("[{}] #{} {}{}", check, id, text, due_str))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
            if rows.is_empty() { Ok("No todos.".to_string()) }
            else { Ok(rows.join("\n")) }
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;
        Ok(RichToolResult::text(todos))
    }
}

// ── Calendar Events ─────────────────────────────────────────────────────────

pub struct AddCalendarEventTool;

#[async_trait]
impl Tool for AddCalendarEventTool {
    fn name(&self) -> &str { "add_calendar_event" }
    fn description(&self) -> &str { "Add an event to the user's calendar. Appears on the dashboard calendar across all devices." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "title": { "type": "string", "description": "Event title" },
            "start_time": { "type": "string", "description": "Start time (ISO 8601 or YYYY-MM-DD for all-day)" },
            "end_time": { "type": "string", "description": "Optional end time" },
            "description": { "type": "string", "description": "Optional description" },
            "all_day": { "type": "boolean", "description": "True for all-day events" }
        }, "required": ["title", "start_time"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let title = arg_str(&args, "title");
        let start = arg_str(&args, "start_time");
        if title.is_empty() || start.is_empty() { return Err("title and start_time required".into()); }
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let title_owned = title.to_string();
        let desc = args.get("description").and_then(|v| v.as_str()).map(String::from);
        let start_owned = start.to_string();
        let end = args.get("end_time").and_then(|v| v.as_str()).map(String::from);
        let all_day = args.get("all_day").and_then(|v| v.as_bool()).unwrap_or(false);
        let source = format!("agent:{}", ctx.agent_id);
        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO calendar_events (user_id, title, description, start_time, end_time, all_day, source, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![uid, &title_owned, &desc, &start_owned, &end, all_day as i64, &source, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;
        Ok(RichToolResult::text(format!("Added calendar event #{}: {} on {}", id, title, start)))
    }
}

// ── Tax Module Tools ────────────────────────────────────────────────────────

pub struct LogExpenseTool;

#[async_trait]
impl Tool for LogExpenseTool {
    fn name(&self) -> &str { "log_expense" }
    fn description(&self) -> &str { "Log an expense to the user's tax tracker. Specify vendor, amount, category, and date." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "vendor": { "type": "string", "description": "Vendor or store name" },
            "amount": { "type": "string", "description": "Amount in dollars (e.g. '45.99')" },
            "category": { "type": "string", "description": "Expense category (e.g. 'Hardware & Supplies', 'Meals & Entertainment', 'Medical')" },
            "date": { "type": "string", "description": "Date (YYYY-MM-DD)" },
            "description": { "type": "string", "description": "Brief description" },
            "entity": { "type": "string", "description": "'business' or 'personal'" }
        }, "required": ["vendor", "amount", "date"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { read_only: false, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let vendor = arg_str(&args, "vendor");
        let amount = arg_str(&args, "amount");
        let date = arg_str(&args, "date");
        if vendor.is_empty() || amount.is_empty() || date.is_empty() {
            return Err("vendor, amount, and date are required".into());
        }
        let amount_cents = crate::tax::parse_cents(amount).ok_or("Invalid amount format")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let now = chrono::Utc::now().timestamp();
        let vendor_owned = vendor.to_string();
        let date_owned = date.to_string();
        let desc = args.get("description").and_then(|v| v.as_str()).map(String::from);
        let entity = args.get("entity").and_then(|v| v.as_str()).unwrap_or("personal").to_string();
        let cat = args.get("category").and_then(|v| v.as_str()).map(String::from);

        let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let category_id: Option<i64> = cat.as_deref().and_then(|c|
                conn.query_row("SELECT id FROM expense_categories WHERE name = ?", rusqlite::params![c], |r| r.get(0)).ok()
            );
            conn.execute(
                "INSERT INTO expenses (user_id, amount_cents, vendor, category_id, expense_date, description, entity, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![uid, amount_cents, &vendor_owned, category_id, &date_owned, &desc, &entity, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(format!("Logged expense #{}: {} {} at {} on {}", id, crate::tax::cents_to_display(amount_cents), vendor, vendor, date)))
    }
}

pub struct ExpenseSummaryTool;

#[async_trait]
impl Tool for ExpenseSummaryTool {
    fn name(&self) -> &str { "expense_summary" }
    fn description(&self) -> &str { "Get a summary of the user's expenses — totals by category for a given period." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "start": { "type": "string", "description": "Start date (YYYY-MM-DD), defaults to Jan 1 of current year" },
            "end": { "type": "string", "description": "End date (YYYY-MM-DD), defaults to Dec 31 of current year" },
            "entity": { "type": "string", "description": "Filter by 'business' or 'personal'" }
        }})
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let year = chrono::Utc::now().format("%Y").to_string();
        let start = args.get("start").and_then(|v| v.as_str()).unwrap_or(&format!("{}-01-01", year)).to_string();
        let end = args.get("end").and_then(|v| v.as_str()).unwrap_or(&format!("{}-12-31", year)).to_string();
        let entity_filter = args.get("entity").and_then(|v| v.as_str()).map(String::from);

        let summary = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let mut lines = vec![format!("Expense Summary ({} to {})", start, end)];

            // By category
            let mut sql = "SELECT c.name, c.entity, SUM(e.amount_cents), COUNT(*) \
                           FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
                           WHERE e.user_id = ? AND e.expense_date >= ? AND e.expense_date <= ?".to_string();
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
                Box::new(uid), Box::new(start.clone()), Box::new(end.clone()),
            ];
            if let Some(ref ent) = entity_filter {
                sql.push_str(" AND e.entity = ?");
                params.push(Box::new(ent.clone()));
            }
            sql.push_str(" GROUP BY c.id ORDER BY SUM(e.amount_cents) DESC");

            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(refs.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
            }).map_err(|e| e.to_string())?;

            let mut total: i64 = 0;
            lines.push(String::new());
            for r in rows {
                if let Ok((cat, ent, cents, count)) = r {
                    total += cents;
                    lines.push(format!("  {} ({}): {} ({} items)", cat, ent, crate::tax::cents_to_display(cents), count));
                }
            }
            lines.push(String::new());
            lines.push(format!("Total: {}", crate::tax::cents_to_display(total)));

            Ok(lines.join("\n"))
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(summary))
    }
}

pub struct GetIncomeTool;

#[async_trait]
impl Tool for GetIncomeTool {
    fn name(&self) -> &str { "get_income" }
    fn description(&self) -> &str { "Get the user's income records for a tax year. Returns W-2 wages, capital gains, dividends, interest, and totals." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "year": { "type": "integer", "description": "Tax year (e.g. 2025)" }
        }, "required": ["year"] })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let year = args.get("year").and_then(|v| v.as_i64()).unwrap_or(2025);
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let has_table: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tax_income'",
                [], |r| r.get::<_, i64>(0)
            ).unwrap_or(0) > 0;
            if !has_table { return Ok("No income data available.".into()); }

            let mut stmt = conn.prepare(
                "SELECT source, amount_cents, category, description FROM tax_income WHERE user_id = ? AND tax_year = ? ORDER BY amount_cents DESC"
            ).map_err(|e| e.to_string())?;
            let rows: Vec<String> = stmt.query_map(rusqlite::params![uid, year], |r| {
                let cents: i64 = r.get(1)?;
                Ok(format!("{}: {} ({})", r.get::<_, String>(0)?, crate::tax::cents_to_display(cents), r.get::<_, Option<String>>(3)?.unwrap_or_default()))
            }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

            let gross: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let withheld: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category = 'Federal Withholding'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);

            if rows.is_empty() { return Ok(format!("No income records for {}.", year)); }
            let mut result = format!("Income for {}:\n{}\n\nGross income: {}", year, rows.join("\n"), crate::tax::cents_to_display(gross));
            if withheld > 0 {
                result += &format!("\nFederal tax withheld: {}", crate::tax::cents_to_display(withheld));
            }
            Ok(result)
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

pub struct EstimateTaxTool;

#[async_trait]
impl Tool for EstimateTaxTool {
    fn name(&self) -> &str { "estimate_tax" }
    fn description(&self) -> &str { "Estimate federal tax liability for a given year based on income, deductions, and filing status. Uses the actual income and expense data from the tax module." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "year": { "type": "integer", "description": "Tax year (e.g. 2025)" },
            "filing_status": { "type": "string", "description": "Filing status: single, married_jointly, married_separately, head_of_household. Default: married_jointly" }
        }, "required": ["year"] })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let year = args.get("year").and_then(|v| v.as_i64()).unwrap_or(2025);
        let status = arg_str_or(&args, "filing_status", "married_jointly");
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let status_owned = status.to_string();

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

            // Get income (exclude withholding — that's a payment, not income)
            let gross_income: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);

            let start = format!("{}-01-01", year);
            let end = format!("{}-12-31", year);

            // Get business deductions (Schedule C)
            let biz_deductions: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND e.entity = 'business' AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // Get itemized deductions
            let itemized: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND c.tax_deductible = 1 AND e.entity = 'personal' AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // FICA already paid
            let fica_paid: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND (c.name LIKE 'FICA%') AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // Federal tax already paid
            let fed_paid: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND c.name LIKE 'Federal Income Tax%' AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // Standard deduction (2025, IRS Rev. Proc. 2024-40)
            let standard_deduction: i64 = match status_owned.as_str() {
                "married_jointly" => 3020000,   // $30,200
                "head_of_household" => 2265000, // $22,650
                _ => 1510000,                    // $15,100 single / married_separately
            };

            let deduction = std::cmp::max(standard_deduction, itemized);
            let deduction_type = if itemized > standard_deduction { "Itemized" } else { "Standard" };

            // Adjusted gross income
            let agi = gross_income - biz_deductions;
            let taxable = std::cmp::max(agi - deduction, 0);

            // 2025 tax brackets (married filing jointly)
            let tax = if status_owned == "married_jointly" {
                calculate_tax_mfj(taxable)
            } else {
                calculate_tax_single(taxable)
            };

            // W-2 withholding (box 2) - check if we have it
            let w2_withheld: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category LIKE '%Withholding%'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);

            // Credits against tax: W-2 withholding + estimated payments (NOT FICA — that's separate)
            let total_payments = w2_withheld + fed_paid;
            let owed = tax - total_payments;

            let d = |c: i64| crate::tax::cents_to_display(c);
            let mut result = format!(
                "Tax Estimate for {} ({}):\n\n\
                 Gross Income:              {}\n\
                 Business Deductions:      -{}\n\
                 Adjusted Gross Income:     {}\n\
                 {} Deduction:        -{}\n\
                 Taxable Income:            {}\n\n\
                 Federal Income Tax:        {}\n\n\
                 Credits / Payments:\n",
                year, status_owned.replace('_', " "),
                d(gross_income), d(biz_deductions), d(agi),
                deduction_type, d(deduction), d(taxable), d(tax));

            if w2_withheld > 0 {
                result += &format!("   W-2 Withholding:       -{}\n", d(w2_withheld));
            }
            if fed_paid > 0 {
                result += &format!("   Estimated Tax Paid:    -{}\n", d(fed_paid));
            }
            result += &format!("   Total Payments:        -{}\n", d(total_payments));
            result += &format!(" ────────────────────────────\n");
            result += &format!("   {}\n", if owed > 0 { format!("ESTIMATED TAX DUE: {}", d(owed)) } else { format!("ESTIMATED REFUND: {}", d(-owed)) });

            // Separate note about FICA
            if fica_paid > 0 {
                result += &format!("\nNote: FICA/payroll taxes ({}) are separate from federal income tax and are NOT credited against your income tax bill.\n", d(fica_paid));
            }

            if w2_withheld == 0 {
                result += &format!("\n⚠ IMPORTANT: This estimate does NOT include federal income tax withheld from your W-2 paychecks (Box 2 on your W-2 forms). \
                    This is typically the largest credit. Check your W-2s for the actual withholding amount — it will significantly reduce the amount owed.\n");
            }

            result += &format!("\n{}", if itemized > standard_deduction {
                format!("Itemizing saves {} over the standard deduction.", d(itemized - standard_deduction))
            } else {
                format!("Standard deduction saves {}. Itemized total: {}.", d(standard_deduction - itemized), d(itemized))
            });

            let effective_rate = if gross_income > 0 { (tax as f64 / gross_income as f64) * 100.0 } else { 0.0 };
            result += &format!("\nEffective federal tax rate: {:.1}%", effective_rate);

            Ok(result)
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

fn calculate_tax_mfj(taxable_cents: i64) -> i64 {
    // 2025 MFJ brackets (IRS Rev. Proc. 2024-40)
    let brackets: &[(i64, i64)] = &[
        (2385000, 1000),    // 10% up to $23,850
        (9695000, 1200),    // 12% up to $96,950
        (20670000, 2200),   // 22% up to $206,700
        (39460000, 2400),   // 24% up to $394,600
        (50105000, 3200),   // 32% up to $501,050
        (75160000, 3500),   // 35% up to $751,600
        (i64::MAX, 3700),   // 37% above
    ];
    let mut tax: i64 = 0;
    let mut prev = 0i64;
    for &(limit, rate_bps) in brackets {
        let bracket_income = std::cmp::min(taxable_cents, limit) - prev;
        if bracket_income <= 0 { break; }
        tax += bracket_income * rate_bps / 10000;
        prev = limit;
    }
    tax
}

fn calculate_tax_single(taxable_cents: i64) -> i64 {
    // 2025 Single brackets (IRS Rev. Proc. 2024-40)
    let brackets: &[(i64, i64)] = &[
        (1192500, 1000),    // 10% up to $11,925
        (4847500, 1200),    // 12% up to $48,475
        (10335000, 2200),   // 22% up to $103,350
        (19730000, 2400),   // 24% up to $197,300
        (25052500, 3200),   // 32% up to $250,525
        (62660000, 3500),   // 35% up to $626,600
        (i64::MAX, 3700),   // 37% above
    ];
    let mut tax: i64 = 0;
    let mut prev = 0i64;
    for &(limit, rate_bps) in brackets {
        let bracket_income = std::cmp::min(taxable_cents, limit) - prev;
        if bracket_income <= 0 { break; }
        tax += bracket_income * rate_bps / 10000;
        prev = limit;
    }
    tax
}

pub struct ScanReceiptTool;

#[async_trait]
impl Tool for ScanReceiptTool {
    fn name(&self) -> &str { "scan_receipt" }
    fn description(&self) -> &str { "Scan a receipt image that the user has uploaded. Extracts vendor, amount, date, and category using AI vision. The receipt must already be uploaded via the dashboard." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "receipt_id": { "type": "integer", "description": "The receipt ID to scan (from a pending upload)" }
        }, "required": ["receipt_id"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { read_only: false, ..Default::default() } }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let id = args.get("receipt_id").and_then(|v| v.as_i64()).ok_or("receipt_id required")?;
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();

        // Check receipt exists and is pending
        let status: String = {
            let db2 = db.clone();
            tokio::task::spawn_blocking(move || -> Result<String, String> {
                let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
                conn.query_row("SELECT status FROM receipts WHERE id = ?", rusqlite::params![id], |r| r.get(0))
                    .map_err(|e| format!("Receipt not found: {}", e))
            }).await.map_err(|e| e.to_string())?.map_err(|e| e)?
        };

        if status == "scanned" {
            return Ok(RichToolResult::text(format!("Receipt #{} has already been scanned.", id)));
        }

        Ok(RichToolResult::text(format!("Receipt #{} is queued for scanning. The vision AI will process it shortly and create an expense entry automatically.", id)))
    }
}
