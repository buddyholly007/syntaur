//! Image generation tool — async, non-blocking.
//!
//! Spawns a background task for image generation via OpenRouter's
//! image-capable models (Seedream 4.5 free tier). Returns immediately
//! with a task_id. The chat response includes `pending_tasks` so the
//! frontend can poll for completion.
//!
//! When the image is ready, it's stored as a result on the background
//! task AND optionally appended to the conversation as an assistant
//! message so the user sees it on next refresh.

use async_trait::async_trait;
use log::{error, info};
use serde_json::{json, Value};
use std::sync::Arc;

use super::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct GenerateImageTool {
    pub task_manager: Arc<crate::background_tasks::BackgroundTaskManager>,
    pub config: Arc<crate::config::Config>,
    pub http: reqwest::Client,
    pub conversations: Option<Arc<crate::conversations::ConversationManager>>,
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str { "generate_image" }

    fn description(&self) -> &str {
        "Generate an image from a text description. This runs in the background — \
         you'll get a task_id immediately and the image will appear in the conversation \
         when ready (usually 5-15 seconds). Continue talking to the user while it generates."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Detailed description of the image to generate. Be specific about style, content, colors, composition."
                },
                "size": {
                    "type": "string",
                    "enum": ["1024x1024", "1024x1536", "1536x1024"],
                    "description": "Image dimensions. Default: 1024x1024 (square)."
                }
            },
            "required": ["prompt"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities { network: true, ..ToolCapabilities::default() }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let prompt = args.get("prompt").and_then(|v| v.as_str())
            .ok_or("prompt is required")?;
        let size = args.get("size").and_then(|v| v.as_str()).unwrap_or("1024x1024");

        if prompt.len() < 3 { return Err("Prompt too short".to_string()); }
        if prompt.len() > 2000 { return Err("Prompt too long (max 2000 chars)".to_string()); }

        // Create background task
        let conv_id = ctx.conversation_id.clone();
        let task_id = self.task_manager.create(
            ctx.user_id,
            conv_id.clone(),
            ctx.agent_id,
            "image_generate",
            &format!("{}... ({})", &prompt[..prompt.len().min(60)], size),
        ).await;

        info!("[image] spawning generation task {} for user {}: {}...",
            task_id, ctx.user_id, &prompt[..prompt.len().min(40)]);

        // Spawn the actual generation in the background
        let tm = Arc::clone(&self.task_manager);
        let http = self.http.clone();
        let api_key = self.config.models.providers.values()
            .find(|p| p.base_url.contains("openrouter"))
            .map(|p| p.api_key.clone())
            .unwrap_or_default();
        let prompt_owned = prompt.to_string();
        let size_owned = size.to_string();
        let tid = task_id.clone();
        let convs = self.conversations.clone();
        let conv_id = conv_id;

        tokio::spawn(async move {
            let result = call_image_api(&http, &api_key, &prompt_owned, &size_owned).await;

            match result {
                Ok(image_url) => {
                    info!("[image] task {} completed: {}", tid, &image_url[..image_url.len().min(80)]);
                    tm.complete(&tid, json!({
                        "url": image_url,
                        "prompt": prompt_owned,
                        "size": size_owned,
                    })).await;
                    // Append image to conversation so the agent can see it next turn
                    if let (Some(ref cid), Some(ref cm)) = (&conv_id, &convs) {
                        let img_msg = format!(
                            "![Generated image]({})

Generated image from prompt: {}",
                            image_url, prompt_owned
                        );
                        let _ = cm.append(cid, "assistant", &img_msg).await;
                    }
                }
                Err(e) => {
                    error!("[image] task {} failed: {}", tid, e);
                    tm.fail(&tid, &e).await;
                }
            }
        });

        Ok(RichToolResult::text(format!(
            "Image generation started (task: {}). The image will appear in the conversation \
             when ready — continue talking to the user in the meantime. \
             Don't ask them to wait.",
            task_id
        )))
    }
}


pub struct EditImageTool {
    pub task_manager: Arc<crate::background_tasks::BackgroundTaskManager>,
    pub config: Arc<crate::config::Config>,
    pub http: reqwest::Client,
    pub conversations: Option<Arc<crate::conversations::ConversationManager>>,
}

