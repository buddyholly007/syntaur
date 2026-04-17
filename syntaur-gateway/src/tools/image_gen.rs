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
        let conv_id = None::<String>; // Will be set by the handler if available
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

/// Call OpenRouter's image generation API (Seedream 4.5 or similar).
async fn call_image_api(
    http: &reqwest::Client,
    api_key: &str,
    prompt: &str,
    size: &str,
) -> Result<String, String> {
    let (width, height) = match size {
        "1024x1536" => (1024, 1536),
        "1536x1024" => (1536, 1024),
        _ => (1024, 1024),
    };

    let body = json!({
        "model": "bytedance/seedream-4.5",
        "prompt": prompt,
        "n": 1,
        "size": format!("{}x{}", width, height),
    });

    let resp = http
        .post("https://openrouter.ai/api/v1/images/generations")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
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

    // Extract image URL from OpenAI-compatible response
    data.get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url").or_else(|| img.get("b64_json")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No image URL in response".to_string())
}
