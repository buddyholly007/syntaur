//! Tax module — receipt scanning, expense tracking, tax dashboard.
//! Premium add-on module ($49).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct Receipt {
    pub id: i64,
    pub vendor: Option<String>,
    pub amount_cents: Option<i64>,
    pub amount_display: Option<String>,
    pub category: Option<String>,
    pub receipt_date: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub image_url: String,
    pub created_at: i64,
}

#[derive(Serialize)]
pub struct Expense {
    pub id: i64,
    pub amount_cents: i64,
    pub amount_display: String,
    pub vendor: String,
    pub category: Option<String>,
    pub expense_date: String,
    pub description: Option<String>,
    pub entity: String,
    pub receipt_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Serialize)]
pub struct CategorySummary {
    pub category: String,
    pub entity: String,
    pub total_cents: i64,
    pub total_display: String,
    pub count: i64,
    pub tax_deductible: bool,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn cents_to_display(cents: i64) -> String {
    let negative = cents < 0;
    let abs = cents.unsigned_abs();
    let dollars = abs / 100;
    let c = abs % 100;
    if negative {
        format!("-${}.{:02}", dollars, c)
    } else {
        format!("${}.{:02}", dollars, c)
    }
}

pub fn parse_cents(s: &str) -> Option<i64> {
    let cleaned = s.replace(['$', ',', ' '], "");
    let parts: Vec<&str> = cleaned.split('.').collect();
    match parts.len() {
        1 => parts[0].parse::<i64>().ok().map(|d| d * 100),
        2 => {
            let dollars = parts[0].parse::<i64>().ok()?;
            let mut cents_str = parts[1].to_string();
            if cents_str.len() == 1 { cents_str.push('0'); }
            if cents_str.len() > 2 { cents_str.truncate(2); }
            let cents = cents_str.parse::<i64>().ok()?;
            Some(dollars * 100 + if dollars < 0 { -cents } else { cents })
        }
        _ => None,
    }
}

fn receipts_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dir = PathBuf::from(format!("{}/.syntaur/receipts", home));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// ── Receipt Upload ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReceiptUploadQuery {
    pub token: String,
}

pub async fn handle_receipt_upload(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ReceiptUploadQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &params.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No image data".to_string()));
    }
    if body.len() > 10 * 1024 * 1024 {
        return Err((StatusCode::BAD_REQUEST, "Image too large (max 10MB)".to_string()));
    }

    let content_type = headers.get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg");
    let ext = if content_type.contains("png") { "png" }
        else if content_type.contains("pdf") { "pdf" }
        else { "jpg" };

    // Save image
    let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
    let path = receipts_dir().join(&filename);
    std::fs::write(&path, &body).map_err(|e|
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save receipt: {}", e))
    )?;

    // Insert receipt record
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let path_str = path.to_string_lossy().to_string();
    let fname = filename.clone();

    let receipt_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO receipts (user_id, image_path, status, created_at) VALUES (?, ?, 'pending', ?)",
            rusqlite::params![uid, &path_str, now],
        ).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!("[tax] Receipt #{} uploaded: {}", receipt_id, filename);

    // Kick off async vision scan
    let state2 = Arc::clone(&state);
    let rid = receipt_id;
    tokio::spawn(async move {
        if let Err(e) = scan_receipt_vision(&state2, rid).await {
            error!("[tax] Vision scan failed for receipt #{}: {}", rid, e);
        }
    });

    Ok(Json(serde_json::json!({
        "id": receipt_id,
        "status": "pending",
        "image_url": format!("/api/tax/receipts/{}/image", receipt_id),
        "message": "Receipt uploaded. Scanning with AI..."
    })))
}