#[async_trait]
impl Tool for EditImageTool {
    fn name(&self) -> &str { "edit_image" }

    fn description(&self) -> &str {
        "Edit an existing image based on a modification instruction. Provide the \
         original image URL and describe what to change. The system will analyze \
         the original and generate a refined version. For best results, be specific \
         about what to keep and what to change."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "original_url": {"type": "string", "description": "URL of the image to edit"},
                "edit_instruction": {"type": "string", "description": "What to change (e.g., 'make the sky darker', 'add clouds', 'change background to forest')"},
                "preserve": {"type": "string", "description": "What to keep from the original (e.g., 'keep the mountains and foreground')"},
                "size": {"type": "string", "enum": ["1024x1024", "1024x1536", "1536x1024"]}
            },
            "required": ["original_url", "edit_instruction"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities { network: true, ..ToolCapabilities::default() }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let original_url = args.get("original_url").and_then(|v| v.as_str())
            .ok_or("original_url required")?.to_string();
        let edit_instruction = args.get("edit_instruction").and_then(|v| v.as_str())
            .ok_or("edit_instruction required")?.to_string();
        let preserve = args.get("preserve").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let size = args.get("size").and_then(|v| v.as_str()).unwrap_or("1024x1024").to_string();

        // Step 1: Use vision to describe the original image
        let chain = crate::llm::LlmChain::from_config_fast(&self.config, "main", self.http.clone());
        let vision_messages = vec![
            crate::llm::ChatMessage::system(
                "Describe this image in detail: composition, colors, subjects, style, lighting.                  Be specific enough that someone could recreate it from your description.                  Respond with ONLY the description, no preamble."
            ),
            crate::llm::ChatMessage::user_with_images("Describe this image.", &[original_url.clone()]),
        ];

        let description = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            chain.call(&vision_messages)
        ).await {
            Ok(Ok(desc)) => desc,
            _ => "original image".to_string(), // fallback if vision fails
        };

        // Step 2: Build refined prompt combining description + edit
        let refined_prompt = if preserve.is_empty() {
            format!(
                "Based on this image: {}. Apply this edit: {}.                  Keep the overall composition and style consistent.",
                description.trim(), edit_instruction
            )
        } else {
            format!(
                "Based on this image: {}. Apply this edit: {}.                  Preserve: {}. Keep style consistent.",
                description.trim(), edit_instruction, preserve
            )
        };

        // Step 3: Generate new image with refined prompt (async)
        let conv_id = ctx.conversation_id.clone();
        let task_id = self.task_manager.create(
            ctx.user_id, conv_id.clone(), ctx.agent_id,
            "image_edit", &format!("Edit: {}", &edit_instruction[..edit_instruction.len().min(50)]),
        ).await;

        let tm = Arc::clone(&self.task_manager);
        let http = self.http.clone();
        let api_key = self.config.models.providers.values()
            .find(|p| p.base_url.contains("openrouter"))
            .map(|p| p.api_key.clone()).unwrap_or_default();
        let tid = task_id.clone();
        let convs = self.conversations.clone();
        let edit_desc = edit_instruction.to_string();
        let size_owned = size.to_string();

        tokio::spawn(async move {
            match call_image_api(&http, &api_key, &refined_prompt, &size_owned).await {
                Ok(url) => {
                    info!("[image-edit] task {} completed", tid);
                    tm.complete(&tid, json!({
                        "url": url,
                        "edit": edit_desc,
                        "based_on": original_url.clone(),
                    })).await;
                    if let (Some(ref cid), Some(ref cm)) = (&conv_id, &convs) {
                        let msg = format!("![Edited image]({})

Edited: {}", url, edit_desc);
                        let _ = cm.append(cid, "assistant", &msg).await;
                    }
                }
                Err(e) => { error!("[image-edit] task {} failed: {}", tid, e); tm.fail(&tid, &e).await; }
            }
        });

        Ok(RichToolResult::text(format!(
            "Image edit started (task: {}). Analyzing the original and generating an edited version              with: '{}'. Continue talking — the result will appear shortly.",
            task_id, edit_instruction
        )))
    }
}

