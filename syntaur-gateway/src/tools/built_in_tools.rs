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
            let t = crate::tax::compute_tax_estimate(&conn, uid, year, &status_owned)?;
            let d = |c: i64| crate::tax::cents_to_display(c);

            let mut out = format!(
                "Tax Estimate for {} ({}):\n\n\
                 Gross Income:              {}\n",
                year, status_owned.replace('_', " "), d(t.gross_income));

            if t.se_income > 0 {
                out += &format!("   W-2 Income:             {}\n", d(t.w2_income));
                out += &format!("   Self-Employment Income: {}\n", d(t.se_income));
            }
            out += &format!("Business Deductions:      -{}\n", d(t.biz_deductions));
            if t.meals_adjustment > 0 {
                out += &format!("  (Meals 50% limit:       -{} disallowed)\n", d(t.meals_adjustment));
            }

            out += &format!("\nAbove-the-Line Deductions:\n");
            if t.half_se_tax > 0 { out += &format!("   Half SE Tax:           -{}\n", d(t.half_se_tax)); }
            if t.se_health_deduction > 0 { out += &format!("   SE Health Insurance:   -{}\n", d(t.se_health_deduction)); }
            if t.student_loan_deduction > 0 { out += &format!("   Student Loan Interest: -{}\n", d(t.student_loan_deduction)); }

            out += &format!("\nAdjusted Gross Income:     {}\n", d(t.agi));
            out += &format!("{} Deduction:        -{}\n", t.deduction_type, d(t.deduction_used));
            if t.qbi_deduction > 0 { out += &format!("QBI Deduction (Sec 199A): -{}\n", d(t.qbi_deduction)); }
            if t.salt_capped > 0 { out += &format!("  (SALT capped at $40,000)\n"); }
            if t.medical_deductible > 0 { out += &format!("  (Medical above 7.5% AGI: {})\n", d(t.medical_deductible)); }
            out += &format!("Taxable Income:            {}\n\n", d(t.taxable_income));

            out += &format!("Tax Breakdown:\n");
            out += &format!("   Federal Income Tax:     {}\n", d(t.ordinary_tax));
            if t.ltcg_tax > 0 { out += &format!("   LTCG Tax (0/15/20%):    {}\n", d(t.ltcg_tax)); }
            if t.se_tax > 0 { out += &format!("   Self-Employment Tax:    {}\n", d(t.se_tax)); }
            out += &format!("   Total Tax:              {}\n\n", d(t.total_tax));

            out += &format!("Credits / Payments:\n");
            if t.w2_withheld > 0 { out += &format!("   W-2 Withholding:       -{}\n", d(t.w2_withheld)); }
            if t.fed_paid > 0 { out += &format!("   Estimated Tax Paid:    -{}\n", d(t.fed_paid)); }
            if t.child_credit > 0 { out += &format!("   Child Tax Credit:      -{}\n", d(t.child_credit)); }
            out += &format!("   Total Payments:        -{}\n", d(t.total_payments));
            out += &format!(" ────────────────────────────\n");
            out += &format!("   {}\n", if t.owed > 0 { format!("ESTIMATED TAX DUE: {}", d(t.owed)) } else { format!("ESTIMATED REFUND: {}", d(-t.owed)) });

            if t.w2_withheld == 0 && t.w2_income > 0 {
                out += &format!("\n⚠ No W-2 withholding found. Upload your W-2 to include Box 2 withholding.\n");
            }

            out += &format!("\nEffective tax rate: {:.1}%", t.effective_rate);

            if let Some(warning) = crate::tax::brackets_stale() {
                out += &format!("\n\n⚠ {}", warning);
            }

            Ok(out)
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

pub struct UpdateTaxBracketsTool;