/// Vision scan a receipt — extract vendor, amount, date, category via LLM
async fn scan_receipt_vision(state: &AppState, receipt_id: i64) -> Result<(), String> {
    let db = state.db_path.clone();

    // Load the image
    let image_path: String = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT image_path FROM receipts WHERE id = ?",
                rusqlite::params![receipt_id],
                |r| r.get(0),
            ).map_err(|e| e.to_string())
        }).await.map_err(|e| e.to_string())??
    };

    let image_bytes = std::fs::read(&image_path).map_err(|e| e.to_string())?;
    use base64::Engine;
    let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    let ext = image_path.rsplit('.').next().unwrap_or("jpg");
    let mime = match ext {
        "png" => "image/png",
        "pdf" => "application/pdf",
        _ => "image/jpeg",
    };

    // Call LLM with vision
    let client = &state.client;
    let config = &state.config;

    // Find a provider that supports vision. Use OpenRouter with a vision model.
    // The user's configured model may not support images, so we override to a
    // known vision-capable model on whatever cloud provider is available.
    let provider = config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| config.models.providers.iter().next());

    let (provider_name, provider_config) = provider.ok_or("No LLM provider configured")?;

    // Force a vision-capable model regardless of what's configured
    let model = if provider_config.base_url.contains("openrouter") {
        "google/gemini-2.0-flash-001" // Free, vision-capable on OpenRouter
    } else if provider_config.base_url.contains("anthropic") {
        "claude-sonnet-4-6"
    } else if provider_config.base_url.contains("openai") {
        "gpt-4o-mini"
    } else {
        // Local models rarely support vision — try anyway with configured model
        provider_config.models.first()
            .map(|m| m.id.as_str())
            .or_else(|| provider_config.extra.get("model").and_then(|v| v.as_str()))
            .unwrap_or("gpt-4o-mini")
    };

    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    let prompt = r#"Analyze this receipt image. Extract:
1. Vendor/Store name
2. Total amount (in dollars, e.g. "45.99")
3. Date (YYYY-MM-DD format)
4. Category (one of: Advertising & Marketing, Equipment & Tools, Hardware & Supplies, Lumber & Raw Materials, Office Supplies, Professional Services, Rent & Utilities, Insurance, Software & Subscriptions, Shipping & Packaging, Vehicle & Mileage, Education & Training, Meals & Entertainment, Travel, Tools - Consumables, Safety Gear, Medical, Mortgage, Vehicle, Donations, Education, Home Improvement, Utilities, Groceries, Dining, Entertainment, Other)
5. Brief description of items

Respond ONLY with JSON: {"vendor":"...","amount":"...","date":"...","category":"...","description":"..."}"#;

    let payload = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime, base64_image)}}
            ]
        }],
        "max_tokens": 500,
        "temperature": 0.1
    });

    let resp = client.post(&url)
        .header("Authorization", format!("Bearer {}", provider_config.api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("LLM request: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("LLM HTTP {}: {}", status, &body[..body.len().min(200)]));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");

    // Parse the JSON from the response
    let extracted: serde_json::Value = serde_json::from_str(
        content.trim().trim_start_matches("```json").trim_end_matches("```").trim()
    ).map_err(|e| format!("Parse vision result: {} — raw: {}", e, &content[..content.len().min(200)]))?;

    let vendor = extracted["vendor"].as_str().unwrap_or("").to_string();
    let amount_str = extracted["amount"].as_str().unwrap_or("0");
    let amount_cents = parse_cents(amount_str).unwrap_or(0);
    let date = extracted["date"].as_str().unwrap_or("").to_string();
    let category_name = extracted["category"].as_str().unwrap_or("Other").to_string();
    let description = extracted["description"].as_str().unwrap_or("").to_string();

    // Look up category ID
    let db2 = db.clone();
    let cat_name = category_name.clone();
    let category_id: Option<i64> = tokio::task::spawn_blocking(move || -> Option<i64> {
        let conn = rusqlite::Connection::open(&db2).ok()?;
        conn.query_row(
            "SELECT id FROM expense_categories WHERE name = ?",
            rusqlite::params![&cat_name],
            |r| r.get(0),
        ).ok()
    }).await.unwrap_or(None);

    // Update receipt
    let db3 = db.clone();
    let vendor_log = vendor.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db3).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE receipts SET vendor = ?, amount_cents = ?, category_id = ?, receipt_date = ?, description = ?, status = 'scanned' WHERE id = ?",
            rusqlite::params![&vendor, amount_cents, category_id, &date, &description, receipt_id],
        ).map_err(|e| e.to_string())?;

        // Auto-create expense from scanned receipt
        let uid: i64 = conn.query_row("SELECT user_id FROM receipts WHERE id = ?", rusqlite::params![receipt_id], |r| r.get(0))
            .unwrap_or(0);
        let entity = if category_id.map(|c| c <= 17).unwrap_or(false) { "business" } else { "personal" };
        conn.execute(
            "INSERT INTO expenses (user_id, amount_cents, vendor, category_id, expense_date, description, entity, receipt_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, amount_cents, &vendor, category_id, &date, &description, entity, receipt_id, chrono::Utc::now().timestamp()],
        ).map_err(|e| e.to_string())?;

        Ok(())
    }).await.map_err(|e| e.to_string())??;

    info!("[tax] Receipt #{} scanned: {} {}", receipt_id, vendor_log, cents_to_display(amount_cents));
    Ok(())
}

