use log::{info, warn};
use serde_json::json;
use std::time::Duration;

/// Vision-based CAPTCHA solver
/// Strategy:
/// 1. Try to find and click the accessible challenge icon first (easier than press-and-hold)
/// 2. Handle whatever alternative challenge appears
/// 3. Fall back to press-and-hold only if no accessible option exists

const VISION_MODELS: &[&str] = &[
    "google/gemini-2.0-flash-001",
    "google/gemini-2.5-flash-preview",
    "meta-llama/llama-4-scout:free",
];
const VISION_API: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Analyze a screenshot using a vision model
async fn vision_analyze(
    client: &reqwest::Client,
    api_key: &str,
    image_base64: &str,
    prompt: &str,
    model: Option<&str>,
) -> Result<String, String> {
    let model = model.unwrap_or(VISION_MODELS[0]);
    let request = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": {
                    "url": format!("data:image/png;base64,{}", image_base64)
                }}
            ]
        }],
        "max_tokens": 500,
        "temperature": 0.1
    });

    let resp = client.post(VISION_API)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .json(&request)
        .send().await
        .map_err(|e| format!("Vision API error: {}", e))?;

    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("Vision API parse error: {}", e))?;

    let text = body.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("(no response)");

    Ok(text.to_string())
}

/// Try vision analysis with model fallback
async fn vision_ask(
    client: &reqwest::Client,
    api_key: &str,
    image_base64: &str,
    prompt: &str,
) -> Result<String, String> {
    for model in VISION_MODELS {
        match vision_analyze(client, api_key, image_base64, prompt, Some(model)).await {
            Ok(result) if !result.is_empty() && result != "(no response)" => return Ok(result),
            Ok(_) => { warn!("[captcha] Empty from {}", model); continue; }
            Err(e) => { warn!("[captcha] {} failed: {}", model, e); continue; }
        }
    }
    Err("All vision models failed".to_string())
}