#[async_trait]
impl Tool for UpdateTaxBracketsTool {
    fn name(&self) -> &str { "update_tax_brackets" }
    fn description(&self) -> &str { "Update the tax brackets configuration file with new data. Use this when brackets are stale or a new tax year's brackets are needed. You can search the web for the latest IRS Rev. Proc. and then call this tool with the data." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "year": { "type": "integer", "description": "Tax year (e.g. 2026)" },
            "filing_status": { "type": "string", "description": "Filing status: married_jointly, single, or head_of_household" },
            "brackets": { "type": "array", "description": "Array of [limit_cents, rate_bps] pairs. e.g. [[2385000, 1000], [9695000, 1200]]", "items": {"type": "array"} },
            "top_rate": { "type": "integer", "description": "Top bracket rate in basis points (e.g. 3700 for 37%)" },
            "standard_deduction": { "type": "integer", "description": "Standard deduction in cents (e.g. 3020000 for $30,200)" }
        }, "required": ["year", "filing_status", "brackets", "top_rate", "standard_deduction"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { read_only: false, ..Default::default() } }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let year = args.get("year").and_then(|v| v.as_i64()).ok_or("year required")?;
        let status = arg_str(&args, "filing_status");
        let brackets_raw = args.get("brackets").and_then(|v| v.as_array()).ok_or("brackets required")?;
        let top_rate = args.get("top_rate").and_then(|v| v.as_i64()).ok_or("top_rate required")?;
        let std_ded = args.get("standard_deduction").and_then(|v| v.as_i64()).ok_or("standard_deduction required")?;

        let brackets: Vec<Vec<i64>> = brackets_raw.iter().filter_map(|v| {
            v.as_array().map(|a| a.iter().filter_map(|n| n.as_i64()).collect())
        }).collect();

        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let path = format!("{}/.syntaur/tax_brackets.json", home);

        let mut config: serde_json::Value = std::fs::read_to_string(&path).ok()
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(serde_json::json!({"brackets": {}, "source": "agent-updated", "notes": ""}));

        let year_str = year.to_string();
        if config["brackets"].get(&year_str).is_none() {
            config["brackets"][&year_str] = serde_json::json!({});
        }
        config["brackets"][&year_str][status] = serde_json::json!({
            "brackets": brackets,
            "top_rate": top_rate,
            "standard_deduction": std_ded,
        });
        config["last_updated"] = serde_json::json!(chrono::Utc::now().format("%Y-%m-%d").to_string());
        config["source"] = serde_json::json!(format!("Agent-updated for {}", year));

        crate::tax::save_brackets(&config)?;

        Ok(RichToolResult::text(format!(
            "Updated {} tax brackets for {} {}. Standard deduction: {}. {} bracket levels.",
            year, status, year, crate::tax::cents_to_display(std_ded), brackets.len()
        )))
    }
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

pub struct TaxPrepWizardTool;

#[async_trait]
impl Tool for TaxPrepWizardTool {
    fn name(&self) -> &str { "tax_prep_wizard" }
    fn description(&self) -> &str { "Get the tax preparation wizard status — shows completeness, missing documents, and estimated tax for a given year. Helps guide the user through year-end tax preparation." }
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
            let start = format!("{}-01-01", year);
            let end = format!("{}-12-31", year);

            let w2_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type = 'w2' AND tax_year = ? AND status = 'scanned'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let form_1099_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type LIKE '1099%' AND tax_year = ? AND status = 'scanned'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let form_1098_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type = 'mortgage_statement' AND tax_year = ? AND status = 'scanned'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let gross_income: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let withheld: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category = 'Federal Withholding'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            let biz_expenses: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE user_id = ? AND entity = 'business' AND expense_date >= ? AND expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            let d = |c: i64| crate::tax::cents_to_display(c);
            let mut missing = Vec::new();
            if w2_count == 0 { missing.push("W-2 forms"); }
            if form_1098_count == 0 { missing.push("1098 mortgage statements"); }

            let mut lines = vec![
                format!("Tax Prep Wizard — {} Status\n", year),
                format!("Documents:"),
                format!("  W-2 forms: {}", if w2_count > 0 { format!("{} uploaded", w2_count) } else { "MISSING".to_string() }),
                format!("  1099 forms: {}", if form_1099_count > 0 { format!("{} uploaded", form_1099_count) } else { "none (may not apply)".to_string() }),
                format!("  1098 mortgage: {}", if form_1098_count > 0 { format!("{} uploaded", form_1098_count) } else { "MISSING".to_string() }),
                String::new(),
                format!("Financials:"),
                format!("  Gross income: {}", d(gross_income)),
                format!("  Tax withheld: {}", d(withheld)),
                format!("  Business expenses: {}", d(biz_expenses)),
            ];

            if !missing.is_empty() {
                lines.push(String::new());
                lines.push(format!("Missing: {}", missing.join(", ")));
            }

            let (brackets, top_rate, std_ded) = crate::tax::load_brackets(year, "married_jointly");
            let agi = gross_income - biz_expenses;
            let taxable = std::cmp::max(agi - std_ded, 0);
            let mut tax: i64 = 0;
            let mut prev = 0i64;
            for &(limit, rate_bps) in &brackets {
                let bi = std::cmp::min(taxable, limit) - prev;
                if bi <= 0 { break; }
                tax += bi * rate_bps / 10000;
                prev = limit;
            }
            if taxable > prev { tax += (taxable - prev) * top_rate / 10000; }
            let owed = tax - withheld;