// ── Receipt List ────────────────────────────────────────────────────────────

pub async fn handle_receipt_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let receipts = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT r.id, r.vendor, r.amount_cents, r.receipt_date, r.description, r.status, r.created_at, c.name \
             FROM receipts r LEFT JOIN expense_categories c ON r.category_id = c.id \
             WHERE r.user_id = ? ORDER BY r.created_at DESC LIMIT 100"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![uid], |r| {
            let cents: Option<i64> = r.get(2)?;
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "vendor": r.get::<_, Option<String>>(1)?,
                "amount_cents": cents,
                "amount_display": cents.map(cents_to_display),
                "receipt_date": r.get::<_, Option<String>>(3)?,
                "description": r.get::<_, Option<String>>(4)?,
                "status": r.get::<_, String>(5)?,
                "created_at": r.get::<_, i64>(6)?,
                "category": r.get::<_, Option<String>>(7)?,
                "image_url": format!("/api/tax/receipts/{}/image", r.get::<_, i64>(0)?),
            }))
        }).map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "receipts": receipts })))
}

// ── Tax Document Upload (smart classifier) ──────────────────────────────────

pub async fn handle_tax_doc_upload(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ReceiptUploadQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &params.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    if body.is_empty() { return Err((StatusCode::BAD_REQUEST, "No file data".to_string())); }
    if body.len() > 10 * 1024 * 1024 { return Err((StatusCode::BAD_REQUEST, "File too large (max 10MB)".to_string())); }

    let content_type = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/pdf");
    let ext = if content_type.contains("png") { "png" } else if content_type.contains("pdf") { "pdf" } else { "jpg" };

    let filename = format!("taxdoc-{}.{}", uuid::Uuid::new_v4(), ext);
    let path = receipts_dir().join(&filename);
    std::fs::write(&path, &body).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Save failed: {}", e)))?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let path_str = path.to_string_lossy().to_string();

    let doc_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO tax_documents (user_id, doc_type, image_path, status, created_at) VALUES (?, 'unknown', ?, 'pending', ?)",
            rusqlite::params![uid, &path_str, now],
        ).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!("[tax] Document #{} uploaded: {}", doc_id, filename);

    let state2 = Arc::clone(&state);
    tokio::spawn(async move {
        if let Err(e) = classify_and_extract(&state2, doc_id).await {
            error!("[tax] Document classification failed for #{}: {}", doc_id, e);
        }
    });

    Ok(Json(serde_json::json!({
        "id": doc_id,
        "status": "pending",
        "message": "Document uploaded. Classifying and extracting data..."
    })))
}

/// Two-pass scan: classify document type, then extract type-specific fields
async fn classify_and_extract(state: &AppState, doc_id: i64) -> Result<(), String> {
    let db = state.db_path.clone();

    let image_path: String = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            conn.query_row("SELECT image_path FROM tax_documents WHERE id = ?", rusqlite::params![doc_id], |r| r.get(0))
                .map_err(|e| e.to_string())
        }).await.map_err(|e| e.to_string())??
    };

    let image_bytes = std::fs::read(&image_path).map_err(|e| e.to_string())?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
    let ext = image_path.rsplit('.').next().unwrap_or("pdf");
    let mime = match ext { "png" => "image/png", "pdf" => "application/pdf", _ => "image/jpeg" };

    let config = &state.config;
    let provider = config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| config.models.providers.iter().next());
    let (_, provider_config) = provider.ok_or("No LLM provider")?;
    let model = if provider_config.base_url.contains("openrouter") { "google/gemini-2.0-flash-001" }
        else if provider_config.base_url.contains("anthropic") { "claude-sonnet-4-6" }
        else { "gpt-4o-mini" };
    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    // Pass 1: Classify + extract in one call with a comprehensive prompt
    let prompt = r#"Analyze this tax document. First identify what type of document it is, then extract ALL relevant fields.