/// Take a screenshot via CDP and return base64 PNG
async fn capture_screenshot() -> Result<String, String> {
    let result = super::browser::cdp_command_pub("Page.captureScreenshot", json!({"format": "png"})).await?;
    result.get("result").and_then(|r| r.get("data"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or("No screenshot data".to_string())
}

/// Main CAPTCHA solver
pub async fn solve_captcha(api_key: &str) -> Result<String, String> {
    info!("[captcha] Starting CAPTCHA solver");
    let client = reqwest::Client::new();
    let mut tried_accessible = false;
    let mut last_action = String::new();
    let mut same_action_count = 0;

    for attempt in 0..15 {
        info!("[captcha] Attempt {}/15", attempt + 1);

        let screenshot = capture_screenshot().await?;

        // Detect stuck loop — if we keep getting the same action, something's wrong
        let analysis = vision_ask(&client, api_key, &screenshot, concat!(
            "Look at this screenshot. What do you see? Respond with ONE of these words ONLY:\n",
            "- PRESS_AND_HOLD — if you see a 'Press and hold' button\n",
            "- PUZZLE — if you see an image puzzle, rotation puzzle, or tile selection challenge\n",
            "- TEXT_CAPTCHA — if you see a text input CAPTCHA or audio CAPTCHA\n",
            "- SOLVED — if the page has no CAPTCHA (normal page content)\n",
            "- ERROR — if the page shows an error\n",
            "\nRespond with ONLY the single keyword. No letters, no explanation, just the keyword."
        )).await?;

        info!("[captcha] Analysis: {}", analysis.trim());
        let upper = analysis.trim().to_uppercase();

        // Check for solved
        if upper.contains("SOLVED") || upper.contains("NORMAL PAGE") {
            info!("[captcha] CAPTCHA solved!");
            return Ok("CAPTCHA solved".to_string());
        }

        if upper.contains("ERROR") {
            warn!("[captcha] Error page detected");
            return Err("CAPTCHA page shows an error".to_string());
        }

        // Detect stuck loop
        if upper == last_action {
            same_action_count += 1;
            if same_action_count >= 3 {
                warn!("[captcha] Stuck in loop (same response {} times), trying different approach", same_action_count);
                // Try clicking somewhere else or refreshing
                let _ = super::browser::browser_execute_js("", "location.reload()").await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                same_action_count = 0;
                continue;
            }
        } else {
            same_action_count = 0;
        }
        last_action = upper.clone();

        // Strategy: Handle press-and-hold CAPTCHA (PerimeterX / Arkose)
        if upper.contains("PRESS_AND_HOLD") {
            // Find the px-captcha element on the MAIN page (PerimeterX renders the button there)
            let btn_js = "(() => { var el = document.querySelector('#px-captcha'); if (!el) el = document.querySelector('[id*=captcha][tabindex]'); if (!el) el = document.querySelector('button:has(> span)'); if (!el) { var all = document.querySelectorAll('div, button, span'); for (var i = 0; i < all.length; i++) { if (all[i].textContent.trim() === 'Press and hold') { el = all[i]; break; } } } if (!el) return 'NO_BUTTON'; var r = el.getBoundingClientRect(); return 'BTN:' + Math.round(r.x + r.width/2) + ',' + Math.round(r.y + r.height/2) + ':' + Math.round(r.width) + 'x' + Math.round(r.height); })()";
            let btn_result = super::browser::cdp_command_pub("Runtime.evaluate", json!({"expression": btn_js})).await
                .ok().and_then(|r| super::browser::extract_js_value_pub(&r).ok())
                .unwrap_or_default();
            info!("[captcha] Button search: {}", btn_result);

            let hold_secs = 4 + (attempt as u64 % 5);

            if btn_result.starts_with("BTN:") {
                // Got exact coordinates from DOM
                if let Some(coords) = parse_coords(&btn_result, "BTN:") {
                    info!("[captcha] Hold at DOM coords ({}, {}) for {}s", coords.0, coords.1, hold_secs);
                    super::browser::browser_hold_at("", coords.0, coords.1, hold_secs).await?;
                }
            } else {
                // Fallback to vision model
                let hold_analysis = vision_ask(&client, api_key, &screenshot, concat!(
                    "Find the 'Press and hold' button in this screenshot. ",
                    "Respond with HOLD_AT:<x>,<y> for the EXACT CENTER of the button. Just the command."
                )).await?;
                if let Some(coords) = parse_coords(&hold_analysis.trim().to_uppercase(), "HOLD_AT:") {
                    info!("[captcha] Hold at vision coords ({}, {}) for {}s", coords.0, coords.1, hold_secs);
                    super::browser::browser_hold_at("", coords.0, coords.1, hold_secs).await?;
                }
            }

            tokio::time::sleep(Duration::from_secs(3)).await;

            // Check post-hold state
            let post_hold_ss = capture_screenshot().await?;
            let post_hold = vision_ask(&client, api_key, &post_hold_ss, concat!(
                "What do you see now? Respond with ONE keyword:\n",
                "- SOLVED — CAPTCHA is gone, normal page\n",
                "- PUZZLE — a new image puzzle or challenge appeared\n",
                "- PRESS_AND_HOLD — still the same press-and-hold button\n",
                "Only the keyword."
            )).await.unwrap_or_default();
            let post_upper = post_hold.trim().to_uppercase();
            info!("[captcha] Post-hold state: {}", post_upper);

            if post_upper.contains("SOLVED") {
                return Ok("CAPTCHA solved".to_string());
            }

            if post_upper.contains("PUZZLE") && !tried_accessible {
                info!("[captcha] Puzzle appeared, looking for accessibility option...");
                let acc_analysis = vision_ask(&client, api_key, &post_hold_ss, concat!(
                    "Find the accessibility icon in this CAPTCHA dialog. ",
                    "It's a small circle with a person symbol, usually near the bottom.\n",
                    "If found: CLICK_AT:<x>,<y>\n",
                    "If not found: NOT_FOUND\n",
                    "Only the command."
                )).await.unwrap_or_default();

                if let Some(coords) = parse_coords(&acc_analysis.trim().to_uppercase(), "CLICK_AT:") {
                    info!("[captcha] Clicking accessibility at ({}, {})", coords.0, coords.1);
                    super::browser::browser_click_at("", coords.0, coords.1).await?;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    tried_accessible = true;
                } else {
                    tried_accessible = true;
                }
            }

            continue;
        }

        if upper.contains("PUZZLE") || upper.contains("TEXT_CAPTCHA") {
            // For puzzle/text CAPTCHAs, get specific instructions from vision
            let puzzle_analysis = vision_ask(&client, api_key, &screenshot, concat!(
                "This is a CAPTCHA puzzle. Look carefully and tell me what action to take:\n",
                "- If there's a specific image to click, respond: CLICK_AT:<x>,<y>\n",
                "- If there's an arrow to rotate, respond: CLICK_AT:<x>,<y> for the arrow\n",
                "- If there's a text input, respond: TYPE:<text to enter>\n",
                "- If there's a 'Verify' or 'Submit' button to click, respond: CLICK_AT:<x>,<y>\n",
                "- If the puzzle is already solved, respond: SOLVED\n",
                "Respond with ONLY the command."
            )).await?;

            info!("[captcha] Puzzle action: {}", puzzle_analysis.trim());
            let puzzle_upper = puzzle_analysis.trim().to_uppercase();

            if puzzle_upper.starts_with("SOLVED") {
                return Ok("CAPTCHA solved".to_string());
            }

            if let Some(coords) = parse_coords(&puzzle_upper, "CLICK_AT:") {
                info!("[captcha] Puzzle click at ({}, {})", coords.0, coords.1);
                super::browser::browser_click_at("", coords.0, coords.1).await?;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }

            if puzzle_upper.contains("TYPE:") {
                if let Some(text) = puzzle_upper.strip_prefix("TYPE:").or(
                    puzzle_analysis.trim().to_uppercase().split("TYPE:").nth(1)
                ) {
                    let text = text.trim();
                    info!("[captcha] Typing: {}", text);
                    // Find input and type
                    let _ = super::browser::browser_fill("", "input", text).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    // Try to click submit/verify
                    let _ = super::browser::browser_execute_js("", concat!(
                        "(() => { for (const b of document.querySelectorAll('button, [role=button]')) {",
                        "  const t = b.textContent.trim().toLowerCase();",
                        "  if (t.includes('verify') || t.includes('submit') || t.includes('next')) {",
                        "    b.click(); return 'clicked'; }",
                        "} return 'no button'; })()"
                    )).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            }
        }

        // Unparseable response — take screenshot and continue
        warn!("[captcha] Unparseable: {}", analysis.trim());
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Err("CAPTCHA not solved after 15 attempts".to_string())
}

/// Try to solve Arkose FunCaptcha by interacting with the iframe directly via CDP
async fn solve_arkose_via_iframe() -> Result<String, String> {
    info!("[captcha] Attempting Arkose iframe solve via CDP");

    // Microsoft uses hsprotect.net as their Arkose proxy, check multiple patterns
    let patterns = ["hsprotect", "arkoselabs", "funcaptcha", "enforcement"];
    let mut pattern_found = None;
    for p in &patterns {
        if super::browser::eval_in_iframe(p, "'found'").await.is_ok() {
            pattern_found = Some(*p);
            break;
        }
    }

    let pattern = pattern_found.ok_or("No Arkose/CAPTCHA iframe found (tried hsprotect, arkoselabs, funcaptcha, enforcement)")?;
    info!("[captcha] Found CAPTCHA iframe matching '{}'", pattern);

    // Step 1: Explore what's inside the iframe — find elements we can interact with
    let explore_result = super::browser::eval_in_iframe(pattern,
        "(() => { var els = []; var all = document.querySelectorAll('canvas, button, [role=button], iframe, div[class]'); for (var i = 0; i < Math.min(all.length, 20); i++) { var e = all[i]; var r = e.getBoundingClientRect(); els.push(e.tagName + '.' + (e.className||'').substring(0,40) + ' ' + Math.round(r.x) + ',' + Math.round(r.y) + ' ' + Math.round(r.width) + 'x' + Math.round(r.height)); } return 'ELEMENTS(' + all.length + '): ' + els.join(' | '); })()"
    ).await;
    info!("[captcha] Iframe contents: {:?}", explore_result);

    // Step 2: Try to dispatch pointer events on the canvas/button inside the iframe
    let click_result = super::browser::eval_in_iframe(pattern,
        "(() => { var canvas = document.querySelector('canvas'); var btn = document.querySelector('button, [role=button], #challenge-stage button'); var holdBtn = document.querySelector('[class*=hold], [class*=press], [class*=Hold], [class*=Press]'); var target = canvas || btn || holdBtn; if (!target) return 'NO_TARGET: tags=' + document.body.children.length + ' html=' + document.body.innerHTML.substring(0, 500); var rect = target.getBoundingClientRect(); var x = rect.x + rect.width/2; var y = rect.y + rect.height/2; target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, clientX: x, clientY: y, pointerId: 1, pointerType: 'mouse', isPrimary: true, pressure: 0.5 })); target.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, clientX: x, clientY: y, button: 0 })); return 'HOLDING:' + Math.round(x) + ',' + Math.round(y) + ':' + target.tagName + ':' + (target.className || '').substring(0,50); })()"
    ).await;

    match click_result {
        Ok(ref val) if val.starts_with("HOLDING:") => {
            info!("[captcha] Got pointer down inside iframe: {}", val);

            // Hold for 5 seconds by sending pointer moves, then release
            tokio::time::sleep(Duration::from_secs(5)).await;

            // Release
            let _ = super::browser::eval_in_iframe(pattern,
                "(() => { var target = document.querySelector('canvas') || document.querySelector('button, [role=button]') || document.body; var rect = target.getBoundingClientRect(); var x = rect.x + rect.width/2; var y = rect.y + rect.height/2; target.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, clientX: x, clientY: y, pointerId: 1, pointerType: 'mouse', isPrimary: true })); target.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, clientX: x, clientY: y, button: 0 })); return 'RELEASED'; })()"
            ).await;

            tokio::time::sleep(Duration::from_secs(2)).await;
            return Ok("Dispatched pointer events inside iframe".to_string());
        }
        Ok(ref val) => {
            info!("[captcha] Iframe eval result: {}", val);
            return Err(format!("No holdable target in iframe: {}", val));
        }
        Err(e) => {
            return Err(format!("Iframe eval failed: {}", e));
        }
    }
}

fn parse_coords(text: &str, prefix: &str) -> Option<(f64, f64)> {
    if let Some(start) = text.find(prefix) {
        let rest = &text[start + prefix.len()..];
        let coord_str: String = rest.chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.' || *c == ' ')
            .collect();
        let parts: Vec<f64> = coord_str.split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if parts.len() >= 2 {
            return Some((parts[0], parts[1]));
        }
    }
    None
}
