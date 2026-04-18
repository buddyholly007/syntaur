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

        // Spawn the actual generation in the background. Config is already
        // available on the tool struct — clone the Arc and pass it through
        // so the provider dispatch can see the user's choice.
        let tm = Arc::clone(&self.task_manager);
        let http = self.http.clone();
        let config = Arc::clone(&self.config);
        let prompt_owned = prompt.to_string();
        let size_owned = size.to_string();
        let tid = task_id.clone();
        let convs = self.conversations.clone();
        let conv_id = conv_id;

        tokio::spawn(async move {
            let result = call_image_api(&http, &config, &prompt_owned, &size_owned).await;

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
        let config = Arc::clone(&self.config);
        let tid = task_id.clone();
        let convs = self.conversations.clone();
        let edit_desc = edit_instruction.to_string();
        let size_owned = size.to_string();

        tokio::spawn(async move {
            match call_image_api(&http, &config, &refined_prompt, &size_owned).await {
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

/// Provider for image generation. Dispatch is chosen at call time from the
/// `image_gen` section of openclaw.json, with Pollinations.ai as the
/// zero-config default so a fresh install generates images without any
/// signup, API key, or paid account.
///
/// Options, in the order we prefer them:
/// - `local`     — user has ComfyUI / SD.Next running somewhere (Gaming PC
///                 RTX 3090 is the intended target). Best quality, fully
///                 private, free forever. Requires local setup.
/// - `pollinations` — free public API at image.pollinations.ai, no auth,
///                 no key, no signup. Returns a stable cached URL. Quality
///                 is decent (FLUX under the hood). Default.
/// - `openrouter` — paid OpenRouter image-capable model via /chat/completions.
///                 Best for users without a GPU who are willing to pay.
enum ImageProvider<'a> {
    Pollinations,
    LocalSd { base_url: &'a str, model: Option<&'a str> },
    OpenrouterPaid { api_key: &'a str, model: &'a str },
}

fn select_image_provider(config: &crate::config::Config) -> ImageProvider<'_> {
    // 1. Local Stable Diffusion, if configured
    if let Some(url) = config.image_gen.local_sd_url.as_deref() {
        if !url.trim().is_empty() {
            return ImageProvider::LocalSd {
                base_url: url,
                model: config.image_gen.local_sd_model.as_deref(),
            };
        }
    }
    // 2. OpenRouter paid, only if the user has explicitly set a model id
    if let Some(model) = config.image_gen.openrouter_paid_model.as_deref() {
        if !model.trim().is_empty() {
            let key = config.models.providers.values()
                .find(|p| p.base_url.contains("openrouter"))
                .map(|p| p.api_key.as_str())
                .unwrap_or("");
            if !key.is_empty() {
                return ImageProvider::OpenrouterPaid { api_key: key, model };
            }
        }
    }
    // 3. Default: free public Pollinations
    ImageProvider::Pollinations
}

async fn call_image_api(
    http: &reqwest::Client,
    config: &crate::config::Config,
    prompt: &str,
    size: &str,
) -> Result<String, String> {
    match select_image_provider(config) {
        ImageProvider::Pollinations => call_pollinations(http, prompt, size).await,
        ImageProvider::LocalSd { base_url, model } => call_local_sd(http, base_url, model, prompt, size).await,
        ImageProvider::OpenrouterPaid { api_key, model } => call_openrouter_chat(http, api_key, model, prompt).await,
    }
}

/// Free image generation via Pollinations.ai. No key, no auth, no signup.
/// GET request returns the image bytes directly; we return the URL itself
/// (Pollinations caches the output so subsequent hits are fast).
///
/// Terms of service (as of 2026-04): no rate limit published, anonymous use
/// allowed, includes an unobtrusive watermark unless `nologo=true`. We set
/// nologo since the user asked for their own image. If Pollinations adds
/// auth requirements in the future, this path will 4xx and we surface it.
async fn call_pollinations(
    http: &reqwest::Client,
    prompt: &str,
    size: &str,
) -> Result<String, String> {
    let (width, height) = match size {
        "1024x1536" => (1024u32, 1536u32),
        "1536x1024" => (1536, 1024),
        _ => (1024, 1024),
    };
    // URL-encode the prompt — Pollinations puts it right in the path.
    let encoded: String = prompt
        .chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect()
            }
        })
        .collect();
    let url = format!(
        "https://image.pollinations.ai/prompt/{}?width={}&height={}&nologo=true&enhance=true",
        encoded, width, height
    );
    // HEAD probe to confirm Pollinations is reachable and the URL resolves
    // to an image. The frontend will hit the same URL to actually render.
    let resp = http
        .head(&url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("Pollinations unreachable: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Pollinations returned HTTP {} — service may be overloaded. Try again in a moment or switch to a local SD provider.",
            resp.status()
        ));
    }
    Ok(url)
}

/// Local ComfyUI / Automatic1111 / SD.Next HTTP API. Assumes Automatic1111-
/// compatible `/sdapi/v1/txt2img` endpoint (ComfyUI has a compatibility
/// mode, SD.Next is compatible out of the box).
async fn call_local_sd(
    http: &reqwest::Client,
    base_url: &str,
    model: Option<&str>,
    prompt: &str,
    size: &str,
) -> Result<String, String> {
    let (width, height) = match size {
        "1024x1536" => (1024u32, 1536u32),
        "1536x1024" => (1536, 1024),
        _ => (1024, 1024),
    };
    let mut body = json!({
        "prompt": prompt,
        "width": width,
        "height": height,
        "steps": 20,
        "sampler_name": "Euler a",
        "cfg_scale": 7.0,
        "batch_size": 1,
    });
    if let Some(m) = model {
        body["override_settings"] = json!({"sd_model_checkpoint": m});
    }
    let url = format!("{}/sdapi/v1/txt2img", base_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| format!("Local SD unreachable at {}: {}", base_url, e))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Local SD returned HTTP {} — is ComfyUI/Automatic1111 running? Body: {}",
            0,
            body.chars().take(200).collect::<String>()
        ));
    }
    let data: Value = resp.json().await
        .map_err(|e| format!("Local SD response parse: {}", e))?;
    // A1111 returns {"images": ["<base64 png>"]}
    let b64 = data
        .get("images")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Local SD response missing 'images'".to_string())?;
    Ok(format!("data:image/png;base64,{}", b64))
}

/// OpenRouter paid path — kept for users who opt-in. Not used by default
/// (the free Pollinations path is the default for a reason).
async fn call_openrouter_chat(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
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
        .map_err(|e| format!("OpenRouter paid image call failed: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("OpenRouter {} — {}", status, &body[..body.len().min(200)]));
    }
    let data: Value = resp.json().await
        .map_err(|e| format!("OpenRouter paid response parse: {}", e))?;
    if let Some(url) = data
        .pointer("/choices/0/message/images/0/image_url/url")
        .and_then(|v| v.as_str())
    {
        return Ok(url.to_string());
    }
    Err("OpenRouter paid returned no image".to_string())
}