Document types: w2, 1099_int, 1099_div, 1099_b, 1099_misc, 1099_nec, 1095_c, property_tax_statement, mortgage_statement, bank_statement, credit_card_statement, receipt, invoice, insurance_policy, other

For W-2 forms, extract: employer_name, employer_ein, employee_name, employee_ssn_last4, box1_wages, box2_fed_withheld, box3_ss_wages, box4_ss_withheld, box5_medicare_wages, box6_medicare_withheld, box12_codes, box14_other, state, box16_state_wages, box17_state_withheld
For 1099-INT: payer, box1_interest, box4_fed_withheld
For 1099-DIV: payer, box1a_ordinary, box1b_qualified, box2a_capital_gains, box4_fed_withheld
For 1099-B: broker, total_proceeds, total_cost_basis, total_gain_loss
For 1099-MISC/NEC: payer, box1_nonemployee_comp, box4_fed_withheld
For 1095-C: employer, coverage_months
For mortgage statements: lender, box1_interest_paid, box2_outstanding_principal, box5_mortgage_insurance, property_address
For property tax: authority, amount, year, property_address
For receipts: vendor, amount, date, category, items

Respond ONLY with JSON: {"doc_type":"...","tax_year":2025,"issuer":"...","fields":{...all extracted fields...}}"#;

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime, b64)}}
        ]}],
        "max_tokens": 1500,
        "temperature": 0.1
    });

    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", provider_config.api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(60))
        .send().await.map_err(|e| format!("LLM: {}", e))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("LLM HTTP {}: {}", s, &body[..body.len().min(200)]));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let extracted: serde_json::Value = serde_json::from_str(
        content.trim().trim_start_matches("```json").trim_end_matches("```").trim()
    ).map_err(|e| format!("Parse: {} — raw: {}", e, &content[..content.len().min(200)]))?;

    let doc_type = extracted["doc_type"].as_str().unwrap_or("other").to_string();
    let tax_year = extracted["tax_year"].as_i64();
    let issuer = extracted["issuer"].as_str().unwrap_or("").to_string();
    let fields = extracted.get("fields").cloned().unwrap_or(serde_json::json!({}));
    let fields_str = serde_json::to_string(&fields).unwrap_or("{}".to_string());

    // Update document record
    let db2 = db.clone();
    let doc_type2 = doc_type.clone();
    let issuer2 = issuer.clone();
    let fields_str2 = fields_str.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE tax_documents SET doc_type = ?, tax_year = ?, issuer = ?, extracted_fields = ?, status = 'scanned' WHERE id = ?",
            rusqlite::params![&doc_type2, tax_year, &issuer2, &fields_str2, doc_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|e| e.to_string())??;

    // Auto-populate income data from W-2s
    if doc_type == "w2" {
        let db3 = db.clone();
        let fields2 = fields.clone();
        let issuer3 = issuer.clone();
        let year = tax_year.unwrap_or(2025);
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = rusqlite::Connection::open(&db3).map_err(|e| e.to_string())?;
            let uid: i64 = conn.query_row("SELECT user_id FROM tax_documents WHERE id = ?", rusqlite::params![doc_id], |r| r.get(0)).unwrap_or(0);
            let now = chrono::Utc::now().timestamp();

            // Extract W-2 specific fields
            let wages_str = fields2["box1_wages"].as_str().or(fields2["box1_wages"].as_f64().map(|_| "")).unwrap_or("0");
            let wages = parse_cents(wages_str).or_else(|| fields2["box1_wages"].as_f64().map(|f| (f * 100.0) as i64)).unwrap_or(0);
            let withheld_str = fields2["box2_fed_withheld"].as_str().or(fields2["box2_fed_withheld"].as_f64().map(|_| "")).unwrap_or("0");
            let withheld = parse_cents(withheld_str).or_else(|| fields2["box2_fed_withheld"].as_f64().map(|f| (f * 100.0) as i64)).unwrap_or(0);

            let employee = fields2["employee_name"].as_str().unwrap_or("Employee");

            // Upsert income: wages
            if wages > 0 {
                conn.execute(
                    "INSERT OR REPLACE INTO tax_income (user_id, source, amount_cents, tax_year, category, description, created_at) \
                     VALUES (?, 'W-2 Wages', ?, ?, 'Wages', ?, ?)",
                    rusqlite::params![uid, wages, year, format!("{} - {}", issuer3, employee), now],
                ).map_err(|e| e.to_string())?;
            }

            // Upsert income: federal withholding (stored as a separate record for the tax estimator)
            if withheld > 0 {
                conn.execute(
                    "INSERT OR REPLACE INTO tax_income (user_id, source, amount_cents, tax_year, category, description, created_at) \
                     VALUES (?, 'W-2 Withholding', ?, ?, 'Federal Withholding', ?, ?)",
                    rusqlite::params![uid, withheld, year, format!("{} - {} Box 2", issuer3, employee), now],
                ).map_err(|e| e.to_string())?;
            }

            Ok(())
        }).await.map_err(|e| e.to_string())??;
    }

    info!("[tax] Document #{} classified as {} from {} (year {:?})", doc_id, doc_type, issuer, tax_year);
    Ok(())
}

