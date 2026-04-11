use log::{info, warn};
use serde_json::json;
use std::time::Duration;

/// SMS receiving via Google Voice web interface (voice.google.com)
/// Uses headless Chromium to read messages — no API keys needed.
///
/// Required env vars:
///   GOOGLE_VOICE_NUMBER  — the GV phone number (for entering on verification forms)
///   GOOGLE_VOICE_EMAIL   — Google account email
///   GOOGLE_VOICE_PASSWORD — Google account password

const GV_MESSAGES_URL: &str = "https://voice.google.com/u/0/messages";

/// Return the Google Voice phone number for entering on verification forms
pub async fn sms_get_number() -> Result<String, String> {
    let number = std::env::var("GOOGLE_VOICE_NUMBER")
        .map_err(|_| "GOOGLE_VOICE_NUMBER env var not set. Sign up at voice.google.com first.")?;
    info!("[sms] Returning GV number: {}", number);
    Ok(number)
}

/// Read recent SMS messages from Google Voice
pub async fn sms_read(count: usize) -> Result<String, String> {
    let count = count.min(10);
    info!("[sms] Reading {} messages from Google Voice", count);

    ensure_gv_authenticated().await?;

    // Navigate to messages
    super::browser::browser_open("", GV_MESSAGES_URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Try structured extraction first, fall back to raw text
    let js = format!(
        r#"(() => {{
            // Google Voice renders conversations as list items
            const selectors = [
                '[data-item-id]',
                'gv-conversation-item',
                'md-list-item',
                '[role="listitem"]',
                '.gvMessagingView-conversationListItem',
                'a[href*="/messages/"]'
            ];
            for (const sel of selectors) {{
                const rows = document.querySelectorAll(sel);
                if (rows.length > 0) {{
                    const msgs = [];
                    for (const row of [...rows].slice(0, {count})) {{
                        const text = row.textContent.replace(/\s+/g, ' ').trim();
                        if (text.length > 3) msgs.push(text);
                    }}
                    if (msgs.length > 0) return msgs.join('\n---\n');
                }}
            }}
            // Fallback: return all visible text
            return document.body.innerText.substring(0, 4000);
        }})()"#
    );

    let result = super::browser::browser_execute_js("", &js).await?;
    if result.is_empty() || result == "(no value)" {
        // Final fallback
        super::browser::browser_read("").await
    } else {
        Ok(result)
    }
}