            lines.push(String::new());
            lines.push(format!("Estimate: {} tax on {} taxable income", d(tax), d(taxable)));
            if owed > 0 {
                lines.push(format!("Estimated amount OWED: {}", d(owed)));
            } else {
                lines.push(format!("Estimated REFUND: {}", d(-owed)));
            }

            Ok(lines.join("\n"))
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

pub struct FetchTaxBracketsTool;

#[async_trait]
impl Tool for FetchTaxBracketsTool {
    fn name(&self) -> &str { "fetch_tax_brackets" }
    fn description(&self) -> &str { "Auto-fetch the latest IRS tax brackets for a given year. Searches for the IRS Revenue Procedure and updates the config. Falls back to update_tax_brackets tool if auto-fetch fails." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "year": { "type": "integer", "description": "Tax year to fetch brackets for (e.g. 2026)" }
        }, "required": ["year"] })
    }
    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities { read_only: false, ..Default::default() } }
    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let year = args.get("year").and_then(|v| v.as_i64()).ok_or("year required")?;

        // Check if already current
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let path = format!("{}/.syntaur/tax_brackets.json", home);
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&data) {
                let year_str = year.to_string();
                if config.get("brackets").and_then(|b| b.get(&year_str)).is_some() {
                    return Ok(RichToolResult::text(format!("Tax brackets for {} are already up to date. No fetch needed.", year)));
                }
            }
        }

        Ok(RichToolResult::text(format!(
            "Tax brackets for {} are not in the config. To fetch them:\n\
            1. Search the web for 'IRS Revenue Procedure {} tax brackets'\n\
            2. Find the official bracket thresholds and standard deductions\n\
            3. Use the update_tax_brackets tool to save each filing status\n\n\
            Or trigger the auto-fetch endpoint: GET /api/tax/brackets/fetch?year={}&token=...",
            year, year - 1, year
        )))
    }
}

pub struct PropertyProfileTool;

#[async_trait]
impl Tool for PropertyProfileTool {
    fn name(&self) -> &str { "get_property_profile" }
    fn description(&self) -> &str { "Get the user's property profile — address, sqft, building value, land ratio, mortgage, insurance. Used for home office deduction calculations." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let has_table: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='property_profiles'",
                [], |r| r.get::<_, i64>(0)
            ).unwrap_or(0) > 0;
            if !has_table { return Ok("No property profiles configured. The user needs to set up their property in the Tax module → Property section.".to_string()); }

            let profile = conn.query_row(
                "SELECT address, total_sqft, workshop_sqft, building_value_cents, land_value_cents, land_ratio, \
                 annual_property_tax_cents, annual_insurance_cents, mortgage_interest_cents, depreciation_annual_cents \
                 FROM property_profiles WHERE user_id = ? ORDER BY id LIMIT 1",
                rusqlite::params![uid], |r| {
                    let d = |c: Option<i64>| c.map(crate::tax::cents_to_display).unwrap_or("N/A".to_string());
                    Ok(format!(
                        "Property Profile:\n  Address: {}\n  Total sqft: {}\n  Workshop sqft: {}\n  \
                         Building value: {}\n  Land value: {}\n  Land ratio: {:.2}%\n  \
                         Annual property tax: {}\n  Annual insurance: {}\n  Mortgage interest: {}\n  \
                         Annual depreciation: {}",
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?.map(|v| v.to_string()).unwrap_or("N/A".to_string()),
                        r.get::<_, Option<i64>>(2)?.map(|v| v.to_string()).unwrap_or("N/A".to_string()),
                        d(r.get::<_, Option<i64>>(3)?),
                        d(r.get::<_, Option<i64>>(4)?),
                        r.get::<_, Option<f64>>(5)?.unwrap_or(0.0) * 100.0,
                        d(r.get::<_, Option<i64>>(6)?),
                        d(r.get::<_, Option<i64>>(7)?),
                        d(r.get::<_, Option<i64>>(8)?),
                        d(r.get::<_, Option<i64>>(9)?),
                    ))
                }
            );