// ── Tax Document List ───────────────────────────────────────────────────────

pub async fn handle_tax_doc_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year_filter = params.get("year").and_then(|y| y.parse::<i64>().ok());

    let docs = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let (sql, p): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match year_filter {
            Some(y) => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? AND tax_year = ? ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(y)],
            ),
            None => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>],
            ),
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            let fields_str: Option<String> = r.get(4)?;
            let fields: serde_json::Value = fields_str.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::json!({}));
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "doc_type": r.get::<_, String>(1)?,
                "tax_year": r.get::<_, Option<i64>>(2)?,
                "issuer": r.get::<_, Option<String>>(3)?,
                "fields": fields,
                "status": r.get::<_, String>(5)?,
                "created_at": r.get::<_, i64>(6)?,
                "image_url": format!("/api/tax/documents/{}/image", r.get::<_, i64>(0)?),
            }))
        }).map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "documents": docs })))
}

pub async fn handle_tax_doc_image(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await?;
    let db = state.db_path.clone();
    let path: String = tokio::task::spawn_blocking(move || -> Result<String, ()> {
        let conn = rusqlite::Connection::open(&db).map_err(|_| ())?;
        conn.query_row("SELECT image_path FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)).map_err(|_| ())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.map_err(|_| StatusCode::NOT_FOUND)?;
    let data = std::fs::read(&path).map_err(|_| StatusCode::NOT_FOUND)?;
    let mut headers = axum::http::HeaderMap::new();
    let ct = if path.ends_with(".png") { "image/png" } else if path.ends_with(".pdf") { "application/pdf" } else { "image/jpeg" };
    headers.insert("content-type", ct.parse().unwrap());
    Ok((headers, data))
}

// ── Receipt Image ───────────────────────────────────────────────────────────

pub async fn handle_receipt_image(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await?;
    let db = state.db_path.clone();

    let path: String = tokio::task::spawn_blocking(move || -> Result<String, ()> {
        let conn = rusqlite::Connection::open(&db).map_err(|_| ())?;
        conn.query_row("SELECT image_path FROM receipts WHERE id = ?", rusqlite::params![id], |r| r.get(0))
            .map_err(|_| ())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let data = std::fs::read(&path).map_err(|_| StatusCode::NOT_FOUND)?;
    let mut headers = axum::http::HeaderMap::new();
    let ct = if path.ends_with(".png") { "image/png" }
        else if path.ends_with(".pdf") { "application/pdf" }
        else { "image/jpeg" };
    headers.insert("content-type", ct.parse().unwrap());
    headers.insert("cache-control", "private, max-age=3600".parse().unwrap());
    Ok((headers, data))
}

// ── Expense CRUD ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExpenseCreateRequest {
    pub token: String,
    pub amount: String,
    pub vendor: String,
    pub category: Option<String>,
    pub date: String,
    pub description: Option<String>,
    pub entity: Option<String>,
}

pub async fn handle_expense_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExpenseCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    let amount_cents = parse_cents(&req.amount)
        .ok_or((StatusCode::BAD_REQUEST, "Invalid amount format".to_string()))?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let vendor = req.vendor.clone();
    let date = req.date.clone();
    let desc = req.description.clone();
    let entity = req.entity.clone().unwrap_or_else(|| "personal".to_string());
    let cat = req.category.clone();
    let now = chrono::Utc::now().timestamp();

    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let category_id: Option<i64> = cat.as_deref().and_then(|c| {
            conn.query_row("SELECT id FROM expense_categories WHERE name = ?", rusqlite::params![c], |r| r.get(0)).ok()
        });
        conn.execute(
            "INSERT INTO expenses (user_id, amount_cents, vendor, category_id, expense_date, description, entity, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, amount_cents, &vendor, category_id, &date, &desc, &entity, now],
        ).map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "amount_display": cents_to_display(amount_cents),
        "vendor": req.vendor,
    })))
}