/// Wait for a verification code SMS on Google Voice (polls every 5s)
pub async fn sms_wait_for_code(timeout_secs: u64) -> Result<String, String> {
    let timeout = Duration::from_secs(timeout_secs.max(10).min(120));
    let start = std::time::Instant::now();

    info!("[sms] Waiting for verification code on Google Voice (timeout: {}s)", timeout_secs);

    ensure_gv_authenticated().await?;

    // Snapshot the current page text to detect new messages
    super::browser::browser_open("", GV_MESSAGES_URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let initial_text = page_text().await;
    let initial_codes = extract_all_codes(&initial_text);

    loop {
        if start.elapsed() > timeout {
            return Err("Timeout waiting for SMS verification code on Google Voice".into());
        }

        tokio::time::sleep(Duration::from_secs(5)).await;

        // Refresh the messages page
        super::browser::browser_execute_js("", "location.reload()").await?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let current_text = page_text().await;
        let current_codes = extract_all_codes(&current_text);

        // Find codes that weren't in the initial snapshot
        for code in &current_codes {
            if !initial_codes.contains(code) {
                info!("[sms] Found new verification code: {}", code);
                return Ok(code.clone());
            }
        }
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Ensure the browser is authenticated to Google Voice
async fn ensure_gv_authenticated() -> Result<(), String> {
    super::browser::browser_open("", GV_MESSAGES_URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check current URL — Google redirects to accounts.google.com if not logged in
    let url = super::browser::browser_execute_js(
        "",
        "location.href",
    )
    .await
    .unwrap_or_default();

    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    let lower = brief.to_lowercase();

    if url.contains("accounts.google.com") || lower.contains("sign in") || lower.contains("identifier") {
        info!("[sms] Not authenticated to Google Voice — attempting login");
        login_google().await
    } else {
        Ok(())
    }
}

/// Authenticate to Google for Voice access
async fn login_google() -> Result<(), String> {
    let email = std::env::var("GOOGLE_VOICE_EMAIL")
        .map_err(|_| "GOOGLE_VOICE_EMAIL env var not set")?;
    let password = std::env::var("GOOGLE_VOICE_PASSWORD")
        .map_err(|_| "GOOGLE_VOICE_PASSWORD env var not set")?;

    info!("[sms] Logging into Google as {}", email);

    // Step 1: Enter email on the identifier page
    super::browser::browser_fill("", "input[type=email]", &email).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Click Next — Google uses various button structures
    super::browser::browser_execute_js(
        "",
        r#"(() => {
            const next = document.querySelector('#identifierNext');
            if (next) { next.click(); return 'clicked #identifierNext'; }
            for (const b of document.querySelectorAll('button, [role=button]')) {
                if (b.textContent.trim() === 'Next') { b.click(); return 'clicked Next'; }
            }
            return 'no next button found';
        })()"#,
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Step 2: Enter password
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    let lower = brief.to_lowercase();

    if lower.contains("password") || lower.contains("welcome") || lower.contains("enter your password") {
        super::browser::browser_fill("", "input[type=password]", &password).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;

        super::browser::browser_execute_js(
            "",
            r#"(() => {
                const next = document.querySelector('#passwordNext');
                if (next) { next.click(); return 'clicked #passwordNext'; }
                for (const b of document.querySelectorAll('button, [role=button]')) {
                    if (b.textContent.trim() === 'Next') { b.click(); return 'clicked Next'; }
                }
                return 'no next button found';
            })()"#,
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Step 3: Handle post-login prompts
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    let lower = brief.to_lowercase();

    // "Stay signed in?" / "Save password?" / "Keep your account updated" prompts
    if lower.contains("stay signed in")
        || lower.contains("save password")
        || lower.contains("not now")
        || lower.contains("keep your account")
    {
        // Dismiss by clicking any continue/yes/no button
        super::browser::browser_execute_js(
            "",
            r#"(() => {
                for (const b of document.querySelectorAll('button, [role=button]')) {
                    const t = b.textContent.trim().toLowerCase();
                    if (t === 'yes' || t === 'continue' || t === 'not now' || t === 'no thanks') {
                        b.click(); return 'dismissed: ' + t;
                    }
                }
                return 'no dismiss button';
            })()"#,
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Step 4: Check if login succeeded or hit a blocker
    let url = super::browser::browser_execute_js("", "location.href")
        .await
        .unwrap_or_default();
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    let lower = brief.to_lowercase();

    if lower.contains("2-step")
        || lower.contains("verify it")
        || lower.contains("unusual")
        || lower.contains("captcha")
        || lower.contains("confirm your identity")
    {
        warn!("[sms] Google login blocked by 2FA/CAPTCHA");
        return Err(format!(
            "Google login needs manual verification (2FA or CAPTCHA). \
             Ask Sean to open the browser and log into voice.google.com once. \
             Current page: {}",
            brief
        ));
    }

    if url.contains("accounts.google.com") {
        // Still on login page — something went wrong
        return Err(format!(
            "Google login failed — still on sign-in page. \
             Google may be blocking automated login from this IP. \
             Ask Sean to log in manually once. Page: {}",
            brief
        ));
    }

    // Navigate to GV messages to confirm access
    super::browser::browser_open("", GV_MESSAGES_URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    if brief.to_lowercase().contains("voice") || brief.to_lowercase().contains("message") {
        info!("[sms] Google Voice login successful");
        Ok(())
    } else {
        warn!("[sms] Google Voice login uncertain — page: {}", brief);
        // Don't fail — might still be usable
        Ok(())
    }
}

/// Get page text for code extraction
async fn page_text() -> String {
    super::browser::browser_execute_js("", "document.body.innerText.substring(0, 5000)")
        .await
        .unwrap_or_default()
}

/// Extract all 4-8 digit codes from text
fn extract_all_codes(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    if let Ok(re) = regex::Regex::new(r"\b(\d{4,8})\b") {
        for cap in re.captures_iter(text) {
            codes.push(cap[1].to_string());
        }
    }
    codes
}