pub struct SaveImageTool;

#[async_trait]
impl Tool for SaveImageTool {
    fn name(&self) -> &str { "save_image" }

    fn description(&self) -> &str {
        "Save a generated image to your memory so you can reference it later.          Use when the user likes an image and might want to come back to it."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The image URL to save"},
                "title": {"type": "string", "description": "A memorable title for the image"},
                "description": {"type": "string", "description": "What the image shows"},
                "tags": {"type": "string", "description": "Comma-separated tags"}
            },
            "required": ["url", "title"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities { ToolCapabilities::default() }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let url = args.get("url").and_then(|v| v.as_str()).ok_or("url required")?;
        let title = args.get("title").and_then(|v| v.as_str()).ok_or("title required")?;
        let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let tags = args.get("tags").and_then(|v| v.as_str()).unwrap_or("image");

        let db = ctx.db_path.ok_or("No database")?;
        let conn = rusqlite::Connection::open(db).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp();
        let key = format!("img_{}", &title.to_lowercase().replace(' ', "_")[..title.len().min(30)]);

        conn.execute(
            "INSERT INTO agent_memories              (user_id, agent_id, memory_type, key, title, description, content, tags,               importance, shared, source, created_at, updated_at)              VALUES (?1, ?2, 'reference', ?3, ?4, ?5, ?6, ?7, 6, 1, 'agent_learned', ?8, ?8)              ON CONFLICT(user_id, agent_id, key) DO UPDATE SET                content = excluded.content, updated_at = excluded.updated_at",
            rusqlite::params![ctx.user_id, ctx.agent_id, key, title, description, url, tags, now],
        ).map_err(|e| format!("save image: {}", e))?;

        Ok(RichToolResult::text(format!("Image saved to memory: '{}'. You can recall it later with memory_recall.", title)))
    }
}

/// Generate an image via OpenRouter.
///
/// OpenRouter does NOT expose the OpenAI `/v1/images/generations` endpoint —
/// image-capable models return generated images as base64 data URLs in
/// `choices[0].message.images[]` through regular chat/completions. The
/// previous `bytedance/seedream-4.5` + `/v1/images/generations` path hit
/// OpenRouter's Next.js 404 page because that endpoint doesn't exist on
/// their platform (and that model isn't listed either).
///
/// Model: `google/gemini-2.5-flash-image` — cheapest image-output model
/// on OpenRouter as of 2026-04 (~$0.001 per image). For higher quality:
/// `google/gemini-3-pro-image-preview` or `openai/gpt-5-image`.
///
/// `size` is accepted for API compatibility but ignored — Gemini doesn't
/// take an explicit size argument, it generates at its default (roughly
/// square, 1024-ish px). Callers that want a specific aspect should
/// include it in the prompt ("square", "landscape 16:9", etc).
async fn call_image_api(
    http: &reqwest::Client,
    api_key: &str,
    prompt: &str,
    _size: &str,
) -> Result<String, String> {
    let body = json!({
        "model": "google/gemini-2.5-flash-image",
        "messages": [{
            "role": "user",
            "content": format!("Generate an image: {}", prompt),
        }],
    });

    let resp = http
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| format!("image API request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("image API error {}: {}", status, &body[..body.len().min(200)]));
    }

    let data: Value = resp.json().await
        .map_err(|e| format!("image API response parse error: {}", e))?;

    // Response shape: choices[0].message.images[0].image_url.url = "data:image/png;base64,..."
    if let Some(url) = data
        .pointer("/choices/0/message/images/0/image_url/url")
        .and_then(|v| v.as_str())
    {
        return Ok(url.to_string());
    }

    // Model refused or returned no image — surface the reason so the user
    // sees why instead of a generic "No image" error.
    let refusal = data
        .pointer("/choices/0/message/refusal")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = data
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !refusal.is_empty() {
        Err(format!("image model declined: {}", refusal))
    } else if !content.is_empty() {
        Err(format!(
            "no image returned (model replied with text): {}",
            &content[..content.len().min(200)]
        ))
    } else {
        Err("image API returned no image and no explanation".to_string())
    }
}