pub async fn handle_expense_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let entity_filter = params.get("entity").cloned();
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();

    let expenses = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut sql = "SELECT e.id, e.amount_cents, e.vendor, e.expense_date, e.description, e.entity, e.receipt_id, e.created_at, c.name \
                       FROM expenses e LEFT JOIN expense_categories c ON e.category_id = c.id \
                       WHERE e.user_id = ?".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(uid)];

        if let Some(ref ent) = entity_filter {
            sql.push_str(" AND e.entity = ?");
            params_vec.push(Box::new(ent.clone()));
        }
        if let Some(ref s) = start {
            sql.push_str(" AND e.expense_date >= ?");
            params_vec.push(Box::new(s.clone()));
        }
        if let Some(ref e) = end {
            sql.push_str(" AND e.expense_date <= ?");
            params_vec.push(Box::new(e.clone()));
        }
        sql.push_str(" ORDER BY e.expense_date DESC LIMIT 200");

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            let cents: i64 = r.get(1)?;
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "amount_cents": cents,
                "amount_display": cents_to_display(cents),
                "vendor": r.get::<_, String>(2)?,
                "expense_date": r.get::<_, String>(3)?,
                "description": r.get::<_, Option<String>>(4)?,
                "entity": r.get::<_, String>(5)?,
                "receipt_id": r.get::<_, Option<i64>>(6)?,
                "created_at": r.get::<_, i64>(7)?,
                "category": r.get::<_, Option<String>>(8)?,
            }))
        }).map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "expenses": expenses })))
}

// ── Summary ─────────────────────────────────────────────────────────────────