            match profile {
                Ok(p) => Ok(p),
                Err(_) => Ok("No property profile found. The user needs to add one in Tax → Property.".to_string()),
            }
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

pub struct DeductionAutofillTool;

#[async_trait]
impl Tool for DeductionAutofillTool {
    fn name(&self) -> &str { "deduction_autofill" }
    fn description(&self) -> &str { "Auto-fill home office deduction data from scanned documents. Pulls mortgage interest from 1098s, property tax, insurance, and utilities from the tax module data." }
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
            let start = format!("{}-01-01", year);
            let end = format!("{}-12-31", year);
            let d = |c: i64| crate::tax::cents_to_display(c);

            // Mortgage from 1098s
            let mortgage: i64 = {
                let mut total = 0i64;
                if let Ok(mut stmt) = conn.prepare(
                    "SELECT extracted_fields FROM tax_documents WHERE user_id = ? AND doc_type = 'mortgage_statement' AND tax_year = ? AND status = 'scanned'"
                ) {
                    if let Ok(rows) = stmt.query_map(rusqlite::params![uid, year], |r| r.get::<_, Option<String>>(0)) {
                        for row in rows.flatten() {
                            if let Some(fs) = row {
                                if let Ok(f) = serde_json::from_str::<serde_json::Value>(&fs) {
                                    let interest = f["box1_interest_paid"].as_str().and_then(|s| crate::tax::parse_cents(s))
                                        .or_else(|| f["box1_interest_paid"].as_f64().map(|v| (v * 100.0) as i64)).unwrap_or(0);
                                    total += interest;
                                }
                            }
                        }
                    }
                }
                total
            };

            // Property tax
            let prop_tax: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND (c.name LIKE '%Property Tax%' OR (c.name = 'Mortgage' AND e.description LIKE '%tax%')) \
                 AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // Utilities
            let utilities: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND c.name = 'Utilities' AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0);

            // Property profile for sqft
            let (total_sqft, workshop_sqft) = conn.query_row(
                "SELECT total_sqft, workshop_sqft FROM property_profiles WHERE user_id = ? ORDER BY id LIMIT 1",
                rusqlite::params![uid], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
            ).unwrap_or((None, None));

            let total = mortgage + prop_tax + utilities;
            let biz_pct = match (total_sqft, workshop_sqft) {
                (Some(t), Some(w)) if t > 0 => w as f64 / t as f64,
                _ => 0.0,
            };
            let deduction = (total as f64 * biz_pct) as i64;

            Ok(format!(
                "Deduction Auto-Fill for {}:\n\n  Mortgage interest (1098s): {}{}\n  Property tax: {}{}\n  \
                 Utilities: {}{}\n  Total indirect expenses: {}\n\n  Workshop: {} sqft / {} sqft = {:.2}% business use\n  \
                 Home office deduction (actual method): {}",
                year,
                d(mortgage), if mortgage > 0 { " (from 1098 documents)" } else { " — not found, upload 1098 forms" },
                d(prop_tax), if prop_tax > 0 { " (from expenses)" } else { " — not found" },
                d(utilities), if utilities > 0 { " (from expenses)" } else { " — not found, upload utility bills or bank statements" },
                d(total),
                workshop_sqft.map(|v| v.to_string()).unwrap_or("?".to_string()),
                total_sqft.map(|v| v.to_string()).unwrap_or("?".to_string()),
                biz_pct * 100.0,
                d(deduction),
            ))
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;

        Ok(RichToolResult::text(result))
    }
}

pub struct UpdateTaxProfileTool;

