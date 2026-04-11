use log::{info, warn, error};
use serde_json::json;
use std::time::Duration;

/// High-level account creation tool
/// Handles the entire signup flow including CAPTCHA solving

/// Create an Outlook/Hotmail email account — with step-by-step verification
pub async fn create_outlook_account(
    email: &str,
    password: &str,
    first_name: &str,
    last_name: &str,
    birth_month: &str,
    birth_day: &str,
    birth_year: &str,
    api_key: &str,
) -> Result<String, String> {
    info!("[account] Creating Outlook account: {}", email);
    let debug_path = std::path::Path::new("/tmp");

    // Helper: log page state
    async fn log_step(step: &str) {
        let title = eval_js("document.title").await.unwrap_or_default();
        let url = eval_js("location.href").await.unwrap_or_default();
        let inputs = eval_js("document.querySelectorAll('input').length + ' inputs, ' + document.querySelectorAll('button').length + ' buttons'").await.unwrap_or_default();
        info!("[account] {} — title='{}' url='{}' {}", step, title, &url[..url.len().min(80)], inputs);
    }

    // Step 1: Open signup page
    info!("[account] Step 1: Opening signup page");
    super::browser::browser_open("", "https://signup.live.com/signup").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    log_step("After open").await;

    // Step 2: Enter email
    info!("[account] Step 2: Entering email");
    let fill_result = super::browser::browser_fill("", "input", email).await;
    info!("[account] Fill result: {:?}", fill_result);
    let click_result = click_button_by_text("Next").await;
    info!("[account] Click Next result: {:?}", click_result);
    tokio::time::sleep(Duration::from_secs(3)).await;
    log_step("After email+Next").await;

    // Check if email is taken or error
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.contains("already") && page.contains("Microsoft account") {
        return Err(format!("Email {} is already registered", email));
    }
    if page.to_lowercase().contains("try a different") || page.to_lowercase().contains("not available") {
        return Err(format!("Email {} is not available", email));
    }

    // Step 3: Enter password
    info!("[account] Step 3: Entering password");
    // Make sure we're on the password page
    if !page.to_lowercase().contains("password") {
        warn!("[account] Expected password page, got: {}", page.chars().take(100).collect::<String>());
        let _ = super::browser::browser_screenshot("", debug_path).await;
        return Err(format!("Not on password page after email. Page: {}", page.chars().take(200).collect::<String>()));
    }
    super::browser::browser_fill("", "input", password).await?;
    click_button_by_text("Next").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    log_step("After password+Next").await;

    // Steps 4-5: Handle name and birthday pages using form discovery
    // Microsoft's order varies — discover what's on each page by its INPUT structure, not text
    let mut last_title = String::new();
    let mut same_page_count = 0;
    for form_step in 0..6 {
        // Detect if we're stuck on the same page
        let current_title = eval_js("document.title").await.unwrap_or_default();
        if current_title == last_title {
            same_page_count += 1;
            if same_page_count >= 2 {
                warn!("[account] Stuck on same page '{}' for {} iterations", current_title, same_page_count);
                let _ = super::browser::browser_screenshot("", debug_path).await;
                break; // Move to post-step handling
            }
        } else {
            same_page_count = 0;
        }
        last_title = current_title;

        let form_info = eval_js(concat!(
            "JSON.stringify({",
            "  title: document.title,",
            "  textInputs: Array.from(document.querySelectorAll('input[type=text],input:not([type])')).filter(i=>i.offsetParent!==null&&i.type!=='hidden').length,",
            "  numberInputs: Array.from(document.querySelectorAll('input[type=number]')).filter(i=>i.offsetParent!==null).length,",
            "  passwordInputs: Array.from(document.querySelectorAll('input[type=password]')).filter(i=>i.offsetParent!==null).length,",
            "  combos: document.querySelectorAll('[role=combobox]').length,",
            "  selects: document.querySelectorAll('select').length",
            "})"
        )).await.unwrap_or_default();
        info!("[account] Form step {}: {}", form_step, form_info);

        let info: serde_json::Value = serde_json::from_str(&form_info).unwrap_or_default();
        let text_inputs = info.get("textInputs").and_then(|v| v.as_u64()).unwrap_or(0);
        let number_inputs = info.get("numberInputs").and_then(|v| v.as_u64()).unwrap_or(0);
        let combos = info.get("combos").and_then(|v| v.as_u64()).unwrap_or(0);
        let title = info.get("title").and_then(|v| v.as_str()).unwrap_or("");

        // NAME PAGE: 2+ text inputs, no number inputs, no comboboxes
        if text_inputs >= 2 && number_inputs == 0 && combos == 0 {
            info!("[account] Detected NAME page ({}+ text inputs)", text_inputs);
            let fill_js = format!(
                concat!(
                    "(() => {{ ",
                    "const inputs = document.querySelectorAll('input[type=text], input:not([type])'); ",
                    "const visible = Array.from(inputs).filter(i => i.offsetParent !== null && i.type !== 'hidden'); ",
                    "const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; ",
                    "if (visible.length >= 2) {{ ",
                    "  s.call(visible[0], '{}'); visible[0].dispatchEvent(new Event('input', {{bubbles:true}})); ",
                    "  s.call(visible[1], '{}'); visible[1].dispatchEvent(new Event('input', {{bubbles:true}})); ",
                    "  return 'OK: ' + visible[0].value + ' ' + visible[1].value; ",
                    "}} ",
                    "return 'NO INPUTS (' + visible.length + ')'; ",
                    "}})()"
                ),
                first_name, last_name
            );
            let result = eval_js(&fill_js).await?;
            info!("[account] Name fill: {}", result);
            click_button_by_text("Next").await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // BIRTHDAY PAGE: has comboboxes (month/day dropdowns) or number input (year)
        if combos >= 2 || number_inputs >= 1 || title.to_lowercase().contains("detail") || title.to_lowercase().contains("birth") {
            info!("[account] Detected BIRTHDAY page (combos={}, number_inputs={})", combos, number_inputs);

            // Month — try native <select> first (Microsoft uses native selects), then ARIA dropdown
            let month_result = super::browser::browser_select("", "BirthMonth", birth_month).await
                .or(super::browser::browser_select("", "select[id*=irth]", birth_month).await)
                .or(super::browser::browser_set_dropdown("", "Month", birth_month).await);
            info!("[account] Month: {:?}", month_result);

            // Day — same approach
            let day_result = super::browser::browser_select("", "BirthDay", birth_day).await
                .or(super::browser::browser_select("", "select[id*=ay]", birth_day).await)
                .or(super::browser::browser_set_dropdown("", "Day", birth_day).await);
            info!("[account] Day: {:?}", day_result);

            // Year — number input first, then dropdown
            let year_js = format!(
                "(() => {{ const inp = document.querySelector('input[type=number]'); if (inp) {{ const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; s.call(inp, '{}'); inp.dispatchEvent(new Event('input', {{bubbles:true}})); inp.dispatchEvent(new Event('change', {{bubbles:true}})); return 'OK: ' + inp.value; }} return 'NO_INPUT'; }})()",
                birth_year
            );
            let year_result = eval_js(&year_js).await.unwrap_or_default();
            info!("[account] Year: {}", year_result);
            if year_result.contains("NO_INPUT") {
                let _ = super::browser::browser_set_dropdown("", "Year", birth_year).await;
            }

            click_button_by_text("Next").await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // Neither name nor birthday — we're past these steps
        info!("[account] Form step {}: not name or birthday page (title='{}'). Moving to post-step.", form_step, title);
        break;
    }

    // Step 6: Handle whatever page we're on now — could be CAPTCHA, terms, puzzle, etc.
    // Loop up to 5 times to handle multi-step post-signup flows
    for post_step in 0..5 {
        let page = super::browser::browser_read("").await.unwrap_or_default();
        let title = eval_js("document.title").await.unwrap_or_default();
        let _ = super::browser::browser_screenshot("", debug_path).await;
        info!("[account] Post-step {}: title='{}' content='{}'", post_step, title, page.chars().take(150).collect::<String>());

        let lower = page.to_lowercase();

        // CAPTCHA
        if lower.contains("human") || lower.contains("press and hold") || lower.contains("prove") || lower.contains("captcha") {
            info!("[account] CAPTCHA detected — invoking solver");
            match super::captcha::solve_captcha(api_key).await {
                Ok(msg) => info!("[account] CAPTCHA result: {}", msg),
                Err(e) => {
                    warn!("[account] CAPTCHA solver failed: {}", e);
                    return Err(format!("CAPTCHA solver failed: {}. Take screenshot for debug.", e));
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // "Add your name" page (Microsoft shows this AFTER birthday)
        if title.to_lowercase().contains("add your name") || (lower.contains("first name") && lower.contains("last name")) {
            info!("[account] Name page detected — filling name: {} {}", first_name, last_name);
            let fill_js = format!(
                concat!(
                    "(() => {{ ",
                    "const inputs = document.querySelectorAll('input[type=text], input:not([type])'); ",
                    "const visible = Array.from(inputs).filter(i => i.offsetParent !== null && i.type !== 'hidden'); ",
                    "const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; ",
                    "if (visible.length >= 2) {{ ",
                    "  s.call(visible[0], '{}'); visible[0].dispatchEvent(new Event('input', {{bubbles:true}})); ",
                    "  s.call(visible[1], '{}'); visible[1].dispatchEvent(new Event('input', {{bubbles:true}})); ",
                    "  return 'OK: ' + visible[0].value + ' ' + visible[1].value; ",
                    "}} ",
                    "return 'NO INPUTS (' + visible.length + ')'; ",
                    "}})()"
                ),
                first_name, last_name
            );
            let result = eval_js(&fill_js).await.unwrap_or_default();
            info!("[account] Name fill: {}", result);
            let _ = click_button_by_text("Next").await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // Terms/Privacy — check title, not just page content (footer always has "Privacy" link)
        if title.to_lowercase().contains("terms") || title.to_lowercase().contains("agree")
            || (lower.contains("agree") && lower.contains("accept") && !lower.contains("add your name")) {
            info!("[account] Accepting terms/privacy");
            let _ = click_button_by_text("Accept").await;
            let _ = click_button_by_text("I agree").await;
            let _ = click_button_by_text("Yes").await;
            let _ = click_button_by_text("Next").await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // "Stay signed in?" prompt
        if lower.contains("stay signed in") {
            info!("[account] Stay signed in prompt — clicking Yes");
            let _ = click_button_by_text("Yes").await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        // Success indicators
        if lower.contains("welcome") || lower.contains("inbox") || lower.contains("account has been created")
            || title.to_lowercase().contains("mail") || title.to_lowercase().contains("outlook") {
            info!("[account] Success page detected!");
            break;
        }

        // Error
        if lower.contains("error") || lower.contains("sorry") || lower.contains("could not create") {
            return Err(format!("Account creation failed. Title: {} Page: {}", title, page.chars().take(300).collect::<String>()));
        }

        // Unknown page — take screenshot and try clicking Next
        info!("[account] Unknown page state, trying Next...");
        let _ = click_button_by_text("Next").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Step 7: VERIFY the account actually exists by trying to login
    info!("[account] Step 7: Verifying account exists via login");
    super::browser::browser_open("", "https://login.live.com").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    super::browser::browser_fill("", "input", email).await?;
    click_button_by_text("Next").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let page = super::browser::browser_read("").await.unwrap_or_default();
    let _ = super::browser::browser_screenshot("", debug_path).await;

    if page.to_lowercase().contains("password") || page.to_lowercase().contains("enter the password") {
        info!("[account] VERIFIED: Account {} exists (password prompt shown)", email);
        return Ok(format!("Account created and VERIFIED: {}\nPassword: {}\nName: {} {}", email, password, first_name, last_name));
    }

    if page.to_lowercase().contains("couldn't find") || page.to_lowercase().contains("not found") {
        warn!("[account] VERIFICATION FAILED: Account {} does NOT exist", email);
        return Err(format!("Account creation appeared to succeed but VERIFICATION FAILED — {} does not exist. Check /tmp/screenshot-*.png for debug.", email));
    }

    Err(format!("Account verification unclear. Page: {}", page.chars().take(300).collect::<String>()))
}

/// Create a Facebook account — handles form, dropdowns, submit, email verification
pub async fn create_facebook_account(
    email: &str,
    password: &str,
    first_name: &str,
    last_name: &str,
    birth_month: &str,
    birth_day: &str,
    birth_year: &str,
    email_password: &str,
    api_key: &str,
) -> Result<String, String> {
    info!("[account] Creating Facebook account for {} {} ({})", first_name, last_name, email);

    // Step 1: Open Facebook signup
    info!("[account:fb] Step 1: Opening signup page");
    super::browser::browser_open("", "https://www.facebook.com/r.php").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Step 2: Fill form fields
    info!("[account:fb] Step 2: Filling form fields");
    let fields = json!({
        "first name": first_name,
        "last name": last_name,
        "email": email,
        "password": password
    });
    super::browser::browser_fill_form("", &fields).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Step 3: Set birthday dropdowns
    info!("[account:fb] Step 3: Setting birthday");
    let _ = super::browser::browser_set_dropdown("", "Month", birth_month).await;
    let _ = super::browser::browser_set_dropdown("", "Day", birth_day).await;
    let _ = super::browser::browser_set_dropdown("", "Year", birth_year).await;

    // Step 4: Set gender
    info!("[account:fb] Step 4: Setting gender");
    let _ = super::browser::browser_set_dropdown("", "Select your gender", "Male").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Step 5: Find and click submit
    info!("[account:fb] Step 5: Submitting form");
    let submit_js = concat!(
        "(() => { window.scrollTo(0,9999); ",
        "const all = document.querySelectorAll('button, div[role=button], input[type=submit]'); ",
        "for (const e of all) { ",
        "  const t = e.textContent.trim().toLowerCase(); ",
        "  if (t.includes('sign up') || t.includes('submit') || e.name==='websubmit') { ",
        "    e.scrollIntoView({block:'center'}); ",
        "    const r = e.getBoundingClientRect(); ",
        "    return 'BTN:' + Math.round(r.x + r.width/2) + ',' + Math.round(r.y + r.height/2); ",
        "  } ",
        "} return 'NOT FOUND'; })()"
    );
    let result = eval_js(submit_js).await?;
    if result.starts_with("BTN:") {
        let coords: Vec<f64> = result.trim_start_matches("BTN:")
            .split(',').filter_map(|s| s.parse().ok()).collect();
        if coords.len() == 2 {
            super::browser::browser_click_at("", coords[0], coords[1]).await?;
        }
    } else {
        // Fallback: try clicking any submit-like button
        if click_button_by_text("Sign Up").await.is_err() {
            click_button_by_text("Submit").await?;
        }
    }

    tokio::time::sleep(Duration::from_secs(5)).await;

    // Step 6: Check result
    info!("[account:fb] Step 6: Checking result");
    let page = super::browser::browser_read("").await.unwrap_or_default();
    let _ = super::browser::browser_screenshot("", std::path::Path::new("/tmp")).await;

    if page.to_lowercase().contains("confirmation code") || page.to_lowercase().contains("confirm your") {
        info!("[account:fb] Facebook wants confirmation code — checking email");

        // Wait a bit for email delivery
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Try to read the code from Outlook via browser
        let code = read_outlook_email_code(email, email_password).await;
        match code {
            Ok(code) => {
                info!("[account:fb] Got confirmation code: {}", code);
                // Navigate back to Facebook and enter the code
                // The page might still be on the confirmation screen
                super::browser::browser_open("", "https://www.facebook.com").await?;
                tokio::time::sleep(Duration::from_secs(3)).await;

                let page = super::browser::browser_read("").await.unwrap_or_default();
                if page.to_lowercase().contains("confirmation") || page.to_lowercase().contains("code") {
                    super::browser::browser_fill("", "input", &code).await?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if click_button_by_text("Continue").await.is_err() {
                        let _ = click_button_by_text("Confirm").await;
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }

                return Ok(format!("Facebook account created!\nEmail: {}\nPassword: {}\nName: {} {}\nConfirmation code: {}",
                    email, password, first_name, last_name, code));
            }
            Err(e) => {
                warn!("[account:fb] Could not get confirmation code: {}", e);
                return Ok(format!(
                    "Facebook account created but needs email confirmation.\nEmail: {}\nPassword: {}\nName: {} {}\nCheck {} for the 5-digit confirmation code and enter it on Facebook.",
                    email, password, first_name, last_name, email
                ));
            }
        }
    }

    if page.to_lowercase().contains("error") {
        return Err(format!("Facebook signup error. Page: {}", page.chars().take(300).collect::<String>()));
    }

    Ok(format!("Facebook account status: {}", page.chars().take(300).collect::<String>()))
}

/// Read a confirmation code from Outlook inbox via browser
async fn read_outlook_email_code(email: &str, password: &str) -> Result<String, String> {
    info!("[account:fb] Reading Outlook inbox for confirmation code");

    // Open Outlook login
    super::browser::browser_open("", "https://outlook.live.com/mail").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check if we need to log in
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("sign in") || page.to_lowercase().contains("email") {
        // Enter email
        super::browser::browser_fill("", "input", email).await?;
        click_button_by_text("Next").await?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Enter password
        super::browser::browser_fill("", "input[type=password]", password).await?;
        if click_button_by_text("Sign in").await.is_err() {
            click_button_by_text("Next").await?;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Handle "Stay signed in?" prompt
        let _ = click_button_by_text("Yes").await;
        let _ = click_button_by_text("No").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Read inbox content
    let page = super::browser::browser_read("").await.unwrap_or_default();

    // Look for a 5-digit code in the page text
    let code = extract_confirmation_code(&page);
    if let Some(code) = code {
        return Ok(code);
    }

    // Click on the Facebook email if visible
    let click_js = concat!(
        "(() => { const all = document.querySelectorAll('*'); ",
        "for (const el of all) { ",
        "  if (el.textContent.includes('Facebook') && el.textContent.includes('FB-') && el.offsetParent !== null) { ",
        "    el.click(); return 'CLICKED'; ",
        "  } ",
        "} return 'NO FB EMAIL'; })()"
    );
    let _ = eval_js(click_js).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let page = super::browser::browser_read("").await.unwrap_or_default();
    let code = extract_confirmation_code(&page);
    if let Some(code) = code {
        return Ok(code);
    }

    Err(format!("No confirmation code found in inbox. Page: {}", page.chars().take(200).collect::<String>()))
}

fn extract_confirmation_code(text: &str) -> Option<String> {
    // Look for FB- followed by 5 digits, or just 5 consecutive digits near "code" or "confirm"
    let re_fb = regex::Regex::new(r"FB-(\d{5})").ok()?;
    if let Some(cap) = re_fb.captures(text) {
        return Some(cap[1].to_string());
    }

    // Look for standalone 5-digit numbers near confirmation context
    let re_5digit = regex::Regex::new(r"\b(\d{5})\b").ok()?;
    let lower = text.to_lowercase();
    if lower.contains("code") || lower.contains("confirm") || lower.contains("verify") {
        if let Some(cap) = re_5digit.captures(text) {
            return Some(cap[1].to_string());
        }
    }

    None
}

/// Create an Instagram account via browser
pub async fn create_instagram_account(
    email: &str,
    password: &str,
    full_name: &str,
    username: &str,
    api_key: &str,
) -> Result<String, String> {
    info!("[account:ig] Creating Instagram account: {} ({})", username, email);
    let debug_path = std::path::Path::new("/tmp");

    // Step 1: Open Instagram signup
    info!("[account:ig] Step 1: Opening signup page");
    super::browser::browser_open("", "https://www.instagram.com/accounts/emailsignup/").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Step 2: Fill form
    info!("[account:ig] Step 2: Filling form");
    let fields = json!({
        "email": email,
        "full name": full_name,
        "username": username,
        "password": password
    });
    super::browser::browser_fill_form("", &fields).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Click Sign up / Next
    click_button_by_text("Sign up").await
        .or(click_button_by_text("Next").await)?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Step 3: Handle birthday if shown
    info!("[account:ig] Step 3: Checking for birthday");
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("birthday") || page.to_lowercase().contains("birth") {
        let _ = super::browser::browser_set_dropdown("", "Month", "April").await;
        let _ = super::browser::browser_set_dropdown("", "Day", "1").await;
        let _ = super::browser::browser_set_dropdown("", "Year", "1985").await;
        let _ = click_button_by_text("Next").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Step 4: Handle CAPTCHA or verification
    info!("[account:ig] Step 4: Checking for CAPTCHA/verification");
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("human") || page.to_lowercase().contains("captcha")
        || page.to_lowercase().contains("prove") || page.to_lowercase().contains("security") {
        match super::captcha::solve_captcha(api_key).await {
            Ok(msg) => info!("[account:ig] CAPTCHA: {}", msg),
            Err(e) => warn!("[account:ig] CAPTCHA failed: {}", e),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Step 5: Handle confirmation code
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("confirmation") || page.to_lowercase().contains("code") {
        info!("[account:ig] Confirmation code needed — checking email");
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Try to read code from email
        let code = extract_email_code(email).await;
        if let Some(code) = code {
            info!("[account:ig] Got code: {}", code);
            super::browser::browser_fill("", "input", &code).await?;
            let _ = click_button_by_text("Next").await;
            let _ = click_button_by_text("Confirm").await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    let _ = super::browser::browser_screenshot("", debug_path).await;
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    info!("[account:ig] Final state: {}", brief);

    // Check for success indicators
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("phone") || page.to_lowercase().contains("mobile number") {
        return Ok(format!(
            "Instagram account partially created — needs phone verification.\nUsername: {}\nEmail: {}\nPassword: {}\nPhone verification required to complete.",
            username, email, password
        ));
    }

    Ok(format!(
        "Instagram account creation attempted.\nUsername: {}\nEmail: {}\nPassword: {}\nFinal page: {}",
        username, email, password, brief
    ))
}

/// Helper: try to read a verification code from email
async fn extract_email_code(email: &str) -> Option<String> {
    // Determine which account to read from
    let account = if email.contains("outlook") || email.contains("hotmail") {
        "felix"
    } else if email.contains("gmail") {
        "crimson-lantern"
    } else {
        return None;
    };

    match super::email::email_read_account("INBOX", 5, account).await {
        Ok(emails) => {
            // Look for 5-6 digit codes
            let re = regex::Regex::new(r"\b(\d{5,6})\b").ok()?;
            let lower = emails.to_lowercase();
            if lower.contains("code") || lower.contains("confirm") || lower.contains("verify") {
                if let Some(cap) = re.captures(&emails) {
                    return Some(cap[1].to_string());
                }
            }
            None
        }
        Err(_) => None,
    }
}

/// Navigate Meta OAuth flow to get access tokens for Threads/Instagram API
pub async fn meta_oauth_flow(
    app_id: &str,
    app_secret: &str,
    redirect_uri: &str,
    scopes: &str,
    email: &str,
    password: &str,
) -> Result<String, String> {
    info!("[account:meta] Starting OAuth flow for app {}", app_id);

    // Step 1: Navigate to authorization URL
    let auth_url = format!(
        "https://www.threads.net/oauth/authorize?client_id={}&redirect_uri={}&scope={}&response_type=code",
        app_id,
        urlencoding(redirect_uri),
        urlencoding(scopes)
    );
    info!("[account:meta] Opening auth URL");
    super::browser::browser_open("", &auth_url).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Step 2: Login if needed
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("log in") || page.to_lowercase().contains("sign in") || page.to_lowercase().contains("email") {
        info!("[account:meta] Login required");
        let fields = json!({"email": email, "password": password});
        super::browser::browser_fill_form("", &fields).await?;
        let _ = click_button_by_text("Log in").await;
        let _ = click_button_by_text("Log In").await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Step 3: Authorize app if prompted
    let page = super::browser::browser_read("").await.unwrap_or_default();
    if page.to_lowercase().contains("authorize") || page.to_lowercase().contains("allow") || page.to_lowercase().contains("continue") {
        info!("[account:meta] Authorizing app");
        let _ = click_button_by_text("Allow").await;
        let _ = click_button_by_text("Authorize").await;
        let _ = click_button_by_text("Continue").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Step 4: Capture redirect URL with authorization code
    let url = eval_js("location.href").await.unwrap_or_default();
    info!("[account:meta] Redirect URL: {}", url);

    // Extract code from URL
    let code = url.split("code=").nth(1)
        .and_then(|s| s.split('&').next())
        .map(|s| s.to_string());

    let code = match code {
        Some(c) => c,
        None => return Err(format!("No authorization code in redirect URL: {}", url)),
    };

    info!("[account:meta] Got authorization code: {}...", &code[..code.len().min(20)]);

    // Step 5: Exchange code for short-lived token
    let client = reqwest::Client::new();
    let token_resp = client.post("https://graph.threads.net/oauth/access_token")
        .form(&[
            ("client_id", app_id),
            ("client_secret", app_secret),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
            ("code", &code),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Token exchange error: {}", e))?;

    let token_body: serde_json::Value = token_resp.json().await
        .map_err(|e| format!("Token parse error: {}", e))?;

    let short_token = token_body.get("access_token").and_then(|v| v.as_str())
        .ok_or("No access_token in response")?;
    let user_id = token_body.get("user_id").and_then(|v| v.as_u64())
        .map(|v| v.to_string())
        .unwrap_or_default();

    info!("[account:meta] Got short-lived token, user_id={}", user_id);

    // Step 6: Exchange for long-lived token (60 days)
    let ll_resp = client.get("https://graph.threads.net/access_token")
        .query(&[
            ("grant_type", "th_exchange_token"),
            ("client_secret", app_secret),
            ("access_token", short_token),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Long-lived token error: {}", e))?;

    let ll_body: serde_json::Value = ll_resp.json().await
        .map_err(|e| format!("Long-lived token parse error: {}", e))?;

    let long_token = ll_body.get("access_token").and_then(|v| v.as_str())
        .unwrap_or(short_token);
    let expires_in = ll_body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(0);

    Ok(format!(
        "OAuth complete!\naccess_token={}\nuser_id={}\nexpires_in={}s (~{}days)",
        long_token, user_id, expires_in, expires_in / 86400
    ))
}

/// Refresh a Meta long-lived token before it expires
pub async fn meta_refresh_token(
    app_secret: &str,
    current_token: &str,
) -> Result<String, String> {
    info!("[account:meta] Refreshing token");
    let client = reqwest::Client::new();

    let resp = client.get("https://graph.threads.net/refresh_access_token")
        .query(&[
            ("grant_type", "th_refresh_token"),
            ("access_token", current_token),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Refresh error: {}", e))?;

    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("Refresh parse error: {}", e))?;

    let new_token = body.get("access_token").and_then(|v| v.as_str())
        .ok_or("No access_token in refresh response")?;
    let expires_in = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(0);

    Ok(format!("access_token={}\nexpires_in={}s (~{}days)", new_token, expires_in, expires_in / 86400))
}

/// Post to Threads API
pub async fn threads_post(
    access_token: &str,
    user_id: &str,
    text: &str,
    url: Option<&str>,
) -> Result<String, String> {
    info!("[social] Posting to Threads: {}...", &text[..text.len().min(50)]);
    let client = reqwest::Client::new();

    // Step 1: Create media container
    let mut params = vec![
        ("media_type", "TEXT"),
        ("text", text),
        ("access_token", access_token),
    ];
    if let Some(link) = url {
        params.push(("link_attachment", link));
    }

    let create_resp = client.post(format!("https://graph.threads.net/v1.0/{}/threads", user_id))
        .form(&params)
        .timeout(Duration::from_secs(30))
        .send().await
        .map_err(|e| format!("Create error: {}", e))?;

    let create_body: serde_json::Value = create_resp.json().await
        .map_err(|e| format!("Create parse error: {}", e))?;

    let creation_id = create_body.get("id").and_then(|v| v.as_str())
        .ok_or_else(|| format!("No creation ID: {:?}", create_body))?;

    // Step 2: Publish
    let pub_resp = client.post(format!("https://graph.threads.net/v1.0/{}/threads_publish", user_id))
        .form(&[
            ("creation_id", creation_id),
            ("access_token", access_token),
        ])
        .timeout(Duration::from_secs(30))
        .send().await
        .map_err(|e| format!("Publish error: {}", e))?;

    let pub_body: serde_json::Value = pub_resp.json().await
        .map_err(|e| format!("Publish parse error: {}", e))?;

    let post_id = pub_body.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
    info!("[social] Threads post published: {}", post_id);

    Ok(format!("Posted to Threads: {}", post_id))
}

/// Refresh YouTube OAuth tokens via REST API — no browser needed.
/// This is the primary path: uses the existing refresh_token to get a new
/// access_token (and sometimes a new refresh_token). Fast, reliable, no 2FA.
/// Only fails if the refresh_token itself has expired (7-day limit for
/// unapproved restricted scopes).
pub async fn youtube_token_refresh(workspace: &std::path::Path) -> Result<String, String> {
    info!("[yt-refresh] Refreshing YouTube token via REST API");

    let client_path = workspace.join("client_secret.json");
    let token_path = workspace.join("youtube_token.json");

    // Load client credentials
    let client_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&client_path)
            .map_err(|e| format!("Cannot read client_secret.json: {}", e))?
    ).map_err(|e| format!("Invalid client_secret.json: {}", e))?;

    let client = client_json.get("web")
        .or_else(|| client_json.get("installed"))
        .ok_or("client_secret.json has no 'web' or 'installed' key")?;

    let client_id = client.get("client_id").and_then(|v| v.as_str())
        .ok_or("No client_id")?;
    let client_secret = client.get("client_secret").and_then(|v| v.as_str())
        .ok_or("No client_secret")?;

    // Load current token
    let token_data: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&token_path)
            .map_err(|e| format!("Cannot read youtube_token.json: {}", e))?
    ).map_err(|e| format!("Invalid youtube_token.json: {}", e))?;

    let refresh_token = token_data.get("refresh_token").and_then(|v| v.as_str())
        .ok_or("No refresh_token in youtube_token.json")?;

    // Call Google token endpoint
    let http = reqwest::Client::new();
    let resp = http.post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .timeout(Duration::from_secs(30))
        .send().await
        .map_err(|e| format!("Token refresh request failed: {}", e))?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("Token response parse failed: {}", e))?;

    if !status.is_success() {
        let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        let desc = body.get("error_description").and_then(|v| v.as_str()).unwrap_or("");

        if error == "invalid_grant" {
            return Err(format!(
                "Refresh token expired or revoked ({}). Use youtube_reauth to do a full browser OAuth flow.",
                desc
            ));
        }
        return Err(format!("Token refresh failed: {} — {}", error, desc));
    }

    let access_token = body.get("access_token").and_then(|v| v.as_str())
        .ok_or("No access_token in refresh response")?;
    let expires_in = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let new_refresh = body.get("refresh_token").and_then(|v| v.as_str());
    let refresh_expires_in = body.get("refresh_token_expires_in").and_then(|v| v.as_u64());

    // Update token file
    let mut updated = token_data.clone();
    updated["token"] = json!(access_token);
    updated["expiry"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
            + expires_in as f64
    );
    if let Some(rt) = new_refresh {
        updated["refresh_token"] = json!(rt);
        info!("[yt-refresh] New refresh token obtained");
    }

    std::fs::write(&token_path, serde_json::to_string_pretty(&updated).unwrap())
        .map_err(|e| format!("Failed to save token: {}", e))?;

    // Verify the token works
    let verify = http.get("https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true")
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(Duration::from_secs(15))
        .send().await;

    let channel_name = match verify {
        Ok(resp) if resp.status().is_success() => {
            let vbody: serde_json::Value = resp.json().await.unwrap_or(json!({}));
            vbody.get("items").and_then(|i| i.get(0))
                .and_then(|i| i.get("snippet"))
                .and_then(|s| s.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string()
        }
        _ => "verification failed".to_string(),
    };

    let refresh_days = refresh_expires_in.map(|s| s / 86400).unwrap_or(0);
    let msg = format!(
        "YouTube token refreshed via API.\nChannel: {}\nAccess token: valid {}min\nRefresh token: {}, ~{}d remaining",
        channel_name,
        expires_in / 60,
        if new_refresh.is_some() { "renewed" } else { "unchanged" },
        refresh_days,
    );
    info!("[yt-refresh] {}", msg);
    Ok(msg)
}

/// Re-authorize YouTube OAuth — runs the full browser consent flow,
/// captures the redirect on localhost:19876, exchanges for new tokens,
/// and saves to youtube_token.json in the workspace.
pub async fn youtube_reauth(workspace: &std::path::Path) -> Result<String, String> {
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    info!("[yt-reauth] Starting YouTube OAuth re-authorization");

    let client_path = workspace.join("client_secret.json");
    let token_path = workspace.join("youtube_token.json");

    // Load client credentials
    let client_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&client_path)
            .map_err(|e| format!("Cannot read client_secret.json: {}", e))?
    ).map_err(|e| format!("Invalid client_secret.json: {}", e))?;

    // Support both "web" and "installed" client types
    let client = client_json.get("web")
        .or_else(|| client_json.get("installed"))
        .ok_or("client_secret.json has no 'web' or 'installed' key")?;

    let client_id = client.get("client_id").and_then(|v| v.as_str())
        .ok_or("No client_id in client_secret.json")?;
    let client_secret = client.get("client_secret").and_then(|v| v.as_str())
        .ok_or("No client_secret in client_secret.json")?;

    let redirect_port = 19876u16;
    let redirect_uri = format!("http://localhost:{}", redirect_port);
    let scope = "https://www.googleapis.com/auth/youtube.force-ssl";

    // Build OAuth URL
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        urlencoding(client_id),
        urlencoding(&redirect_uri),
        urlencoding(scope),
    );

    // Start TCP listener to capture the redirect
    let listener = TcpListener::bind(format!("127.0.0.1:{}", redirect_port)).await
        .map_err(|e| format!("Cannot bind to port {}: {}", redirect_port, e))?;
    info!("[yt-reauth] Listening on 127.0.0.1:{}", redirect_port);

    // Navigate browser to OAuth consent
    info!("[yt-reauth] Opening OAuth consent in browser");
    super::browser::browser_open("", &auth_url).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check if Google needs login (auto-approve if session exists)
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    let lower = brief.to_lowercase();

    if lower.contains("choose an account") || lower.contains("select an account") {
        // Click the account (first account in list)
        info!("[yt-reauth] Selecting account");
        let _ = eval_js(
            "(() => { const a = document.querySelector('[data-identifier], [data-email]'); if (a) { a.click(); return 'clicked'; } return 'no account'; })()"
        ).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    if lower.contains("sign in") || lower.contains("identifier") {
        // Need to login — use GV credentials
        let gv_email = std::env::var("GOOGLE_VOICE_EMAIL")
            .map_err(|_| "GOOGLE_VOICE_EMAIL not set — cannot login to Google")?;
        let gv_password = std::env::var("GOOGLE_VOICE_PASSWORD")
            .map_err(|_| "GOOGLE_VOICE_PASSWORD not set")?;

        info!("[yt-reauth] Logging into Google as {}", gv_email);
        super::browser::browser_fill("", "input[type=email]", &gv_email).await?;
        let _ = eval_js("document.querySelector('#identifierNext')?.click() || 'no btn'").await;
        tokio::time::sleep(Duration::from_secs(4)).await;

        super::browser::browser_fill("", "input[type=password]", &gv_password).await?;
        let _ = eval_js("document.querySelector('#passwordNext')?.click() || 'no btn'").await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Check for consent screen — click Allow/Continue
    let brief = super::browser::browser_read_brief("").await.unwrap_or_default();
    if brief.to_lowercase().contains("allow") || brief.to_lowercase().contains("continue")
        || brief.to_lowercase().contains("grant") || brief.to_lowercase().contains("access")
    {
        info!("[yt-reauth] Approving consent");
        let _ = click_button_by_text("Continue").await;
        let _ = click_button_by_text("Allow").await;
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Sometimes there's a second "Allow" for specific permissions
        let _ = click_button_by_text("Continue").await;
        let _ = click_button_by_text("Allow").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Wait for the redirect (timeout 60s)
    info!("[yt-reauth] Waiting for OAuth redirect on port {}...", redirect_port);
    let code = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);

                // Parse the GET request for the code parameter
                if let Some(path) = request.lines().next() {
                    if let Some(query_start) = path.find('?') {
                        let query = &path[query_start + 1..path.rfind(' ').unwrap_or(path.len())];
                        for param in query.split('&') {
                            if let Some(val) = param.strip_prefix("code=") {
                                // Send HTTP response
                                let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<h1>YouTube authorization successful!</h1><p>You can close this tab.</p>";
                                let _ = stream.write_all(resp.as_bytes()).await;
                                return Ok::<String, String>(val.to_string());
                            }
                        }
                        // Check for error
                        for param in query.split('&') {
                            if let Some(val) = param.strip_prefix("error=") {
                                let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<h1>Authorization failed</h1>";
                                let _ = stream.write_all(resp.as_bytes()).await;
                                return Err(format!("OAuth error: {}", val));
                            }
                        }
                    }
                }
                // Not the right request, send 404 and keep listening
                let resp = "HTTP/1.1 404 Not Found\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    }).await.map_err(|_| {
        "Timeout waiting for OAuth redirect (60s). Google may need manual consent — check the browser.".to_string()
    })??;

    info!("[yt-reauth] Got authorization code: {}...", &code[..code.len().min(20)]);

    // Exchange code for tokens
    let http = reqwest::Client::new();
    let token_resp = http.post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .timeout(Duration::from_secs(30))
        .send().await
        .map_err(|e| format!("Token exchange failed: {}", e))?;

    let token_body: serde_json::Value = token_resp.json().await
        .map_err(|e| format!("Token parse failed: {}", e))?;

    let access_token = token_body.get("access_token").and_then(|v| v.as_str())
        .ok_or_else(|| format!("No access_token in response: {:?}", token_body))?;
    let refresh_token = token_body.get("refresh_token").and_then(|v| v.as_str());
    let expires_in = token_body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let refresh_expires = token_body.get("refresh_token_expires_in").and_then(|v| v.as_u64());

    info!("[yt-reauth] Got access token, expires_in={}s", expires_in);
    if let Some(rt) = refresh_token {
        info!("[yt-reauth] New refresh token obtained");
    }

    // Update token file
    let mut token_data: serde_json::Value = if token_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&token_path).unwrap_or_default())
            .unwrap_or(json!({}))
    } else {
        json!({})
    };

    token_data["token"] = json!(access_token);
    token_data["expiry"] = json!(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() + expires_in as f64);
    if let Some(rt) = refresh_token {
        token_data["refresh_token"] = json!(rt);
    }

    std::fs::write(&token_path, serde_json::to_string_pretty(&token_data).unwrap())
        .map_err(|e| format!("Failed to save token: {}", e))?;

    // Verify token works
    let verify = http.get("https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true")
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(Duration::from_secs(15))
        .send().await;

    let channel_name = match verify {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or(json!({}));
            body.get("items").and_then(|i| i.get(0))
                .and_then(|i| i.get("snippet"))
                .and_then(|s| s.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string()
        }
        _ => "verification failed".to_string(),
    };

    let refresh_days = refresh_expires.map(|s| s / 86400).unwrap_or(0);
    let msg = format!(
        "YouTube OAuth re-auth successful!\nChannel: {}\nRefresh token: {}\nRefresh valid: ~{}d",
        channel_name,
        if refresh_token.is_some() { "renewed" } else { "unchanged" },
        refresh_days,
    );
    info!("[yt-reauth] {}", msg);
    Ok(msg)
}

/// Simple URL encoding
fn urlencoding(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
        .replace('?', "%3F").replace('=', "%3D")
        .replace('&', "%26").replace(' ', "+")
        .replace(',', "%2C")
}

async fn click_button_by_text(text: &str) -> Result<String, String> {
    let js = format!(
        "(() => {{ for (const b of document.querySelectorAll('button, [role=button], input[type=submit]')) {{ if (b.textContent.trim() === '{}' || b.value === '{}') {{ b.click(); return 'OK'; }} }} return 'NOT FOUND'; }})()",
        text, text
    );
    let result = eval_js(&js).await?;
    if result.contains("NOT FOUND") {
        // Try partial match
        let js2 = format!(
            "(() => {{ for (const b of document.querySelectorAll('button, [role=button]')) {{ if (b.textContent.trim().toLowerCase().includes('{}')) {{ b.click(); return 'OK'; }} }} return 'FAIL'; }})()",
            text.to_lowercase()
        );
        eval_js(&js2).await
    } else {
        Ok(result)
    }
}

async fn eval_js(js: &str) -> Result<String, String> {
    let result = super::browser::cdp_command_pub(
        "Runtime.evaluate",
        json!({"expression": js})
    ).await?;

    let val = result.get("result").and_then(|r| r.get("result")).and_then(|r| r.get("value"))
        .and_then(|v| v.as_str()).unwrap_or("(no value)");
    Ok(val.to_string())
}