pub async fn handle_expense_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    // Default to YTD
    let year = chrono::Utc::now().format("%Y").to_string();
    let start = params.get("start").cloned().unwrap_or_else(|| format!("{}-01-01", year));
    let end = params.get("end").cloned().unwrap_or_else(|| format!("{}-12-31", year));

    let summary = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // By category
        let mut stmt = conn.prepare(
            "SELECT c.name, c.entity, c.tax_deductible, SUM(e.amount_cents), COUNT(*) \
             FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.expense_date >= ? AND e.expense_date <= ? \
             GROUP BY c.id ORDER BY SUM(e.amount_cents) DESC"
        ).map_err(|e| e.to_string())?;
        let categories: Vec<serde_json::Value> = stmt.query_map(
            rusqlite::params![uid, &start, &end],
            |r| {
                let total: i64 = r.get(3)?;
                Ok(serde_json::json!({
                    "category": r.get::<_, String>(0)?,
                    "entity": r.get::<_, String>(1)?,
                    "tax_deductible": r.get::<_, i64>(2)? != 0,
                    "total_cents": total,
                    "total_display": cents_to_display(total),
                    "count": r.get::<_, i64>(4)?,
                }))
            }
        ).map_err(|e| e.to_string())?
        .filter_map(|r| r.ok()).collect();

        // Totals
        let total_all: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE user_id = ? AND expense_date >= ? AND expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let total_business: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE user_id = ? AND entity = 'business' AND expense_date >= ? AND expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let total_deductible: i64 = conn.query_row(
            "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND c.tax_deductible = 1 AND e.expense_date >= ? AND e.expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let receipt_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM receipts WHERE user_id = ? AND created_at >= ? AND created_at <= ?",
            rusqlite::params![uid, chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").map(|d| d.and_hms_opt(0,0,0).unwrap().and_utc().timestamp()).unwrap_or(0),
                chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d").map(|d| d.and_hms_opt(23,59,59).unwrap().and_utc().timestamp()).unwrap_or(i64::MAX)],
            |r| r.get(0)
        ).unwrap_or(0);

        Ok(serde_json::json!({
            "period": { "start": start, "end": end },
            "total_cents": total_all,
            "total_display": cents_to_display(total_all),
            "business_cents": total_business,
            "business_display": cents_to_display(total_business),
            "deductible_cents": total_deductible,
            "deductible_display": cents_to_display(total_deductible),
            "receipt_count": receipt_count,
            "categories": categories,
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(summary))
}

// ── Categories ──────────────────────────────────────────────────────────────

pub async fn handle_category_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await?;
    let db = state.db_path.clone();

    let categories = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, name, entity, tax_deductible FROM expense_categories ORDER BY entity, name"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?,
            "name": r.get::<_, String>(1)?,
            "entity": r.get::<_, String>(2)?,
            "tax_deductible": r.get::<_, i64>(3)? != 0,
        }))).map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "categories": categories })))
}

// ── Income ───────────────────────────────────────────────────────────────────

pub async fn handle_income_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or(2025);

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Check if tax_income table exists
        let has_table: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tax_income'",
            [], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0;

        if !has_table {
            return Ok(serde_json::json!({ "income": [], "total_cents": 0, "total_display": "$0.00" }));
        }

        let mut stmt = conn.prepare(
            "SELECT source, amount_cents, category, description FROM tax_income WHERE user_id = ? AND tax_year = ? ORDER BY amount_cents DESC"
        ).map_err(|e| e.to_string())?;
        let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok(serde_json::json!({
                "source": r.get::<_, String>(0)?,
                "amount_cents": r.get::<_, i64>(1)?,
                "category": r.get::<_, Option<String>>(2)?,
                "description": r.get::<_, Option<String>>(3)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        Ok(serde_json::json!({
            "income": rows,
            "total_cents": total,
            "total_display": cents_to_display(total),
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

// ── CSV Export ───────────────────────────────────────────────────────────────

pub async fn handle_expense_export(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, String), StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let year = chrono::Utc::now().format("%Y").to_string();
    let start = params.get("start").cloned().unwrap_or_else(|| format!("{}-01-01", year));
    let end = params.get("end").cloned().unwrap_or_else(|| format!("{}-12-31", year));

    let csv = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT e.expense_date, e.vendor, e.amount_cents, c.name, e.entity, e.description \
             FROM expenses e LEFT JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.expense_date >= ? AND e.expense_date <= ? \
             ORDER BY e.expense_date"
        ).map_err(|e| e.to_string())?;
        let mut lines = vec!["Date,Vendor,Amount,Category,Entity,Description".to_string()];
        let rows = stmt.query_map(rusqlite::params![uid, &start, &end], |r| {
            let cents: i64 = r.get(2)?;
            Ok(format!("{},{},{},{},{},{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?.replace(',', ";"),
                cents_to_display(cents),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?.unwrap_or_default().replace(',', ";"),
            ))
        }).map_err(|e| e.to_string())?;
        for r in rows { if let Ok(line) = r { lines.push(line); } }
        Ok(lines.join("\n"))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/csv".parse().unwrap());
    let s = params.get("start").cloned().unwrap_or_default();
    let e = params.get("end").cloned().unwrap_or_default();
    headers.insert("content-disposition", format!("attachment; filename=\"expenses-{}-{}.csv\"", s, e).parse().unwrap());
    Ok((headers, csv))
}