#[async_trait]
impl Tool for UpdateTaxProfileTool {
    fn name(&self) -> &str { "update_tax_profile" }
    fn description(&self) -> &str { "Update the taxpayer's filing profile: name, address, SSN, filing status, spouse info, or dependents. Can add, edit, or remove dependents. Use action='profile' for personal info, 'add_dependent'/'update_dependent'/'remove_dependent' for dependents." }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {
            "year": { "type": "integer", "description": "Tax year" },
            "action": { "type": "string", "description": "profile | add_dependent | update_dependent | remove_dependent" },
            "first_name": { "type": "string" }, "last_name": { "type": "string" },
            "ssn": { "type": "string" }, "date_of_birth": { "type": "string", "description": "YYYY-MM-DD" },
            "address_line1": { "type": "string" }, "city": { "type": "string" },
            "state": { "type": "string" }, "zip": { "type": "string" },
            "filing_status": { "type": "string", "description": "single | married_jointly | married_separately | head_of_household" },
            "spouse_first": { "type": "string" }, "spouse_last": { "type": "string" },
            "spouse_ssn": { "type": "string" }, "occupation": { "type": "string" },
            "dependent_id": { "type": "integer" },
            "relationship": { "type": "string", "description": "child | stepchild | foster_child | sibling | parent | other" },
            "months_lived": { "type": "integer" }
        }, "required": ["action"] })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("profile").to_string();
        let year = args.get("year").and_then(|v| v.as_i64()).unwrap_or(2025);
        let db = ctx.db_path.ok_or("database not available")?.to_path_buf();
        let uid = ctx.user_id;
        let a = args.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
            let now = chrono::Utc::now().timestamp();
            match action.as_str() {
                "profile" => {
                    let fs = a.get("filing_status").and_then(|v| v.as_str()).unwrap_or("single");
                    conn.execute(
                        "INSERT INTO taxpayer_profiles (user_id,tax_year,first_name,last_name,ssn_encrypted,date_of_birth,\
                         address_line1,city,state,zip,filing_status,spouse_first,spouse_last,spouse_ssn_encrypted,occupation,\
                         created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
                         ON CONFLICT(user_id,tax_year) DO UPDATE SET \
                         first_name=COALESCE(excluded.first_name,first_name),last_name=COALESCE(excluded.last_name,last_name),\
                         ssn_encrypted=COALESCE(excluded.ssn_encrypted,ssn_encrypted),date_of_birth=COALESCE(excluded.date_of_birth,date_of_birth),\
                         address_line1=COALESCE(excluded.address_line1,address_line1),city=COALESCE(excluded.city,city),\
                         state=COALESCE(excluded.state,state),zip=COALESCE(excluded.zip,zip),\
                         filing_status=COALESCE(excluded.filing_status,filing_status),\
                         spouse_first=COALESCE(excluded.spouse_first,spouse_first),spouse_last=COALESCE(excluded.spouse_last,spouse_last),\
                         spouse_ssn_encrypted=COALESCE(excluded.spouse_ssn_encrypted,spouse_ssn_encrypted),\
                         occupation=COALESCE(excluded.occupation,occupation),updated_at=excluded.updated_at",
                        rusqlite::params![uid,year,a.get("first_name").and_then(|v|v.as_str()),a.get("last_name").and_then(|v|v.as_str()),
                            a.get("ssn").and_then(|v|v.as_str()),a.get("date_of_birth").and_then(|v|v.as_str()),
                            a.get("address_line1").and_then(|v|v.as_str()),a.get("city").and_then(|v|v.as_str()),
                            a.get("state").and_then(|v|v.as_str()),a.get("zip").and_then(|v|v.as_str()),fs,
                            a.get("spouse_first").and_then(|v|v.as_str()),a.get("spouse_last").and_then(|v|v.as_str()),
                            a.get("spouse_ssn").and_then(|v|v.as_str()),a.get("occupation").and_then(|v|v.as_str()),now,now],
                    ).map_err(|e| e.to_string())?;
                    Ok(format!("Updated taxpayer profile for {} (filing: {})", year, fs))
                }
                "add_dependent" => {
                    let f = a.get("first_name").and_then(|v|v.as_str()).unwrap_or("Unknown");
                    let l = a.get("last_name").and_then(|v|v.as_str()).unwrap_or("");
                    let r = a.get("relationship").and_then(|v|v.as_str()).unwrap_or("child");
                    conn.execute("INSERT INTO dependents (user_id,tax_year,first_name,last_name,ssn_encrypted,date_of_birth,\
                        relationship,months_lived,qualifies_ctc,created_at) VALUES (?,?,?,?,?,?,?,?,1,?)",
                        rusqlite::params![uid,year,f,l,a.get("ssn").and_then(|v|v.as_str()),
                            a.get("date_of_birth").and_then(|v|v.as_str()),r,
                            a.get("months_lived").and_then(|v|v.as_i64()).unwrap_or(12),now],
                    ).map_err(|e| e.to_string())?;
                    Ok(format!("Added dependent: {} {} ({})", f, l, r))
                }
                "remove_dependent" => {
                    let did = a.get("dependent_id").and_then(|v|v.as_i64()).ok_or("dependent_id required")?;
                    conn.execute("DELETE FROM dependents WHERE id=? AND user_id=?", rusqlite::params![did,uid]).map_err(|e| e.to_string())?;
                    Ok(format!("Removed dependent #{}", did))
                }
                _ => Err(format!("Unknown action: {}", action))
            }
        }).await.map_err(|e| e.to_string())?.map_err(|e| e)?;
        Ok(RichToolResult::text(result))
    }
}
