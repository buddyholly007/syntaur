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

// ── Module Licensing ────────────────────────────────────────────────────────

/// Check if a user has access to a given module.
/// Returns Ok(access_info) with status, or Err for DB errors.
/// Access is granted if: (1) user has Pro license, or (2) active trial.
pub fn check_module_access(conn: &rusqlite::Connection, user_id: i64, module: &str) -> Result<ModuleAccess, String> {
    let now = chrono::Utc::now().timestamp();

    // 1. Check for Pro license
    let has_license: bool = conn.query_row(
        "SELECT COUNT(*) FROM user_licenses WHERE user_id = ? AND license_type = 'pro'",
        rusqlite::params![user_id], |r| r.get::<_, i64>(0)
    ).unwrap_or(0) > 0;

    if has_license {
        return Ok(ModuleAccess { granted: true, reason: "pro".to_string(), trial_expires_at: None, trial_days_left: None });
    }

    // 2. Check for active trial
    let trial = conn.query_row(
        "SELECT trial_started_at, trial_expires_at FROM module_trials WHERE user_id = ? AND module_name = ?",
        rusqlite::params![user_id, module], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    ).ok();

    if let Some((started, expires)) = trial {
        if now < expires {
            let days_left = ((expires - now) as f64 / 86400.0).ceil() as i64;
            return Ok(ModuleAccess {
                granted: true,
                reason: "trial".to_string(),
                trial_expires_at: Some(expires),
                trial_days_left: Some(days_left),
            });
        } else {
            return Ok(ModuleAccess { granted: false, reason: "trial_expired".to_string(), trial_expires_at: Some(expires), trial_days_left: Some(0) });
        }
    }

    // 3. No license, no trial — module is locked but trial is available
    Ok(ModuleAccess { granted: false, reason: "no_access".to_string(), trial_expires_at: None, trial_days_left: None })
}

#[derive(Serialize, Clone)]
pub struct ModuleAccess {
    pub granted: bool,
    pub reason: String,    // "pro", "trial", "trial_expired", "no_access"
    pub trial_expires_at: Option<i64>,
    pub trial_days_left: Option<i64>,
}

/// Start a free trial for a module. Returns the trial info.
pub fn start_module_trial(conn: &rusqlite::Connection, user_id: i64, module: &str) -> Result<ModuleAccess, String> {
    let now = chrono::Utc::now().timestamp();

    // Check if trial already exists
    let existing = conn.query_row(
        "SELECT trial_expires_at FROM module_trials WHERE user_id = ? AND module_name = ?",
        rusqlite::params![user_id, module], |r| r.get::<_, i64>(0)
    ).ok();

    if let Some(expires) = existing {
        let days_left = std::cmp::max(0, ((expires - now) as f64 / 86400.0).ceil() as i64);
        return Ok(ModuleAccess {
            granted: now < expires,
            reason: if now < expires { "trial".to_string() } else { "trial_expired".to_string() },
            trial_expires_at: Some(expires),
            trial_days_left: Some(days_left),
        });
    }

    // Get trial duration from modules table (default 3 days)
    let trial_days: i64 = conn.query_row(
        "SELECT trial_days FROM modules WHERE name = ?",
        rusqlite::params![module], |r| r.get(0)
    ).unwrap_or(3);

    let expires = now + (trial_days * 86400);
    conn.execute(
        "INSERT INTO module_trials (user_id, module_name, trial_started_at, trial_expires_at) VALUES (?, ?, ?, ?)",
        rusqlite::params![user_id, module, now, expires],
    ).map_err(|e| e.to_string())?;

    info!("[license] Started {}-day trial for user {} on module {}", trial_days, user_id, module);

    Ok(ModuleAccess {
        granted: true,
        reason: "trial".to_string(),
        trial_expires_at: Some(expires),
        trial_days_left: Some(trial_days),
    })
}

/// API: Get module access status for the current user
pub async fn handle_module_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let module = params.get("module").cloned().unwrap_or_else(|| "tax".to_string());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let access = check_module_access(&conn, uid, &module)?;

        // Get all available modules
        let modules: Vec<serde_json::Value> = {
            if let Ok(mut stmt) = conn.prepare("SELECT name, display_name, description, icon, trial_days, enabled FROM modules") {
                stmt.query_map([], |r| Ok(serde_json::json!({
                    "name": r.get::<_, String>(0)?,
                    "display_name": r.get::<_, String>(1)?,
                    "description": r.get::<_, Option<String>>(2)?,
                    "icon": r.get::<_, Option<String>>(3)?,
                    "trial_days": r.get::<_, i64>(4)?,
                    "enabled": r.get::<_, i64>(5)? != 0,
                }))).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default()
            } else { vec![] }
        };

        // Check trial status for each module
        let mut module_access: Vec<serde_json::Value> = Vec::new();
        for m in &modules {
            let name = m["name"].as_str().unwrap_or("");
            let ma = check_module_access(&conn, uid, name).unwrap_or(ModuleAccess {
                granted: false, reason: "error".to_string(), trial_expires_at: None, trial_days_left: None
            });
            module_access.push(serde_json::json!({
                "module": m,
                "access": {
                    "granted": ma.granted,
                    "reason": ma.reason,
                    "trial_expires_at": ma.trial_expires_at,
                    "trial_days_left": ma.trial_days_left,
                }
            }));
        }

        Ok(serde_json::json!({
            "module": module,
            "access": {
                "granted": access.granted,
                "reason": access.reason,
                "trial_expires_at": access.trial_expires_at,
                "trial_days_left": access.trial_days_left,
            },
            "has_pro": access.reason == "pro",
            "all_modules": module_access,
            "pro_price_cents": 4900,
            "pro_price_display": "$49.00",
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

/// API: Start a free trial for a module
pub async fn handle_start_trial(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TrialRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let module = req.module.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<ModuleAccess, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        start_module_trial(&conn, uid, &module)
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "module": req.module,
        "granted": result.granted,
        "reason": result.reason,
        "trial_expires_at": result.trial_expires_at,
        "trial_days_left": result.trial_days_left,
    })))
}

#[derive(Deserialize)]
pub struct TrialRequest {
    pub token: String,
    pub module: String,
}

/// API: Activate a Pro license (called after payment verification)
pub async fn handle_activate_license(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LicenseActivateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let payment_id = req.payment_id.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT OR REPLACE INTO user_licenses (user_id, license_type, purchased_at, payment_id, amount_cents) VALUES (?, 'pro', ?, ?, 4900)",
            rusqlite::params![uid, now, &payment_id],
        ).map_err(|e| e.to_string())?;
        info!("[license] Pro license activated for user {} (payment: {:?})", uid, payment_id);
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "license": "pro" })))
}

#[derive(Deserialize)]
pub struct LicenseActivateRequest {
    pub token: String,
    pub payment_id: Option<String>,
}

// ── Tax Bracket Loading ─────────────────────────────────────────────────────

/// Load tax brackets from ~/.syntaur/tax_brackets.json or fall back to embedded defaults.
pub fn load_brackets(year: i64, filing_status: &str) -> (Vec<(i64, i64)>, i64, i64) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let path = format!("{}/.syntaur/tax_brackets.json", home);

    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&data) {
            let year_str = year.to_string();
            if let Some(year_data) = config.get("brackets").and_then(|b| b.get(&year_str)) {
                if let Some(status_data) = year_data.get(filing_status) {
                    let brackets: Vec<(i64, i64)> = status_data.get("brackets")
                        .and_then(|b| b.as_array())
                        .map(|arr| arr.iter().filter_map(|v| {
                            let a = v.as_array()?;
                            Some((a.get(0)?.as_i64()?, a.get(1)?.as_i64()?))
                        }).collect())
                        .unwrap_or_default();
                    let top_rate = status_data.get("top_rate").and_then(|v| v.as_i64()).unwrap_or(3700);
                    let std_ded = status_data.get("standard_deduction").and_then(|v| v.as_i64()).unwrap_or(3020000);

                    if !brackets.is_empty() {
                        return (brackets, top_rate, std_ded);
                    }
                }
            }
        }
    }

    // Fallback: hardcoded 2025 brackets
    log::warn!("[tax] Could not load brackets for {} {} from config, using hardcoded 2025 defaults", year, filing_status);
    match filing_status {
        "married_jointly" => (
            vec![(2385000,1000),(9695000,1200),(20670000,2200),(39460000,2400),(50105000,3200),(75160000,3500)],
            3700, 3020000
        ),
        "head_of_household" => (
            vec![(1700000,1000),(6475000,1200),(10335000,2200),(19730000,2400),(25052500,3200),(62660000,3500)],
            3700, 2265000
        ),
        _ => (
            vec![(1192500,1000),(4847500,1200),(10335000,2200),(19730000,2400),(25052500,3200),(62660000,3500)],
            3700, 1510000
        ),
    }
}

/// Check if brackets are stale (config file older than 14 months)
pub fn brackets_stale() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let path = format!("{}/.syntaur/tax_brackets.json", home);

    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(updated) = config.get("last_updated").and_then(|v| v.as_str()) {
                if let Ok(date) = chrono::NaiveDate::parse_from_str(updated, "%Y-%m-%d") {
                    let age = chrono::Utc::now().date_naive() - date;
                    if age.num_days() > 425 { // ~14 months
                        return Some(format!("Tax brackets were last updated {}. They may be outdated for the current tax year.", updated));
                    }
                }
            }
            // Check if current year's brackets exist
            let current_year = chrono::Utc::now().format("%Y").to_string();
            if config.get("brackets").and_then(|b| b.get(&current_year)).is_none() {
                let prev_year: i64 = current_year.parse::<i64>().unwrap_or(2026) - 1;
                return Some(format!("Tax brackets for {} are not yet available. Using {} brackets. The IRS typically publishes new brackets each November.", current_year, prev_year));
            }
        }
    } else {
        return Some("Tax brackets config not found. Using built-in 2025 defaults.".to_string());
    }
    None
}

/// Write updated brackets to config file
pub fn save_brackets(config: &serde_json::Value) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let path = format!("{}/.syntaur/tax_brackets.json", home);
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, &json).map_err(|e| format!("Write brackets: {}", e))?;
    log::info!("[tax] Updated tax brackets config at {}", path);
    Ok(())
}

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

/// Convert a PDF to high-resolution PNGs using pdftoppm (poppler-utils).
/// Returns PNG bytes for each page at 300 DPI. For multi-page docs, all
/// pages are rendered and concatenated into the vision request.
fn convert_pdf_to_png(pdf_path: &str) -> Result<Vec<u8>, String> {
    convert_pdf_to_pngs(pdf_path).map(|pages| pages.into_iter().next().unwrap_or_default())
}

fn convert_pdf_to_pngs(pdf_path: &str) -> Result<Vec<Vec<u8>>, String> {
    let output_prefix = format!("{}.render", pdf_path);
    let result = std::process::Command::new("pdftoppm")
        .args(["-png", "-r", "300", pdf_path, &output_prefix])
        .output()
        .map_err(|e| format!("pdftoppm not available: {}", e))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!("pdftoppm failed: {}", stderr.chars().take(200).collect::<String>()));
    }

    // Collect all rendered pages — pdftoppm uses various naming patterns:
    // prefix-1.png, prefix-01.png, prefix.png (single page)
    let mut pages = Vec::new();
    for i in 1..=20 {
        // Try zero-padded first, then unpadded
        let candidates = [
            format!("{}-{:02}.png", output_prefix, i),
            format!("{}-{}.png", output_prefix, i),
        ];
        let mut found = false;
        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                pages.push(data);
                let _ = std::fs::remove_file(path);
                found = true;
                break;
            }
        }
        if !found {
            if i == 1 {
                // Single-page: no number suffix
                let path = format!("{}.png", output_prefix);
                if let Ok(data) = std::fs::read(&path) {
                    pages.push(data);
                    let _ = std::fs::remove_file(&path);
                }
            }
            break;
        }
    }

    if pages.is_empty() {
        return Err("pdftoppm produced no output".to_string());
    }

    Ok(pages)
}

fn receipts_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let dir = PathBuf::from(format!("{}/.syntaur/receipts", home));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// ── Shared Tax Estimate Calculation ─────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct TaxEstimate {
    pub gross_income: i64,
    pub se_income: i64,
    pub w2_income: i64,
    pub biz_deductions: i64,
    pub meals_total: i64,
    pub meals_adjustment: i64,
    pub se_tax: i64,
    pub half_se_tax: i64,
    pub se_health_deduction: i64,
    pub student_loan_deduction: i64,
    pub agi: i64,
    pub standard_deduction: i64,
    pub itemized_deduction: i64,
    pub salt_capped: i64,
    pub medical_deductible: i64,
    pub qbi_deduction: i64,
    pub deduction_used: i64,
    pub deduction_type: String,
    pub taxable_income: i64,
    pub ordinary_tax: i64,
    pub ltcg_tax: i64,
    pub total_tax: i64,
    pub w2_withheld: i64,
    pub fed_paid: i64,
    pub child_credit: i64,
    pub total_payments: i64,
    pub owed: i64,
    pub effective_rate: f64,
}

pub fn compute_tax_estimate(
    conn: &rusqlite::Connection,
    user_id: i64,
    year: i64,
    filing_status: &str,
) -> Result<TaxEstimate, String> {
    let start = format!("{}-01-01", year);
    let end = format!("{}-12-31", year);

    // ── Gather income ──
    let gross_income: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);

    // SE income = 1099-NEC + 1099-MISC (self-employment)
    let se_income: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND (category LIKE '%1099%NEC%' OR category LIKE '%1099%MISC%' OR category LIKE '%Self-Employ%' OR category LIKE '%Schedule C%')",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);

    let w2_income = gross_income - se_income;

    // ── Business deductions ──
    let biz_deductions: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND e.entity = 'business' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);

    // Meals 50% adjustment
    let meals_total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND e.entity = 'business' AND c.name = 'Meals & Entertainment' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);
    let meals_adjustment = meals_total / 2; // only 50% deductible
    let biz_deductions_adj = biz_deductions - meals_adjustment;

    // ── SE Tax ──
    let se_net = std::cmp::max(se_income - biz_deductions_adj, 0);
    let se_taxable = (se_net as f64 * 0.9235) as i64;
    let ss_wage_base: i64 = 17_610_000; // $176,100 for 2025
    let se_tax_ss = std::cmp::min(se_taxable, ss_wage_base) * 1240 / 10000; // 12.4%
    let se_tax_medicare = se_taxable * 290 / 10000; // 2.9%
    let se_tax = se_tax_ss + se_tax_medicare;
    let half_se_tax = se_tax / 2;

    // ── SE Health Insurance deduction ──
    let se_health_deduction: i64 = {
        let q_answers: serde_json::Value = conn.query_row(
            "SELECT answers_json FROM deduction_questionnaire WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![user_id, year], |r| r.get::<_, String>(0),
        ).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();

        let is_se = q_answers.get("self_employed").and_then(|v| v.as_bool()).unwrap_or(se_income > 0);
        let pays_own = q_answers.get("health_insurance_self").and_then(|v| v.as_bool()).unwrap_or(false);

        if is_se && pays_own {
            // Sum health insurance expenses
            let hi: i64 = conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND (c.name LIKE '%Health Insurance%' OR c.name = 'Medical') \
                 AND e.entity IN ('business', 'personal') AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![user_id, &start, &end], |r| r.get(0),
            ).unwrap_or(0);
            std::cmp::min(hi, se_net) // capped at net SE income
        } else { 0 }
    };

    // Student loan interest
    let student_loan_deduction: i64 = {
        let sl: i64 = conn.query_row(
            "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
             JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND c.name LIKE '%Student Loan%' AND e.expense_date >= ? AND e.expense_date <= ?",
            rusqlite::params![user_id, &start, &end], |r| r.get(0),
        ).unwrap_or(0);
        std::cmp::min(sl, 250_000) // $2,500 cap
    };

    // ── AGI ──
    let agi = gross_income - biz_deductions_adj - half_se_tax - se_health_deduction - student_loan_deduction;

    // ── Itemized deductions ──
    let personal_deductible: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND c.tax_deductible = 1 AND e.entity = 'personal' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);

    // SALT cap: $40,000 (OBBBA 2025-2029)
    let salt_cap: i64 = 4_000_000; // $40,000 in cents
    let property_tax: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND (c.name LIKE '%Property Tax%' OR c.name = 'Mortgage') AND e.entity = 'personal' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);
    let salt_capped = std::cmp::min(property_tax, salt_cap);
    let salt_reduction = property_tax - salt_capped;

    // Medical: only above 7.5% AGI
    let medical_total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND c.name = 'Medical' AND e.entity = 'personal' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);
    let medical_floor = (agi as f64 * 0.075) as i64;
    let medical_deductible = std::cmp::max(medical_total - medical_floor, 0);

    let itemized_deduction = personal_deductible - salt_reduction - (medical_total - medical_deductible);

    // Standard deduction
    let (brackets, top_rate, standard_deduction) = load_brackets(year, filing_status);
    let deduction_used = std::cmp::max(standard_deduction, itemized_deduction);
    let deduction_type = if itemized_deduction > standard_deduction { "Itemized" } else { "Standard" };

    // ── QBI deduction (Section 199A) ──
    let qbi_deduction = if se_net > 0 {
        let qbi_raw = se_net * 2000 / 10000; // 20%
        let qbi_cap = std::cmp::max(agi - deduction_used, 0) * 2000 / 10000;
        let qbi = std::cmp::min(qbi_raw, qbi_cap);
        // Phase-out
        let (phase_start, phase_range) = match filing_status {
            "married_jointly" => (38_390_000i64, 10_000_000i64),
            _ => (19_195_000i64, 5_000_000i64),
        };
        if agi > phase_start + phase_range { 0 }
        else if agi > phase_start {
            let reduction = ((agi - phase_start) as f64 / phase_range as f64).min(1.0);
            ((1.0 - reduction) * qbi as f64) as i64
        } else { qbi }
    } else { 0 };

    // ── Taxable income ──
    let taxable_income = std::cmp::max(agi - deduction_used - qbi_deduction, 0);

    // ── Capital gains ──
    let long_term_gains: i64 = conn.query_row(
        "SELECT COALESCE(SUM(realized_pl_cents), 0) FROM investment_transactions \
         WHERE user_id = ? AND transaction_date >= ? AND transaction_date <= ? AND activity_type = 'long_term'",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);
    let ltcg = std::cmp::max(long_term_gains, 0);

    // LTCG brackets (2025)
    let ltcg_brackets: Vec<(i64, i64)> = match filing_status {
        "married_jointly" => vec![(9_670_000, 0), (60_005_000, 1500), (i64::MAX, 2000)],
        _ => vec![(4_835_000, 0), (53_375_000, 1500), (i64::MAX, 2000)],
    };
    let ordinary_portion = std::cmp::max(taxable_income - ltcg, 0);

    // Ordinary tax
    let mut ordinary_tax: i64 = 0;
    let mut prev = 0i64;
    for &(limit, rate_bps) in &brackets {
        let bracket_income = std::cmp::min(ordinary_portion, limit) - prev;
        if bracket_income <= 0 { break; }
        ordinary_tax += bracket_income * rate_bps / 10000;
        prev = limit;
    }
    if ordinary_portion > prev {
        ordinary_tax += (ordinary_portion - prev) * top_rate / 10000;
    }

    // LTCG tax
    let mut ltcg_tax: i64 = 0;
    let mut prev_ltcg = 0i64;
    for &(limit, rate_bps) in &ltcg_brackets {
        let bracket_income = std::cmp::min(ltcg, limit) - prev_ltcg;
        if bracket_income <= 0 { break; }
        ltcg_tax += bracket_income * rate_bps / 10000;
        prev_ltcg = limit;
    }

    let total_tax = ordinary_tax + ltcg_tax + se_tax;

    // ── Payments & Credits ──
    let w2_withheld: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category LIKE '%Withholding%'",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);
    let fed_paid: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
         JOIN expense_categories c ON e.category_id = c.id \
         WHERE e.user_id = ? AND c.name LIKE 'Federal Income Tax%' AND e.expense_date >= ? AND e.expense_date <= ?",
        rusqlite::params![user_id, &start, &end], |r| r.get(0),
    ).unwrap_or(0);

    // Child tax credit
    let dependents: i64 = conn.query_row(
        "SELECT answers_json FROM deduction_questionnaire WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year], |r| r.get::<_, String>(0),
    ).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    .and_then(|v| v.get("dependents")?.as_i64()).unwrap_or(0);
    let child_credit = dependents * 200_000; // $2,000 per child

    let total_payments = w2_withheld + fed_paid + child_credit;
    let owed = total_tax - total_payments;

    let effective_rate = if gross_income > 0 { (total_tax as f64 / gross_income as f64) * 100.0 } else { 0.0 };

    Ok(TaxEstimate {
        gross_income, se_income, w2_income, biz_deductions: biz_deductions_adj,
        meals_total, meals_adjustment, se_tax, half_se_tax, se_health_deduction,
        student_loan_deduction, agi, standard_deduction, itemized_deduction: itemized_deduction,
        salt_capped, medical_deductible, qbi_deduction, deduction_used, deduction_type: deduction_type.to_string(),
        taxable_income, ordinary_tax, ltcg_tax, total_tax, w2_withheld, fed_paid,
        child_credit, total_payments, owed, effective_rate,
    })
}

// ── Module Gate Helper ──────────────────────────────────────────────────────

/// Check if the user has access to the tax module. Returns Ok(()) if granted,
/// or Err with a 403 and a JSON body explaining the lock + how to unlock.
async fn require_tax_module(state: &AppState, user_id: i64) -> Result<(), (StatusCode, String)> {
    let db = state.db_path.clone();
    let uid = user_id;
    let access = tokio::task::spawn_blocking(move || -> Result<ModuleAccess, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        check_module_access(&conn, uid, "tax")
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if access.granted {
        return Ok(());
    }

    let msg = match access.reason.as_str() {
        "trial_expired" => "Your free trial of the Tax & Expenses module has ended. Upgrade to Syntaur Pro ($49) to unlock all modules.".to_string(),
        _ => "The Tax & Expenses module requires Syntaur Pro ($49) or a free trial. Start a 3-day free trial to try it out.".to_string(),
    };

    Err((StatusCode::PAYMENT_REQUIRED, serde_json::json!({
        "error": "module_locked",
        "module": "tax",
        "message": msg,
        "reason": access.reason,
        "trial_available": access.reason == "no_access",
        "pro_price": "$49.00",
    }).to_string()))
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

    // Module gate: receipt upload requires tax module access
    require_tax_module(&state, principal.user_id()).await?;

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

    // Convert PDF to PNG for better vision accuracy
    let (image_bytes, mime) = if image_path.ends_with(".pdf") {
        match convert_pdf_to_png(&image_path) {
            Ok(png) => (png, "image/png"),
            Err(e) => {
                log::warn!("[tax] PDF→PNG failed for receipt ({}), using raw", e);
                (std::fs::read(&image_path).map_err(|e| e.to_string())?, "application/pdf")
            }
        }
    } else {
        let ext = image_path.rsplit('.').next().unwrap_or("jpg");
        let m = match ext { "png" => "image/png", _ => "image/jpeg" };
        (std::fs::read(&image_path).map_err(|e| e.to_string())?, m)
    };
    use base64::Engine;
    let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

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

    // Force a vision-capable model optimized for document OCR
    let model = if provider_config.base_url.contains("openrouter") {
        "nvidia/nemotron-nano-12b-v2-vl:free" // #1 on OCRBench, free, purpose-built for documents
    } else if provider_config.base_url.contains("anthropic") {
        "claude-sonnet-4-6"
    } else if provider_config.base_url.contains("openai") {
        "gpt-4o-mini"
    } else {
        provider_config.models.first()
            .map(|m| m.id.as_str())
            .or_else(|| provider_config.extra.get("model").and_then(|v| v.as_str()))
            .unwrap_or("gpt-4o-mini")
    };

    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    let prompt = r#"Analyze this receipt/invoice image carefully. Read ALL text on the document.

Extract these fields:
1. Vendor/Store name (the business that issued this receipt, NOT a payment processor or card network)
2. Total amount paid (in dollars, e.g. "45.99" — look for "Total", "Grand Total", "Amount Due", or the final amount)
3. Date of transaction (YYYY-MM-DD format — look for purchase date, transaction date, or order date)
4. Category — choose the BEST match:
   Business: Advertising & Marketing, Equipment & Tools, Hardware & Supplies, Lumber & Raw Materials, Office Supplies, Professional Services, Rent & Utilities, Insurance, Software & Subscriptions, Shipping & Packaging, Vehicle & Mileage, Education & Training, Meals & Entertainment, Travel, Tools - Consumables, Safety Gear, Power Tools, Shop Maintenance, Dust Collection, Fuel Equipment, Furniture and Equipment, Backup Power Equipment, Supplies
   Personal: Medical, Mortgage, Vehicle, Donations, Education, Home Improvement, Utilities, Groceries, Dining, Entertainment, Other
5. Brief description of what was purchased (list main items)

IMPORTANT: Read numbers carefully. Double-check the total amount. If the receipt shows tax separately, use the grand total including tax.

Respond ONLY with valid JSON: {"vendor":"...","amount":"45.99","date":"2025-12-15","category":"...","description":"..."}"#;

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

    // Validate: flag suspicious category/amount combinations
    let suspicious = match category_name.as_str() {
        "Homeowner's Insurance Premium" | "Insurance" if amount_cents > 1_000_000 =>
            Some("Insurance amount over $10,000 — verify this isn't a closing cost or escrow payment"),
        _ if amount_cents > 5_000_000 =>
            Some("Amount over $50,000 — verify this is a single expense and not a statement total"),
        _ => None,
    };
    if let Some(warning) = suspicious {
        log::warn!("[tax] Receipt #{} flagged: {} ({})", receipt_id, warning, cents_to_display(amount_cents));
    }

    // Validate: skip auto-expense for non-receipt documents (W-2, 1099, etc.)
    let vendor_lower = vendor_log.to_lowercase();
    let is_tax_form = vendor_lower.contains("w-2") || vendor_lower.contains("w2") ||
        vendor_lower.contains("1099") || vendor_lower.contains("1098") ||
        vendor_lower.contains("1095") || vendor_lower.contains("irs") ||
        vendor_lower.contains("internal revenue") || vendor_lower.contains("tax form") ||
        category_name == "Other" && amount_cents > 50_000_00;

    if is_tax_form {
        log::info!("[tax] Receipt #{} looks like a tax form, not a receipt — skipping expense creation", receipt_id);
        // Update receipt status but don't create an expense
        let db_skip = db.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = rusqlite::Connection::open(&db_skip) {
                let _ = conn.execute("UPDATE receipts SET status = 'tax_form' WHERE id = ?", rusqlite::params![receipt_id]);
            }
        }).await.ok();
        return Ok(());
    }

    if amount_cents > 100_000_00 {
        log::warn!("[tax] Receipt #{} has unusually large amount: {} — may need verification", receipt_id, cents_to_display(amount_cents));
    }

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

    // Module gate: tax document upload requires tax module access
    require_tax_module(&state, principal.user_id()).await?;

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

    // Convert PDF to high-resolution PNGs (all pages) for better accuracy
    use base64::Engine;
    let image_parts: Vec<(String, &str)> = if image_path.ends_with(".pdf") {
        match convert_pdf_to_pngs(&image_path) {
            Ok(pages) => pages.into_iter().map(|p| (base64::engine::general_purpose::STANDARD.encode(&p), "image/png")).collect(),
            Err(e) => {
                log::warn!("[tax] PDF conversion failed ({}), sending raw PDF", e);
                let data = std::fs::read(&image_path).map_err(|e| e.to_string())?;
                vec![(base64::engine::general_purpose::STANDARD.encode(&data), "application/pdf")]
            }
        }
    } else {
        let data = std::fs::read(&image_path).map_err(|e| e.to_string())?;
        let ext = image_path.rsplit('.').next().unwrap_or("jpg");
        let m = match ext { "png" => "image/png", _ => "image/jpeg" };
        vec![(base64::engine::general_purpose::STANDARD.encode(&data), m)]
    };

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

    // Build content with all pages
    let mut content_parts: Vec<serde_json::Value> = vec![serde_json::json!({"type": "text", "text": prompt})];
    for (b64, mime) in &image_parts {
        content_parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime, b64)}}));
    }

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content_parts}],
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

    // Check for duplicate documents before saving
    let db_dup = db.clone();
    let doc_type_dup = doc_type.clone();
    let issuer_dup = issuer.clone();
    let tax_year_dup = tax_year;
    let dup_check = tokio::task::spawn_blocking(move || -> Option<(i64, String)> {
        let conn = rusqlite::Connection::open(&db_dup).ok()?;
        let uid: i64 = conn.query_row("SELECT user_id FROM tax_documents WHERE id = ?", rusqlite::params![doc_id], |r| r.get(0)).ok()?;
        // Look for existing document with same type, issuer, and year
        conn.query_row(
            "SELECT id, issuer FROM tax_documents WHERE user_id = ? AND doc_type = ? AND tax_year = ? AND id != ? AND status = 'scanned'",
            rusqlite::params![uid, &doc_type_dup, tax_year_dup, doc_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default()))
        ).ok()
    }).await.unwrap_or(None);

    let is_duplicate = if let Some((existing_id, existing_issuer)) = &dup_check {
        // Check if issuer is similar (same company, different wording)
        let i1 = issuer.to_lowercase();
        let i2 = existing_issuer.to_lowercase();
        i1.contains(&i2) || i2.contains(&i1) ||
        i1.split_whitespace().next() == i2.split_whitespace().next()
    } else { false };

    let status = if is_duplicate { "duplicate" } else { "scanned" };
    if is_duplicate {
        if let Some((eid, ei)) = &dup_check {
            log::warn!("[tax] Document #{} appears to be a duplicate of #{} ({}) — marked for review", doc_id, eid, ei);
        }
    }

    // Update document record
    let db2 = db.clone();
    let doc_type2 = doc_type.clone();
    let issuer2 = issuer.clone();
    let fields_str2 = fields_str.clone();
    let status2 = status.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE tax_documents SET doc_type = ?, tax_year = ?, issuer = ?, extracted_fields = ?, status = ? WHERE id = ?",
            rusqlite::params![&doc_type2, tax_year, &issuer2, &fields_str2, &status2, doc_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|e| e.to_string())??;

    // Auto-populate income data from W-2s (skip duplicates)
    if doc_type == "w2" && !is_duplicate {
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

            // Upsert income: check for existing record by employee name to avoid duplicates
            if wages > 0 {
                let desc = format!("{} - {}", issuer3, employee);
                let existing: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM tax_income WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Wages' AND description LIKE ?",
                    rusqlite::params![uid, year, format!("%{}%", employee)], |r| r.get(0)
                ).unwrap_or(0);
                if existing == 0 {
                    conn.execute(
                        "INSERT INTO tax_income (user_id, source, amount_cents, tax_year, category, description, created_at) VALUES (?, 'W-2 Wages', ?, ?, 'Wages', ?, ?)",
                        rusqlite::params![uid, wages, year, &desc, now],
                    ).map_err(|e| e.to_string())?;
                } else {
                    conn.execute(
                        "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Wages' AND description LIKE ?",
                        rusqlite::params![wages, uid, year, format!("%{}%", employee)],
                    ).map_err(|e| e.to_string())?;
                }
            }

            if withheld > 0 {
                let desc = format!("{} - {} Box 2", issuer3, employee);
                let existing: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM tax_income WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Withholding' AND description LIKE ?",
                    rusqlite::params![uid, year, format!("%{}%", employee)], |r| r.get(0)
                ).unwrap_or(0);
                if existing == 0 {
                    conn.execute(
                        "INSERT INTO tax_income (user_id, source, amount_cents, tax_year, category, description, created_at) VALUES (?, 'W-2 Withholding', ?, ?, 'Federal Withholding', ?, ?)",
                        rusqlite::params![uid, withheld, year, &desc, now],
                    ).map_err(|e| e.to_string())?;
                } else {
                    conn.execute(
                        "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Withholding' AND description LIKE ?",
                        rusqlite::params![withheld, uid, year, format!("%{}%", employee)],
                    ).map_err(|e| e.to_string())?;
                }
            }

            Ok(())
        }).await.map_err(|e| e.to_string())??;
    }

    // Validate extracted fields
    if doc_type == "w2" {
        let wages = fields["box1_wages"].as_f64().unwrap_or(0.0);
        let ss_wages = fields["box3_ss_wages"].as_f64().unwrap_or(0.0);
        let ss_withheld = fields["box4_ss_withheld"].as_f64().unwrap_or(0.0);
        let medicare_wages = fields["box5_medicare_wages"].as_f64().unwrap_or(0.0);
        let medicare_withheld = fields["box6_medicare_withheld"].as_f64().unwrap_or(0.0);

        // SS withholding should be ~6.2% of SS wages (capped at $176,100 for 2025)
        if ss_wages > 0.0 && ss_withheld > 0.0 {
            let expected_rate = ss_withheld / ss_wages;
            if expected_rate < 0.05 || expected_rate > 0.07 {
                log::warn!("[tax] W-2 #{}: SS withholding rate {:.2}% is outside expected 6.2% range — may need verification", doc_id, expected_rate * 100.0);
            }
        }
        // Medicare should be ~1.45% of Medicare wages
        if medicare_wages > 0.0 && medicare_withheld > 0.0 {
            let expected_rate = medicare_withheld / medicare_wages;
            if expected_rate < 0.01 || expected_rate > 0.025 {
                log::warn!("[tax] W-2 #{}: Medicare withholding rate {:.2}% is outside expected 1.45% range", doc_id, expected_rate * 100.0);
            }
        }
        // Box 5 Medicare wages should be >= Box 1 wages (Medicare has no cap)
        if medicare_wages > 0.0 && wages > 0.0 && medicare_wages < wages * 0.95 {
            log::warn!("[tax] W-2 #{}: Medicare wages ({}) less than Box 1 wages ({}) — unusual", doc_id, medicare_wages, wages);
        }
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
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? AND tax_year = ? AND status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(y)],
            ),
            None => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? AND status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
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

// ── Update Document Status (keep/discard duplicates) ────────────────────────

#[derive(Deserialize)]
pub struct UpdateStatusRequest {
    pub token: String,
    pub status: String,
}

pub async fn handle_tax_doc_update_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(req): Json<UpdateStatusRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let new_status = req.status.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE tax_documents SET status = ? WHERE id = ? AND user_id = ?",
            rusqlite::params![&new_status, id, uid],
        ).map_err(|e| e.to_string())?;

        // If discarding, also remove any income records created from this doc
        if new_status == "discarded" {
            // The income records don't directly reference doc_id, but we can
            // remove the document's image to save space
            if let Ok(path) = conn.query_row::<String, _, _>(
                "SELECT image_path FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)
            ) {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Update Document Field ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateFieldRequest {
    pub token: String,
    pub field: String,
    pub value: String,
}

pub async fn handle_tax_doc_update_field(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(req): Json<UpdateFieldRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let field = req.field.clone();
    let value = req.value.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Get current fields
        let current: String = conn.query_row(
            "SELECT COALESCE(extracted_fields, '{}') FROM tax_documents WHERE id = ? AND user_id = ?",
            rusqlite::params![id, uid], |r| r.get(0)
        ).map_err(|e| format!("Not found: {}", e))?;

        let mut fields: serde_json::Value = serde_json::from_str(&current).unwrap_or(serde_json::json!({}));

        // Try to parse as number, otherwise store as string
        if let Ok(num) = value.parse::<f64>() {
            fields[&field] = serde_json::json!(num);
        } else {
            fields[&field] = serde_json::json!(value);
        }

        let updated = serde_json::to_string(&fields).unwrap_or("{}".to_string());
        conn.execute(
            "UPDATE tax_documents SET extracted_fields = ? WHERE id = ? AND user_id = ?",
            rusqlite::params![&updated, id, uid],
        ).map_err(|e| e.to_string())?;

        // If this is a W-2 field, also update income records
        let doc_type: String = conn.query_row(
            "SELECT doc_type FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)
        ).unwrap_or_default();

        if doc_type == "w2" {
            let year: i64 = conn.query_row(
                "SELECT COALESCE(tax_year, 2025) FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)
            ).unwrap_or(2025);
            let issuer: String = conn.query_row(
                "SELECT COALESCE(issuer, '') FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)
            ).unwrap_or_default();

            if field == "box1_wages" {
                if let Ok(cents) = value.parse::<f64>().map(|f| (f * 100.0) as i64) {
                    conn.execute(
                        "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Wages' AND description LIKE ?",
                        rusqlite::params![cents, uid, year, format!("%{}%", issuer)],
                    ).ok();
                }
            } else if field == "box2_fed_withheld" {
                if let Ok(cents) = value.parse::<f64>().map(|f| (f * 100.0) as i64) {
                    conn.execute(
                        "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Withholding' AND description LIKE ?",
                        rusqlite::params![cents, uid, year, format!("%{}%", issuer)],
                    ).ok();
                }
            }
        }

        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true })))
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

    // Module gate: expense creation requires tax module access
    require_tax_module(&state, principal.user_id()).await?;

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

        let gross: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);
        let withheld: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category = 'Federal Withholding'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        Ok(serde_json::json!({
            "income": rows,
            "total_cents": gross,
            "total_display": cents_to_display(gross),
            "withheld_cents": withheld,
            "withheld_display": cents_to_display(withheld),
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

// ── App Update Check ─────────────────────────────────────────────────────────

pub async fn handle_update_check(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await?;

    let current = env!("CARGO_PKG_VERSION");
    let client = &state.client;

    // Check GitHub releases
    match client.get("https://api.github.com/repos/buddyholly007/syntaur/releases/latest")
        .header("User-Agent", "syntaur-gateway")
        .timeout(std::time::Duration::from_secs(10))
        .send().await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                let latest = data["tag_name"].as_str().unwrap_or("").trim_start_matches('v');
                let update_available = latest != current && !latest.is_empty();
                let notes = data["body"].as_str().unwrap_or("");
                let download_url = data["html_url"].as_str().unwrap_or("");

                return Ok(Json(serde_json::json!({
                    "current_version": current,
                    "latest_version": latest,
                    "update_available": update_available,
                    "release_notes": &notes[..notes.len().min(500)],
                    "download_url": download_url,
                })));
            }
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({
        "current_version": current,
        "latest_version": null,
        "update_available": false,
        "error": "Could not check for updates. Check your internet connection.",
    })))
}

// ── Tax Bracket Status + Update ──────────────────────────────────────────────

pub async fn handle_bracket_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await?;

    let warning = brackets_stale();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let path = format!("{}/.syntaur/tax_brackets.json", home);
    let last_updated = std::fs::read_to_string(&path).ok()
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .and_then(|c| c.get("last_updated").and_then(|v| v.as_str()).map(String::from));
    let available_years: Vec<String> = std::fs::read_to_string(&path).ok()
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .and_then(|c| c.get("brackets").and_then(|b| b.as_object()).map(|o| o.keys().cloned().collect()))
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "last_updated": last_updated,
        "available_years": available_years,
        "stale": warning.is_some(),
        "warning": warning,
        "config_path": path,
    })))
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

// ── TXF Export (TurboTax / H&R Block Desktop) ──────────────────────────────

/// Map expense category names to TXF reference numbers (Schedule C lines).
fn category_to_txf_refnum(category: &str) -> Option<(i32, &'static str)> {
    match category {
        "Advertising & Marketing" => Some((304, "Advertising")),
        "Vehicle & Mileage" => Some((306, "Car and truck expenses")),
        "Professional Services" => Some((327, "Legal and professional services")),
        "Office Supplies" => Some((328, "Office expense")),
        "Rent & Utilities" => Some((337, "Utilities")),
        "Insurance" => Some((324, "Insurance (other than health)")),
        "Meals & Entertainment" => Some((294, "Meals & entertainment")),
        "Travel" => Some((335, "Travel")),
        "Education & Training" => Some((339, "Other expenses")),
        "Software & Subscriptions" => Some((339, "Other expenses")),
        "Equipment & Tools" => Some((313, "Depreciation and section 179")),
        "Hardware & Supplies" => Some((333, "Supplies")),
        "Lumber & Raw Materials" => Some((333, "Supplies")),
        "Tools - Consumables" => Some((333, "Supplies")),
        "Safety Gear" => Some((333, "Supplies")),
        "Shipping & Packaging" => Some((339, "Other expenses")),
        "Miscellaneous Business" => Some((339, "Other expenses")),
        // Personal deductions (Schedule A)
        "Medical" => Some((273, "Medicine and drugs")),
        "Mortgage" => Some((283, "Home mortgage interest")),
        "Donations" => Some((280, "Cash charitable contributions")),
        _ => None,
    }
}

pub async fn handle_txf_export(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, String), (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());
    let start = format!("{}-01-01", year);
    let end = format!("{}-12-31", year);

    let txf = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now();
        let date_str = now.format("%m/%d/%Y").to_string();
        let mut out = format!("V042\nASyntaur Tax Module\nD{}\n^\n", date_str);

        // ── W-2 Income ──
        let mut w2_stmt = conn.prepare(
            "SELECT category, amount_cents, COALESCE(source, '') FROM tax_income WHERE user_id = ? AND tax_year = ?"
        ).map_err(|e| e.to_string())?;
        let w2_rows = w2_stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        }).map_err(|e| e.to_string())?;

        for row in w2_rows {
            let (cat, cents, source) = row.map_err(|e| e.to_string())?;
            let dollars = cents as f64 / 100.0;
            match cat.as_str() {
                c if c.contains("Wage") || c.contains("Salary") => {
                    out += &format!("TD\nN460\nC1\nL1\n${:.2}\n", dollars);
                    if !source.is_empty() { out += &format!("X{}\n", source); }
                    out += "^\n";
                }
                c if c.contains("Withholding") => {
                    out += &format!("TD\nN461\nC1\nL1\n${:.2}\n^\n", -dollars);
                }
                c if c.contains("Social Security") && c.contains("Tax") => {
                    out += &format!("TD\nN462\nC1\nL1\n${:.2}\n^\n", -dollars);
                }
                _ => {}
            }
        }

        // ── Schedule C Expenses ──
        let mut exp_stmt = conn.prepare(
            "SELECT c.name, SUM(e.amount_cents) FROM expenses e \
             JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.entity = 'business' AND e.expense_date >= ? AND e.expense_date <= ? \
             GROUP BY c.name ORDER BY c.name"
        ).map_err(|e| e.to_string())?;
        let exp_rows = exp_stmt.query_map(rusqlite::params![uid, &start, &end], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }).map_err(|e| e.to_string())?;

        for row in exp_rows {
            let (cat_name, total_cents) = row.map_err(|e| e.to_string())?;
            if let Some((refnum, _desc)) = category_to_txf_refnum(&cat_name) {
                let dollars = total_cents as f64 / 100.0;
                // Meals at 50%
                let amount = if cat_name == "Meals & Entertainment" { dollars * 0.5 } else { dollars };
                out += &format!("TS\nN{}\nC1\nL1\n${:.2}\n^\n", refnum, -amount);
            }
        }

        // ── Personal Deductions (Schedule A) ──
        let mut ded_stmt = conn.prepare(
            "SELECT c.name, SUM(e.amount_cents) FROM expenses e \
             JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.entity = 'personal' AND c.tax_deductible = 1 \
             AND e.expense_date >= ? AND e.expense_date <= ? \
             GROUP BY c.name ORDER BY c.name"
        ).map_err(|e| e.to_string())?;
        let ded_rows = ded_stmt.query_map(rusqlite::params![uid, &start, &end], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }).map_err(|e| e.to_string())?;

        for row in ded_rows {
            let (cat_name, total_cents) = row.map_err(|e| e.to_string())?;
            if let Some((refnum, _desc)) = category_to_txf_refnum(&cat_name) {
                let dollars = total_cents as f64 / 100.0;
                out += &format!("TS\nN{}\nC1\nL1\n${:.2}\n^\n", refnum, -dollars);
            }
        }

        // ── Capital Gains (Form 8949 / Schedule D) ──
        let mut inv_stmt = conn.prepare(
            "SELECT symbol, side, qty, price_cents, amount_cents, activity_type, transaction_date, external_id \
             FROM investment_transactions WHERE user_id = ? AND transaction_date >= ? AND transaction_date <= ? \
             ORDER BY transaction_date"
        ).map_err(|e| e.to_string())?;
        let inv_rows = inv_stmt.query_map(rusqlite::params![uid, &start, &end], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<f64>>(2)?, r.get::<_, Option<i64>>(3)?,
                r.get::<_, i64>(4)?, r.get::<_, String>(5)?,
                r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?,
            ))
        }).map_err(|e| e.to_string())?;

        for row in inv_rows {
            let (symbol, side, qty, price_cents, amount_cents, activity_type, date, _ext_id) = row.map_err(|e| e.to_string())?;
            if activity_type == "dividend" || side.as_deref() == Some("dividend") { continue; }
            let is_long = activity_type == "long_term";
            let refnum = if is_long { 323 } else { 321 }; // 321=short-term, 323=long-term
            let symbol_str = symbol.as_deref().unwrap_or("Unknown");
            let qty_val = qty.unwrap_or(0.0);
            let desc = format!("{:.4} {}", qty_val, symbol_str);
            let cost = price_cents.unwrap_or(0) as f64 * qty_val / 100.0;
            let proceeds = amount_cents as f64 / 100.0;
            // Format date MM/DD/YYYY
            let txf_date = if date.len() >= 10 {
                format!("{}/{}/{}", &date[5..7], &date[8..10], &date[..4])
            } else { date.clone() };
            out += &format!("TD\nN{}\nC1\nL1\nP{}\nDVARIOUS\nD{}\n${:.2}\n${:.2}\n^\n",
                refnum, desc, txf_date, cost.abs(), proceeds.abs());
        }

        Ok(out)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/x-txf".parse().unwrap());
    headers.insert("content-disposition", format!("attachment; filename=\"syntaur-tax-{}.txf\"", year).parse().unwrap());
    Ok((headers, txf))
}

// ── CSV Export with IRS Categories ─────────────────────────────────────────

pub async fn handle_csv_irs_export(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, String), (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());

    let csv = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let est = compute_tax_estimate(&conn, uid, year, "married_jointly")?;
        let d = |c: i64| cents_to_display(c);
        let start = format!("{}-01-01", year);
        let end = format!("{}-12-31", year);

        let mut lines: Vec<String> = vec![
            "Section,IRS Form/Line,Description,Amount".to_string(),
            format!("Income,1040 Line 1,Gross Income,{}", d(est.gross_income)),
        ];

        if est.se_income > 0 {
            lines.push(format!("Income,1040 / Sched 1,W-2 Wages,{}", d(est.w2_income)));
            lines.push(format!("Income,1040 / Sched 1,Self-Employment Income (1099),{}", d(est.se_income)));
        }

        // Schedule C expenses by category
        let mut exp_stmt = conn.prepare(
            "SELECT c.name, SUM(e.amount_cents) FROM expenses e \
             JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.entity = 'business' AND e.expense_date >= ? AND e.expense_date <= ? \
             GROUP BY c.name ORDER BY c.name"
        ).map_err(|e| e.to_string())?;
        let exp_rows = exp_stmt.query_map(rusqlite::params![uid, &start, &end], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }).map_err(|e| e.to_string())?;
        for row in exp_rows {
            let (cat, cents) = row.map_err(|e| e.to_string())?;
            let sched_c_line = match cat.as_str() {
                "Advertising & Marketing" => "Sched C Line 8",
                "Vehicle & Mileage" => "Sched C Line 9",
                "Professional Services" => "Sched C Line 17",
                "Office Supplies" => "Sched C Line 18",
                "Insurance" => "Sched C Line 15",
                "Meals & Entertainment" => "Sched C Line 24b",
                "Travel" => "Sched C Line 24a",
                "Rent & Utilities" => "Sched C Line 25",
                "Hardware & Supplies" | "Lumber & Raw Materials" | "Tools - Consumables" | "Safety Gear" => "Sched C Line 22",
                "Equipment & Tools" => "Sched C Line 13",
                "Software & Subscriptions" => "Sched C Line 27",
                _ => "Sched C Line 27",
            };
            lines.push(format!("Business Expense,{},{},{}", sched_c_line, cat.replace(',', ";"), d(cents)));
        }

        // Summary
        lines.push(format!("Calculation,Schedule C Line 28,Total Business Deductions,{}", d(est.biz_deductions)));
        if est.meals_adjustment > 0 {
            lines.push(format!("Calculation,Sched C Line 24b,Meals 50% Disallowed,{}", d(est.meals_adjustment)));
        }
        if est.se_tax > 0 {
            lines.push(format!("Calculation,Schedule SE,Self-Employment Tax,{}", d(est.se_tax)));
            lines.push(format!("Calculation,Schedule 1 Line 15,Deductible Half SE Tax,{}", d(est.half_se_tax)));
        }
        if est.se_health_deduction > 0 {
            lines.push(format!("Calculation,Schedule 1 Line 17,SE Health Insurance Deduction,{}", d(est.se_health_deduction)));
        }
        lines.push(format!("Calculation,1040 Line 11,Adjusted Gross Income,{}", d(est.agi)));
        lines.push(format!("Calculation,1040 Line 12,{} Deduction,{}", est.deduction_type, d(est.deduction_used)));
        if est.qbi_deduction > 0 {
            lines.push(format!("Calculation,1040 Line 13,QBI Deduction (Section 199A),{}", d(est.qbi_deduction)));
        }
        lines.push(format!("Calculation,1040 Line 15,Taxable Income,{}", d(est.taxable_income)));
        lines.push(format!("Tax,1040 Line 16,Federal Income Tax,{}", d(est.ordinary_tax)));
        if est.ltcg_tax > 0 {
            lines.push(format!("Tax,Schedule D,Long-Term Capital Gains Tax,{}", d(est.ltcg_tax)));
        }
        if est.se_tax > 0 {
            lines.push(format!("Tax,Schedule SE,Self-Employment Tax,{}", d(est.se_tax)));
        }
        lines.push(format!("Tax,,Total Tax,{}", d(est.total_tax)));
        if est.w2_withheld > 0 {
            lines.push(format!("Payment,W-2 Box 2,Federal Withholding,{}", d(est.w2_withheld)));
        }
        if est.fed_paid > 0 {
            lines.push(format!("Payment,1040-ES,Estimated Tax Payments,{}", d(est.fed_paid)));
        }
        if est.child_credit > 0 {
            lines.push(format!("Credit,1040 Line 19,Child Tax Credit,{}", d(est.child_credit)));
        }
        lines.push(format!("Result,,{},{}", if est.owed > 0 { "Tax Due" } else { "Refund" }, d(est.owed.abs())));
        lines.push(format!("Rate,,Effective Tax Rate,{:.1}%", est.effective_rate));

        Ok(lines.join("\n"))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/csv".parse().unwrap());
    headers.insert("content-disposition", format!("attachment; filename=\"syntaur-tax-summary-{}.csv\"", year).parse().unwrap());
    Ok((headers, csv))
}

// ── Form 4868 Extension Generator ───────────────────────────────────────────

pub async fn handle_extension(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, String), (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());
    let ext_payment: i64 = params.get("payment").and_then(|p| p.parse().ok()).unwrap_or(0);

    let text = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let est = compute_tax_estimate(&conn, uid, year, "married_jointly")?;
        let d = |c: i64| cents_to_display(c);

        // Get user name from principal or config
        let user_name = conn.query_row(
            "SELECT COALESCE(display_name, username, 'Taxpayer') FROM users WHERE id = ?",
            rusqlite::params![uid], |r| r.get::<_, String>(0),
        ).unwrap_or_else(|_| "Taxpayer".to_string());

        let balance_due = std::cmp::max(est.total_tax - est.total_payments, 0);

        let mut out = String::new();
        out += "═══════════════════════════════════════════════════════════\n";
        out += "     IRS FORM 4868 — Application for Automatic Extension\n";
        out += "         of Time to File U.S. Individual Income Tax Return\n";
        out += &format!("                      Tax Year {}\n", year);
        out += "═══════════════════════════════════════════════════════════\n\n";

        out += &format!("Name: {}\n", user_name);
        out += &format!("Tax Year: {}\n\n", year);

        out += "─── Part I: Identification ─────────────────────────────\n";
        out += "  Line 1: Your name(s) and address: [fill in]\n";
        out += "  Line 2: SSN: [fill in]\n";
        out += "  Line 3: Filing status: [check appropriate box]\n\n";

        out += "─── Part II: Individual Income Tax ─────────────────────\n";
        out += &format!("  Line 4:  Estimate of total tax liability:    {}\n", d(est.total_tax));
        out += &format!("  Line 5:  Total payments already made:        {}\n", d(est.total_payments));
        out += &format!("  Line 6:  Balance due (line 4 - line 5):      {}\n", d(balance_due));
        out += &format!("  Line 7:  Amount you're paying:               {}\n\n", d(ext_payment));

        out += "─── Tax Estimate Breakdown ─────────────────────────────\n";
        out += &format!("  Gross Income:                  {}\n", d(est.gross_income));
        out += &format!("  Adjusted Gross Income:         {}\n", d(est.agi));
        out += &format!("  Federal Income Tax:            {}\n", d(est.ordinary_tax));
        if est.se_tax > 0 { out += &format!("  Self-Employment Tax:           {}\n", d(est.se_tax)); }
        if est.ltcg_tax > 0 { out += &format!("  Capital Gains Tax:             {}\n", d(est.ltcg_tax)); }
        out += &format!("  Total Tax:                     {}\n", d(est.total_tax));
        out += &format!("  W-2 Withholding:              -{}\n", d(est.w2_withheld));
        if est.fed_paid > 0 { out += &format!("  Estimated Payments:           -{}\n", d(est.fed_paid)); }
        if est.child_credit > 0 { out += &format!("  Child Tax Credit:             -{}\n", d(est.child_credit)); }
        out += &format!("  Total Payments:               -{}\n", d(est.total_payments));
        out += &format!("  ─────────────────────────────────\n");
        out += &format!("  {}\n\n", if balance_due > 0 { format!("Balance Due: {}", d(balance_due)) } else { format!("Refund Expected: {}", d(-est.owed)) });

        out += "═══════════════════════════════════════════════════════════\n";
        out += "HOW TO FILE THIS EXTENSION:\n\n";
        out += "Option 1 (Fastest): IRS Free File\n";
        out += "  → Go to irs.gov/freefile\n";
        out += "  → Choose any partner → File Form 4868 electronically\n";
        out += "  → Free for all taxpayers, no income limit\n\n";
        out += "Option 2 (Auto-extension): IRS Direct Pay\n";
        out += "  → Go to irs.gov/payments → Select 'Extension' as reason\n";
        out += "  → Making a payment automatically files your extension\n\n";
        out += "Option 3: Print & mail this form with your payment\n";
        out += "  → Mail to your IRS service center by April 15\n";
        out += "  → Download official Form 4868 at irs.gov/pub/irs-pdf/f4868.pdf\n\n";
        out += "IMPORTANT DATES:\n";
        out += &format!("  Filing deadline (with extension): October 15, {}\n", year + 1);
        out += &format!("  Payment deadline (NOT extended):  April 15, {}\n", year + 1);
        out += "  Interest/penalties accrue on unpaid tax after April 15.\n";
        out += "═══════════════════════════════════════════════════════════\n";
        out += &format!("\nGenerated by Syntaur Tax Module on {}\n", chrono::Utc::now().format("%Y-%m-%d"));

        Ok(out)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/plain; charset=utf-8".parse().unwrap());
    headers.insert("content-disposition", format!("attachment; filename=\"form-4868-{}.txt\"", year).parse().unwrap());
    Ok((headers, text))
}

// ── Extension Filing Workflow (stateful) ────────────────────────────────────

pub async fn handle_extension_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT id, tax_year, total_tax_cents, total_paid_cents, balance_due_cents, payment_cents, \
             filing_method, status, confirmation_id, filed_at, confirmed_at, created_at FROM tax_extensions \
             WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year],
            |r| Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "tax_year": r.get::<_, i64>(1)?,
                "total_tax": cents_to_display(r.get::<_, i64>(2)?),
                "total_tax_cents": r.get::<_, i64>(2)?,
                "total_paid": cents_to_display(r.get::<_, i64>(3)?),
                "total_paid_cents": r.get::<_, i64>(3)?,
                "balance_due": cents_to_display(r.get::<_, i64>(4)?),
                "balance_due_cents": r.get::<_, i64>(4)?,
                "payment_cents": r.get::<_, i64>(5)?,
                "payment": cents_to_display(r.get::<_, i64>(5)?),
                "filing_method": r.get::<_, Option<String>>(6)?,
                "status": r.get::<_, String>(7)?,
                "confirmation_id": r.get::<_, Option<String>>(8)?,
                "filed_at": r.get::<_, Option<i64>>(9)?,
                "confirmed_at": r.get::<_, Option<i64>>(10)?,
                "created_at": r.get::<_, i64>(11)?,
            })),
        ) {
            Ok(ext) => Ok(serde_json::json!({ "extension": ext })),
            Err(_) => {
                // No extension yet — return estimate for pre-fill
                let est = compute_tax_estimate(&conn, uid, year, "married_jointly")?;
                let balance = std::cmp::max(est.total_tax - est.total_payments, 0);
                Ok(serde_json::json!({
                    "extension": null,
                    "estimate": {
                        "total_tax_cents": est.total_tax,
                        "total_tax": cents_to_display(est.total_tax),
                        "total_paid_cents": est.total_payments,
                        "total_paid": cents_to_display(est.total_payments),
                        "balance_due_cents": balance,
                        "balance_due": cents_to_display(balance),
                        "filing_status": "married_jointly",
                    }
                }))
            }
        }
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct ExtensionCreateRequest {
    pub token: String,
    pub year: Option<i64>,
    pub total_tax_cents: Option<i64>,
    pub total_paid_cents: Option<i64>,
    pub payment_cents: Option<i64>,
}

pub async fn handle_extension_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExtensionCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());
    let override_tax = req.total_tax_cents;
    let override_paid = req.total_paid_cents;
    let payment = req.payment_cents.unwrap_or(0);
    let now = chrono::Utc::now().timestamp();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let est = compute_tax_estimate(&conn, uid, year, "married_jointly")?;

        let total_tax = override_tax.unwrap_or(est.total_tax);
        let total_paid = override_paid.unwrap_or(est.total_payments);
        let balance_due = std::cmp::max(total_tax - total_paid, 0);

        // Generate form text
        let user_name = conn.query_row(
            "SELECT COALESCE(display_name, username, 'Taxpayer') FROM users WHERE id = ?",
            rusqlite::params![uid], |r| r.get::<_, String>(0),
        ).unwrap_or_else(|_| "Taxpayer".to_string());

        let d = |c: i64| cents_to_display(c);
        let form_text = format!(
            "FORM 4868 — Tax Year {}\nName: {}\nEstimated Tax: {}\nTotal Payments: {}\nBalance Due: {}\nPayment with Extension: {}\n\nGenerated by Syntaur on {}",
            year, user_name, d(total_tax), d(total_paid), d(balance_due), d(payment),
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
        );

        conn.execute(
            "INSERT INTO tax_extensions (user_id, tax_year, total_tax_cents, total_paid_cents, balance_due_cents, \
             payment_cents, status, form_text, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 'draft', ?, ?, ?) \
             ON CONFLICT(user_id, tax_year) DO UPDATE SET \
             total_tax_cents=excluded.total_tax_cents, total_paid_cents=excluded.total_paid_cents, \
             balance_due_cents=excluded.balance_due_cents, payment_cents=excluded.payment_cents, \
             form_text=excluded.form_text, updated_at=excluded.updated_at, status='draft'",
            rusqlite::params![uid, year, total_tax, total_paid, balance_due, payment, &form_text, now, now],
        ).map_err(|e| e.to_string())?;

        let id = conn.query_row(
            "SELECT id FROM tax_extensions WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get::<_, i64>(0),
        ).map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "id": id,
            "status": "draft",
            "total_tax": d(total_tax),
            "total_paid": d(total_paid),
            "balance_due": d(balance_due),
            "payment": d(payment),
            "balance_due_cents": balance_due,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "extension": result })))
}

#[derive(Deserialize)]
pub struct ExtensionFileRequest {
    pub token: String,
    pub method: String, // "direct_pay" | "free_file" | "mail"
}

pub async fn handle_extension_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(ext_id): axum::extract::Path<i64>,
    Json(req): Json<ExtensionFileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let method = req.method.clone();
    let method_ret = method.clone();
    let now = chrono::Utc::now().timestamp();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE tax_extensions SET status = 'filed', filing_method = ?, filed_at = ?, updated_at = ? \
             WHERE id = ? AND user_id = ?",
            rusqlite::params![&method, now, now, ext_id, uid],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "status": "filed", "method": method_ret })))
}

#[derive(Deserialize)]
pub struct ExtensionConfirmRequest {
    pub token: String,
    pub confirmation_id: String,
}

pub async fn handle_extension_confirm(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(ext_id): axum::extract::Path<i64>,
    Json(req): Json<ExtensionConfirmRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let confirm_id = req.confirmation_id.clone();
    let now = chrono::Utc::now().timestamp();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Get the form text and year
        let (form_text, year): (String, i64) = conn.query_row(
            "SELECT form_text, tax_year FROM tax_extensions WHERE id = ? AND user_id = ?",
            rusqlite::params![ext_id, uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|_| "Extension not found".to_string())?;

        // Save form text as a tax document
        let receipts_dir = receipts_dir();
        let uuid = uuid::Uuid::new_v4().to_string();
        let file_path = receipts_dir.join(format!("form-4868-{}.txt", uuid));
        std::fs::write(&file_path, &form_text).map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT INTO tax_documents (user_id, doc_type, tax_year, issuer, extracted_fields, image_path, status, created_at) \
             VALUES (?, 'extension_4868', ?, 'IRS', ?, ?, 'scanned', ?)",
            rusqlite::params![
                uid, year,
                serde_json::json!({ "confirmation_id": &confirm_id, "filed_at": now }).to_string(),
                file_path.to_string_lossy().to_string(),
                now
            ],
        ).map_err(|e| e.to_string())?;
        let doc_id = conn.last_insert_rowid();

        // Update extension status
        conn.execute(
            "UPDATE tax_extensions SET status = 'confirmed', confirmation_id = ?, confirmed_at = ?, \
             document_id = ?, updated_at = ? WHERE id = ? AND user_id = ?",
            rusqlite::params![&confirm_id, now, doc_id, now, ext_id, uid],
        ).map_err(|e| e.to_string())?;

        info!("[tax] Extension confirmed for user {} year {} (confirmation: {})", uid, year, confirm_id);

        Ok(serde_json::json!({
            "status": "confirmed",
            "confirmation_id": confirm_id,
            "document_id": doc_id,
            "new_deadline": format!("October 15, {}", year + 1),
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "extension": result })))
}

// ── Item 10: Smart Document Routing ─────────────────────────────────────────

/// Unified upload: accept any file, auto-classify it, route to the right handler.
/// Returns: document type + extracted data. Replaces the need for separate
/// receipt vs document upload buttons.
pub async fn handle_smart_upload(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ReceiptUploadQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &params.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    if body.is_empty() { return Err((StatusCode::BAD_REQUEST, "No file data".to_string())); }
    if body.len() > 10 * 1024 * 1024 { return Err((StatusCode::BAD_REQUEST, "File too large (max 10MB)".to_string())); }

    let content_type = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream");
    let ext = if content_type.contains("png") { "png" } else if content_type.contains("pdf") { "pdf" } else { "jpg" };

    let filename = format!("upload-{}.{}", uuid::Uuid::new_v4(), ext);
    let path = receipts_dir().join(&filename);
    std::fs::write(&path, &body).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Save: {}", e)))?;

    let uid = principal.user_id();
    info!("[tax] Smart upload: {} ({} bytes) from user {}", filename, body.len(), uid);

    // Classification is always free — file is saved regardless of module access
    let path_str = path.to_string_lossy().to_string();
    let state2 = Arc::clone(&state);
    let classify_result = classify_document_type(&state2, &path_str).await;

    let doc_class = match classify_result {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[tax] Smart classification failed: {} — falling back to receipt", e);
            "receipt".to_string()
        }
    };

    info!("[tax] Smart upload classified as: {}", doc_class);

    // Check module access — scanning/extraction requires the tax module,
    // but classification + file save is always free
    let has_module = {
        let db_check = state.db_path.clone();
        let uid_check = uid;
        tokio::task::spawn_blocking(move || -> bool {
            if let Ok(conn) = rusqlite::Connection::open(&db_check) {
                check_module_access(&conn, uid_check, "tax").map(|a| a.granted).unwrap_or(true)
            } else { true }
        }).await.unwrap_or(true)
    };

    if !has_module {
        // File saved but NOT scanned — user sees the classification and can
        // start a trial to unlock scanning
        return Ok(Json(serde_json::json!({
            "routed_to": "saved",
            "doc_type": doc_class,
            "status": "saved_unscanned",
            "file_path": path_str,
            "module_locked": true,
            "message": format!("This looks like a {}. File saved — start a free trial to scan and track it.", doc_class),
        })));
    }

    match doc_class.as_str() {
        "receipt" | "invoice" => {
            // Route to receipt handler
            let db = state.db_path.clone();
            let now = chrono::Utc::now().timestamp();
            let ps = path.to_string_lossy().to_string();
            let receipt_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO receipts (user_id, image_path, status, created_at) VALUES (?, ?, 'pending', ?)",
                    rusqlite::params![uid, &ps, now],
                ).map_err(|e| e.to_string())?;
                Ok(conn.last_insert_rowid())
            }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

            let s3 = Arc::clone(&state);
            let rid = receipt_id;
            tokio::spawn(async move {
                if let Err(e) = scan_receipt_vision(&s3, rid).await {
                    error!("[tax] Vision scan failed for receipt #{}: {}", rid, e);
                }
            });

            Ok(Json(serde_json::json!({
                "routed_to": "receipt",
                "id": receipt_id,
                "doc_type": doc_class,
                "status": "pending",
                "message": format!("Identified as {}. Scanning with AI...", doc_class),
            })))
        }
        "bank_statement" | "credit_card_statement" => {
            // Route to statement handler — create tax_document + extract transactions
            let db = state.db_path.clone();
            let now = chrono::Utc::now().timestamp();
            let ps = path.to_string_lossy().to_string();
            let dc = doc_class.clone();
            let doc_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO tax_documents (user_id, doc_type, image_path, status, created_at) VALUES (?, ?, ?, 'pending', ?)",
                    rusqlite::params![uid, &dc, &ps, now],
                ).map_err(|e| e.to_string())?;
                Ok(conn.last_insert_rowid())
            }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

            let s3 = Arc::clone(&state);
            let did = doc_id;
            tokio::spawn(async move {
                if let Err(e) = classify_and_extract(&s3, did).await {
                    error!("[tax] Doc classify failed #{}: {}", did, e);
                }
                if let Err(e) = extract_statement_transactions(&s3, did).await {
                    error!("[tax] Statement extraction failed #{}: {}", did, e);
                }
            });

            Ok(Json(serde_json::json!({
                "routed_to": "statement",
                "id": doc_id,
                "doc_type": doc_class,
                "status": "pending",
                "message": "Identified as bank/credit card statement. Extracting transactions...",
            })))
        }
        _ => {
            // Route to tax document handler (W-2, 1099, 1098, etc.)
            let db = state.db_path.clone();
            let now = chrono::Utc::now().timestamp();
            let ps = path.to_string_lossy().to_string();
            let dc = doc_class.clone();
            let doc_id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
                let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO tax_documents (user_id, doc_type, image_path, status, created_at) VALUES (?, ?, ?, 'pending', ?)",
                    rusqlite::params![uid, &dc, &ps, now],
                ).map_err(|e| e.to_string())?;
                Ok(conn.last_insert_rowid())
            }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

            let s3 = Arc::clone(&state);
            let did = doc_id;
            tokio::spawn(async move {
                if let Err(e) = classify_and_extract(&s3, did).await {
                    error!("[tax] Doc classify failed #{}: {}", did, e);
                }
            });

            Ok(Json(serde_json::json!({
                "routed_to": "document",
                "id": doc_id,
                "doc_type": doc_class,
                "status": "pending",
                "message": format!("Identified as {}. Extracting fields...", doc_class),
            })))
        }
    }
}

/// Quick classification: what kind of document is this?
async fn classify_document_type(state: &AppState, image_path: &str) -> Result<String, String> {
    use base64::Engine;

    let image_data = if image_path.ends_with(".pdf") {
        match convert_pdf_to_png(image_path) {
            Ok(png) => (base64::engine::general_purpose::STANDARD.encode(&png), "image/png"),
            Err(_) => {
                let raw = std::fs::read(image_path).map_err(|e| e.to_string())?;
                (base64::engine::general_purpose::STANDARD.encode(&raw), "application/pdf")
            }
        }
    } else {
        let raw = std::fs::read(image_path).map_err(|e| e.to_string())?;
        let ext = image_path.rsplit('.').next().unwrap_or("jpg");
        let m = match ext { "png" => "image/png", _ => "image/jpeg" };
        (base64::engine::general_purpose::STANDARD.encode(&raw), m)
    };

    let config = &state.config;
    let provider = config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| config.models.providers.iter().next());
    let (_, provider_config) = provider.ok_or("No LLM provider")?;
    let model = if provider_config.base_url.contains("openrouter") {
        "nvidia/nemotron-nano-12b-v2-vl:free"
    } else if provider_config.base_url.contains("anthropic") { "claude-sonnet-4-6" }
    else { "gpt-4o-mini" };
    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "What type of document is this? Respond with ONLY one word from this list: receipt, invoice, w2, 1099_int, 1099_div, 1099_b, 1099_misc, 1099_nec, 1095_c, mortgage_statement, property_tax_statement, bank_statement, credit_card_statement, insurance_policy, settlement_statement, other"},
            {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", image_data.1, image_data.0)}}
        ]}],
        "max_tokens": 50,
        "temperature": 0.0
    });

    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", provider_config.api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send().await.map_err(|e| format!("LLM: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("LLM HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let raw = body["choices"][0]["message"]["content"].as_str().unwrap_or("other");
    let cleaned = raw.trim().to_lowercase().replace(['"', '.', ','], "");
    Ok(cleaned)
}

// ── Item 11: Statement Transaction Extraction ───────────────────────────────

/// Extract individual transactions from a bank/credit card statement.
async fn extract_statement_transactions(state: &AppState, doc_id: i64) -> Result<(), String> {
    let db = state.db_path.clone();

    let (image_path, uid): (String, i64) = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, i64), String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            let path: String = conn.query_row("SELECT image_path FROM tax_documents WHERE id = ?", rusqlite::params![doc_id], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let uid: i64 = conn.query_row("SELECT user_id FROM tax_documents WHERE id = ?", rusqlite::params![doc_id], |r| r.get(0))
                .unwrap_or(0);
            Ok((path, uid))
        }).await.map_err(|e| e.to_string())??
    };

    // Convert all pages to images for comprehensive extraction
    use base64::Engine;
    let image_parts: Vec<(String, &str)> = if image_path.ends_with(".pdf") {
        match convert_pdf_to_pngs(&image_path) {
            Ok(pages) => pages.into_iter().map(|p| (base64::engine::general_purpose::STANDARD.encode(&p), "image/png")).collect(),
            Err(e) => {
                let data = std::fs::read(&image_path).map_err(|e| e.to_string())?;
                log::warn!("[tax] PDF conversion failed for statement ({}), sending raw", e);
                vec![(base64::engine::general_purpose::STANDARD.encode(&data), "application/pdf")]
            }
        }
    } else {
        let data = std::fs::read(&image_path).map_err(|e| e.to_string())?;
        let ext = image_path.rsplit('.').next().unwrap_or("jpg");
        let m = match ext { "png" => "image/png", _ => "image/jpeg" };
        vec![(base64::engine::general_purpose::STANDARD.encode(&data), m)]
    };

    let config = &state.config;
    let provider = config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| config.models.providers.iter().next());
    let (_, provider_config) = provider.ok_or("No LLM provider")?;
    // Use a model with large context for multi-page statements
    let model = if provider_config.base_url.contains("openrouter") { "google/gemini-2.0-flash-001" }
        else if provider_config.base_url.contains("anthropic") { "claude-sonnet-4-6" }
        else { "gpt-4o-mini" };
    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    let prompt = r#"Extract ALL individual transactions from this bank or credit card statement. For each transaction, provide:
- date: transaction date (YYYY-MM-DD)
- description: merchant/payee name as shown
- amount: dollar amount (positive = charge/debit, negative = credit/payment)
- vendor: cleaned-up vendor name (e.g. "AMZN Mktp US" → "Amazon")
- category: best guess from: Utilities, Groceries, Dining, Entertainment, Insurance, Medical, Vehicle, Home Improvement, Software & Subscriptions, Professional Services, Education, Travel, Fuel, Mortgage, Other
- insurance_type: if this looks like an insurance payment, specify: "auto", "home", "health", "life", or null

IMPORTANT: Extract EVERY transaction on every page. Include payments, credits, and charges.

For insurance payments, use these heuristics:
- Auto insurance: typically $100-300/month, vendors like Safeco, GEICO, State Farm, Progressive, Allstate
- Home insurance: typically $100-200/month, often escrowed in mortgage (may not appear on statements)
- Health insurance: varies widely, vendors like Blue Cross, Aetna, Kaiser, UnitedHealth
- Same vendor can have different insurance types (e.g., Safeco auto AND Safeco home)

Respond with JSON array: [{"date":"2025-08-15","description":"SAFECO INS","amount":"165.41","vendor":"Safeco","category":"Insurance","insurance_type":"auto"}, ...]"#;

    let mut content_parts: Vec<serde_json::Value> = vec![serde_json::json!({"type": "text", "text": prompt})];
    for (b64, mime) in &image_parts {
        content_parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime, b64)}}));
    }

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content_parts}],
        "max_tokens": 4000,
        "temperature": 0.1
    });

    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", provider_config.api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(120))
        .send().await.map_err(|e| format!("LLM: {}", e))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("LLM HTTP {}: {}", s, &body[..body.len().min(200)]));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("[]");
    let cleaned = content.trim().trim_start_matches("```json").trim_end_matches("```").trim();
    let transactions: Vec<serde_json::Value> = serde_json::from_str(cleaned)
        .map_err(|e| format!("Parse transactions: {} — raw: {}", e, &cleaned[..cleaned.len().min(200)]))?;

    info!("[tax] Extracted {} transactions from statement #{}", transactions.len(), doc_id);

    // Insert transactions into database
    let db2 = db.clone();
    let txn_count = transactions.len();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp();

        // Load insurance classifications for disambiguation
        let mut insurance_map: std::collections::HashMap<String, Vec<(i64, String, f64)>> = std::collections::HashMap::new();
        if let Ok(mut stmt) = conn.prepare("SELECT vendor, amount_cents, insurance_type, confidence FROM insurance_classifications WHERE user_id = ?") {
            if let Ok(rows) = stmt.query_map(rusqlite::params![uid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get::<_, String>(2)?, r.get::<_, f64>(3)?))
            }) {
                for r in rows.flatten() {
                    insurance_map.entry(r.0.to_lowercase()).or_default().push((r.1, r.2, r.3));
                }
            }
        }

        for txn in &transactions {
            let date = txn["date"].as_str().unwrap_or("").to_string();
            let desc = txn["description"].as_str().unwrap_or("").to_string();
            let amount_str = txn["amount"].as_str().unwrap_or("0");
            let amount_cents = parse_cents(amount_str).unwrap_or(0);
            let vendor = txn["vendor"].as_str().unwrap_or(&desc).to_string();
            let category_name = txn["category"].as_str().unwrap_or("Other").to_string();
            let insurance_type = txn["insurance_type"].as_str().map(String::from);

            // Resolve insurance type using disambiguation rules
            let final_insurance_type = if category_name == "Insurance" || insurance_type.is_some() {
                let vendor_lower = vendor.to_lowercase();
                // Check if we have a learned classification
                if let Some(classes) = insurance_map.get(&vendor_lower) {
                    // Find best match by amount similarity
                    let best = classes.iter()
                        .min_by_key(|(amt, _, _)| (amount_cents - amt).unsigned_abs());
                    best.map(|(_, t, _)| t.clone()).or(insurance_type)
                } else {
                    // Heuristic: classify by amount range
                    let abs = amount_cents.unsigned_abs() as i64;
                    if abs > 0 {
                        Some(classify_insurance_by_amount(abs, &vendor))
                    } else {
                        insurance_type
                    }
                }
            } else {
                None
            };

            let cat_id: Option<i64> = conn.query_row(
                "SELECT id FROM expense_categories WHERE name = ?",
                rusqlite::params![&category_name], |r| r.get(0)
            ).ok();

            let is_deductible = cat_id.map(|id| {
                conn.query_row("SELECT tax_deductible FROM expense_categories WHERE id = ?",
                    rusqlite::params![id], |r| r.get::<_, i64>(0)).unwrap_or(0)
            }).unwrap_or(0);

            conn.execute(
                "INSERT INTO statement_transactions (user_id, document_id, transaction_date, description, amount_cents, category_id, vendor, insurance_type, is_deductible, status, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'extracted', ?)",
                rusqlite::params![uid, doc_id, &date, &desc, amount_cents, cat_id, &vendor, &final_insurance_type, is_deductible, now],
            ).map_err(|e| e.to_string())?;
        }

        Ok(())
    }).await.map_err(|e| e.to_string())??;

    info!("[tax] Stored {} transactions from statement #{}", txn_count, doc_id);
    Ok(())
}

/// Heuristic insurance classification by amount (Item 14)
fn classify_insurance_by_amount(amount_cents: i64, vendor: &str) -> String {
    let monthly = amount_cents; // assume single payment
    let vendor_lower = vendor.to_lowercase();

    // Health insurance keywords
    if vendor_lower.contains("blue cross") || vendor_lower.contains("aetna") ||
       vendor_lower.contains("kaiser") || vendor_lower.contains("united health") ||
       vendor_lower.contains("cigna") || vendor_lower.contains("humana") ||
       vendor_lower.contains("anthem") || vendor_lower.contains("molina") {
        return "health".to_string();
    }

    // Life insurance keywords
    if vendor_lower.contains("life") || vendor_lower.contains("metlife") ||
       vendor_lower.contains("prudential") || vendor_lower.contains("northwestern mutual") {
        return "life".to_string();
    }

    // Amount-based heuristics (in cents)
    // Auto: typically $80-$350/month
    // Home: typically $80-$300/month (often escrowed, so won't appear on statements)
    // Health: typically $200-$2000/month
    if monthly >= 20_000 && monthly <= 200_000 {
        // Could be auto or home. If escrowed, home won't show on credit card.
        // Auto is more common as a standalone payment.
        "auto".to_string()
    } else if monthly > 200_000 {
        "health".to_string()
    } else {
        "auto".to_string() // default for small insurance payments
    }
}

// ── Statement Transaction List Endpoint ─────────────────────────────────────

pub async fn handle_statement_transactions(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let doc_id_filter = params.get("document_id").and_then(|v| v.parse::<i64>().ok());
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let mut sql = "SELECT t.id, t.document_id, t.transaction_date, t.description, t.amount_cents, \
                        t.vendor, t.insurance_type, t.is_deductible, t.status, c.name \
                        FROM statement_transactions t LEFT JOIN expense_categories c ON t.category_id = c.id \
                        WHERE t.user_id = ?".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(uid)];

        if let Some(did) = doc_id_filter {
            sql.push_str(" AND t.document_id = ?");
            params_vec.push(Box::new(did));
        }
        if let Some(ref s) = start {
            sql.push_str(" AND t.transaction_date >= ?");
            params_vec.push(Box::new(s.clone()));
        }
        if let Some(ref e) = end {
            sql.push_str(" AND t.transaction_date <= ?");
            params_vec.push(Box::new(e.clone()));
        }
        sql.push_str(" ORDER BY t.transaction_date DESC LIMIT 500");

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let rows: Vec<serde_json::Value> = stmt.query_map(refs.as_slice(), |r| {
            let cents: i64 = r.get(4)?;
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "document_id": r.get::<_, Option<i64>>(1)?,
                "transaction_date": r.get::<_, String>(2)?,
                "description": r.get::<_, String>(3)?,
                "amount_cents": cents,
                "amount_display": cents_to_display(cents),
                "vendor": r.get::<_, Option<String>>(5)?,
                "insurance_type": r.get::<_, Option<String>>(6)?,
                "is_deductible": r.get::<_, i64>(7)? != 0,
                "status": r.get::<_, String>(8)?,
                "category": r.get::<_, Option<String>>(9)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let total: i64 = rows.iter().map(|r| r["amount_cents"].as_i64().unwrap_or(0)).sum();

        Ok(serde_json::json!({
            "transactions": rows,
            "count": rows.len(),
            "total_cents": total,
            "total_display": cents_to_display(total),
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

// ── Item 12: Property Profile ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PropertyProfileRequest {
    pub token: String,
    pub address: Option<String>,
    pub total_sqft: Option<i64>,
    pub workshop_sqft: Option<i64>,
    pub purchase_price: Option<String>,
    pub purchase_date: Option<String>,
    pub building_value: Option<String>,
    pub land_value: Option<String>,
    pub land_ratio: Option<f64>,
    pub assessor_total: Option<String>,
    pub assessor_land: Option<String>,
    pub annual_property_tax: Option<String>,
    pub annual_insurance: Option<String>,
    pub mortgage_lender: Option<String>,
    pub mortgage_interest: Option<String>,
    pub mortgage_principal: Option<String>,
    pub notes: Option<String>,
}

pub async fn handle_property_profile_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Check if table exists
        let has_table: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='property_profiles'",
            [], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0;
        if !has_table {
            return Ok(serde_json::json!({ "profiles": [] }));
        }

        let mut stmt = conn.prepare(
            "SELECT id, address, total_sqft, workshop_sqft, purchase_price_cents, purchase_date, \
             building_value_cents, land_value_cents, land_ratio, assessor_total_cents, assessor_land_cents, \
             annual_property_tax_cents, annual_insurance_cents, mortgage_lender, mortgage_interest_cents, \
             mortgage_principal_cents, depreciation_basis_cents, depreciation_annual_cents, notes \
             FROM property_profiles WHERE user_id = ? ORDER BY id"
        ).map_err(|e| e.to_string())?;

        let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "address": r.get::<_, String>(1)?,
                "total_sqft": r.get::<_, Option<i64>>(2)?,
                "workshop_sqft": r.get::<_, Option<i64>>(3)?,
                "purchase_price_cents": r.get::<_, Option<i64>>(4)?,
                "purchase_price_display": r.get::<_, Option<i64>>(4)?.map(cents_to_display),
                "purchase_date": r.get::<_, Option<String>>(5)?,
                "building_value_cents": r.get::<_, Option<i64>>(6)?,
                "building_value_display": r.get::<_, Option<i64>>(6)?.map(cents_to_display),
                "land_value_cents": r.get::<_, Option<i64>>(7)?,
                "land_value_display": r.get::<_, Option<i64>>(7)?.map(cents_to_display),
                "land_ratio": r.get::<_, Option<f64>>(8)?,
                "assessor_total_cents": r.get::<_, Option<i64>>(9)?,
                "assessor_total_display": r.get::<_, Option<i64>>(9)?.map(cents_to_display),
                "assessor_land_cents": r.get::<_, Option<i64>>(10)?,
                "annual_property_tax_cents": r.get::<_, Option<i64>>(11)?,
                "annual_property_tax_display": r.get::<_, Option<i64>>(11)?.map(cents_to_display),
                "annual_insurance_cents": r.get::<_, Option<i64>>(12)?,
                "annual_insurance_display": r.get::<_, Option<i64>>(12)?.map(cents_to_display),
                "mortgage_lender": r.get::<_, Option<String>>(13)?,
                "mortgage_interest_cents": r.get::<_, Option<i64>>(14)?,
                "mortgage_interest_display": r.get::<_, Option<i64>>(14)?.map(cents_to_display),
                "mortgage_principal_cents": r.get::<_, Option<i64>>(15)?,
                "depreciation_basis_cents": r.get::<_, Option<i64>>(16)?,
                "depreciation_basis_display": r.get::<_, Option<i64>>(16)?.map(cents_to_display),
                "depreciation_annual_cents": r.get::<_, Option<i64>>(17)?,
                "depreciation_annual_display": r.get::<_, Option<i64>>(17)?.map(cents_to_display),
                "notes": r.get::<_, Option<String>>(18)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        Ok(serde_json::json!({ "profiles": rows }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

pub async fn handle_property_profile_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PropertyProfileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    let address = req.address.clone().unwrap_or_default();
    if address.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Address is required".to_string()));
    }

    let purchase_price = req.purchase_price.as_deref().and_then(parse_cents);
    let building_value = req.building_value.as_deref().and_then(parse_cents);
    let land_value = req.land_value.as_deref().and_then(parse_cents);
    let assessor_total = req.assessor_total.as_deref().and_then(parse_cents);
    let assessor_land = req.assessor_land.as_deref().and_then(parse_cents);
    let annual_tax = req.annual_property_tax.as_deref().and_then(parse_cents);
    let annual_ins = req.annual_insurance.as_deref().and_then(parse_cents);
    let mortgage_int = req.mortgage_interest.as_deref().and_then(parse_cents);
    let mortgage_princ = req.mortgage_principal.as_deref().and_then(parse_cents);

    // Calculate depreciation basis and annual depreciation
    let depreciation_basis = building_value;
    let depreciation_annual = depreciation_basis.map(|b| b / 2750); // 27.5 year residential

    let total_sqft = req.total_sqft;
    let workshop_sqft = req.workshop_sqft;
    let purchase_date = req.purchase_date.clone();
    let land_ratio = req.land_ratio;
    let mortgage_lender = req.mortgage_lender.clone();
    let notes = req.notes.clone();

    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Upsert: update if exists for this user+address, otherwise insert
        let existing: Option<i64> = conn.query_row(
            "SELECT id FROM property_profiles WHERE user_id = ? AND address = ?",
            rusqlite::params![uid, &address], |r| r.get(0)
        ).ok();

        if let Some(eid) = existing {
            conn.execute(
                "UPDATE property_profiles SET total_sqft=?, workshop_sqft=?, purchase_price_cents=?, purchase_date=?, \
                 building_value_cents=?, land_value_cents=?, land_ratio=?, assessor_total_cents=?, assessor_land_cents=?, \
                 annual_property_tax_cents=?, annual_insurance_cents=?, mortgage_lender=?, mortgage_interest_cents=?, \
                 mortgage_principal_cents=?, depreciation_basis_cents=?, depreciation_annual_cents=?, notes=?, updated_at=? \
                 WHERE id = ?",
                rusqlite::params![total_sqft, workshop_sqft, purchase_price, &purchase_date, building_value, land_value,
                    land_ratio, assessor_total, assessor_land, annual_tax, annual_ins, &mortgage_lender, mortgage_int,
                    mortgage_princ, depreciation_basis, depreciation_annual, &notes, now, eid],
            ).map_err(|e| e.to_string())?;
            Ok(eid)
        } else {
            conn.execute(
                "INSERT INTO property_profiles (user_id, address, total_sqft, workshop_sqft, purchase_price_cents, purchase_date, \
                 building_value_cents, land_value_cents, land_ratio, assessor_total_cents, assessor_land_cents, \
                 annual_property_tax_cents, annual_insurance_cents, mortgage_lender, mortgage_interest_cents, \
                 mortgage_principal_cents, depreciation_basis_cents, depreciation_annual_cents, notes, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![uid, &address, total_sqft, workshop_sqft, purchase_price, &purchase_date,
                    building_value, land_value, land_ratio, assessor_total, assessor_land,
                    annual_tax, annual_ins, &mortgage_lender, mortgage_int, mortgage_princ,
                    depreciation_basis, depreciation_annual, &notes, now, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "id": id, "success": true })))
}

// ── Item 13: Deduction Calculator Auto-Fill ─────────────────────────────────

/// Auto-fill deduction data from scanned documents and existing records.
/// Pulls mortgage interest from 1098s, property tax from tax_documents,
/// insurance from settlement docs, utilities from statement_transactions.
pub async fn handle_deduction_autofill(
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
        let start = format!("{}-01-01", year);
        let end = format!("{}-12-31", year);

        // 1. Mortgage interest from 1098 documents
        let mortgage_interest_cents: i64 = {
            let mut total = 0i64;
            if let Ok(mut stmt) = conn.prepare(
                "SELECT extracted_fields FROM tax_documents WHERE user_id = ? AND doc_type = 'mortgage_statement' AND tax_year = ? AND status = 'scanned'"
            ) {
                if let Ok(rows) = stmt.query_map(rusqlite::params![uid, year], |r| r.get::<_, Option<String>>(0)) {
                    for row in rows.flatten() {
                        if let Some(fields_str) = row {
                            if let Ok(fields) = serde_json::from_str::<serde_json::Value>(&fields_str) {
                                let interest = fields["box1_interest_paid"].as_str()
                                    .and_then(|s| parse_cents(s))
                                    .or_else(|| fields["box1_interest_paid"].as_f64().map(|f| (f * 100.0) as i64))
                                    .unwrap_or(0);
                                total += interest;
                            }
                        }
                    }
                }
            }
            total
        };

        // 2. Property tax from tax_documents or expenses
        let property_tax_cents: i64 = {
            let from_docs: i64 = conn.query_row(
                "SELECT COALESCE(SUM(CAST(json_extract(extracted_fields, '$.amount') AS REAL) * 100), 0) \
                 FROM tax_documents WHERE user_id = ? AND doc_type = 'property_tax_statement' AND tax_year = ? AND status = 'scanned'",
                rusqlite::params![uid, year], |r| r.get(0)
            ).unwrap_or(0);
            if from_docs > 0 { from_docs } else {
                // Fallback: from expenses
                conn.query_row(
                    "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                     JOIN expense_categories c ON e.category_id = c.id \
                     WHERE e.user_id = ? AND (c.name LIKE '%Property Tax%' OR c.name = 'Mortgage') \
                     AND e.expense_date >= ? AND e.expense_date <= ? AND e.description LIKE '%tax%'",
                    rusqlite::params![uid, &start, &end], |r| r.get(0)
                ).unwrap_or(0)
            }
        };

        // 3. Insurance from settlement statement or expenses
        let insurance_cents: i64 = {
            // Try settlement statement first
            let mut from_settlement = 0i64;
            if let Ok(mut stmt) = conn.prepare(
                "SELECT extracted_fields FROM tax_documents WHERE user_id = ? AND doc_type = 'settlement_statement' AND status = 'scanned'"
            ) {
                if let Ok(rows) = stmt.query_map(rusqlite::params![uid], |r| r.get::<_, Option<String>>(0)) {
                    for row in rows.flatten() {
                        if let Some(fields_str) = row {
                            if let Ok(fields) = serde_json::from_str::<serde_json::Value>(&fields_str) {
                                let ins = fields["homeowners_insurance_annual"].as_str()
                                    .and_then(|s| parse_cents(s))
                                    .or_else(|| fields["homeowners_insurance_annual"].as_f64().map(|f| (f * 100.0) as i64))
                                    .unwrap_or(0);
                                if ins > 0 { from_settlement = ins; }
                            }
                        }
                    }
                }
            }
            if from_settlement > 0 { from_settlement } else {
                // Fallback: from insurance_classifications for "home" type
                conn.query_row(
                    "SELECT amount_cents FROM insurance_classifications WHERE user_id = ? AND insurance_type = 'home' ORDER BY confidence DESC LIMIT 1",
                    rusqlite::params![uid], |r| r.get::<_, Option<i64>>(0)
                ).unwrap_or(None).map(|monthly| monthly * 12).unwrap_or(0)
            }
        };

        // 4. Utilities from statement_transactions or expenses
        let utilities_cents: i64 = {
            // Try statement_transactions first
            let has_txn_table: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='statement_transactions'",
                [], |r| r.get::<_, i64>(0)
            ).unwrap_or(0) > 0;

            if has_txn_table {
                let from_stmts: i64 = conn.query_row(
                    "SELECT COALESCE(SUM(t.amount_cents), 0) FROM statement_transactions t \
                     JOIN expense_categories c ON t.category_id = c.id \
                     WHERE t.user_id = ? AND c.name = 'Utilities' \
                     AND t.transaction_date >= ? AND t.transaction_date <= ?",
                    rusqlite::params![uid, &start, &end], |r| r.get(0)
                ).unwrap_or(0);
                if from_stmts > 0 { return Ok(build_autofill_json(mortgage_interest_cents, property_tax_cents, insurance_cents, from_stmts)); }
            }

            // Fallback: from expenses table
            conn.query_row(
                "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
                 JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND c.name = 'Utilities' \
                 AND e.expense_date >= ? AND e.expense_date <= ?",
                rusqlite::params![uid, &start, &end], |r| r.get(0)
            ).unwrap_or(0)
        };

        // 5. Property profile data
        let has_prop_table: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='property_profiles'",
            [], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0;

        let property = if has_prop_table {
            conn.query_row(
                "SELECT address, total_sqft, workshop_sqft, building_value_cents, depreciation_annual_cents \
                 FROM property_profiles WHERE user_id = ? ORDER BY id LIMIT 1",
                rusqlite::params![uid], |r| Ok(serde_json::json!({
                    "address": r.get::<_, Option<String>>(0)?,
                    "total_sqft": r.get::<_, Option<i64>>(1)?,
                    "workshop_sqft": r.get::<_, Option<i64>>(2)?,
                    "building_value_cents": r.get::<_, Option<i64>>(3)?,
                    "depreciation_annual_cents": r.get::<_, Option<i64>>(4)?,
                }))
            ).ok()
        } else { None };

        let mut result = build_autofill_json(mortgage_interest_cents, property_tax_cents, insurance_cents, utilities_cents);
        if let Some(prop) = property {
            result["property"] = prop;
        }
        Ok(result)
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

fn build_autofill_json(mortgage: i64, property_tax: i64, insurance: i64, utilities: i64) -> serde_json::Value {
    serde_json::json!({
        "mortgage_interest_cents": mortgage,
        "mortgage_interest_display": if mortgage > 0 { cents_to_display(mortgage) } else { "Not found".to_string() },
        "property_tax_cents": property_tax,
        "property_tax_display": if property_tax > 0 { cents_to_display(property_tax) } else { "Not found".to_string() },
        "insurance_cents": insurance,
        "insurance_display": if insurance > 0 { cents_to_display(insurance) } else { "Not found".to_string() },
        "utilities_cents": utilities,
        "utilities_display": if utilities > 0 { cents_to_display(utilities) } else { "Not found".to_string() },
        "total_indirect_cents": mortgage + property_tax + insurance + utilities,
        "total_indirect_display": cents_to_display(mortgage + property_tax + insurance + utilities),
        "sources": {
            "mortgage": if mortgage > 0 { "1098 documents" } else { "none" },
            "property_tax": if property_tax > 0 { "tax documents / expenses" } else { "none" },
            "insurance": if insurance > 0 { "settlement statement / classifications" } else { "none" },
            "utilities": if utilities > 0 { "statement transactions / expenses" } else { "none" },
        }
    })
}

// ── Item 14: Insurance Classification ───────────────────────────────────────

#[derive(Deserialize)]
pub struct InsuranceClassifyRequest {
    pub token: String,
    pub vendor: String,
    pub amount: Option<String>,
    pub insurance_type: String, // "auto", "home", "health", "life"
}

pub async fn handle_insurance_classify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InsuranceClassifyRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let vendor = req.vendor.clone();
    let amount_cents = req.amount.as_deref().and_then(parse_cents);
    let insurance_type = req.insurance_type.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Upsert classification
        let existing: Option<i64> = conn.query_row(
            "SELECT id FROM insurance_classifications WHERE user_id = ? AND vendor = ? AND amount_cents = ?",
            rusqlite::params![uid, &vendor, amount_cents], |r| r.get(0)
        ).ok();

        if let Some(eid) = existing {
            conn.execute(
                "UPDATE insurance_classifications SET insurance_type = ?, confidence = 1.0 WHERE id = ?",
                rusqlite::params![&insurance_type, eid],
            ).map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "INSERT INTO insurance_classifications (user_id, vendor, amount_cents, insurance_type, confidence, evidence, created_at) \
                 VALUES (?, ?, ?, ?, 1.0, 'user-classified', ?)",
                rusqlite::params![uid, &vendor, amount_cents, &insurance_type, now],
            ).map_err(|e| e.to_string())?;
        }

        // Also update any statement transactions with this vendor
        conn.execute(
            "UPDATE statement_transactions SET insurance_type = ? WHERE user_id = ? AND LOWER(vendor) = LOWER(?)",
            rusqlite::params![&insurance_type, uid, &vendor],
        ).map_err(|e| e.to_string())?;

        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "vendor": req.vendor, "insurance_type": req.insurance_type })))
}

// ── Item 15: Year-End Tax Prep Wizard ───────────────────────────────────────

pub async fn handle_tax_prep_wizard(
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
        let start = format!("{}-01-01", year);
        let end = format!("{}-12-31", year);

        // Step 1: Filing Status
        // We can infer from W-2 count
        let w2_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type = 'w2' AND tax_year = ? AND status = 'scanned'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        let w2_details: Vec<serde_json::Value> = {
            let mut stmt = conn.prepare(
                "SELECT issuer, extracted_fields FROM tax_documents WHERE user_id = ? AND doc_type = 'w2' AND tax_year = ? AND status = 'scanned'"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid, year], |r| {
                let fields_str: Option<String> = r.get(1)?;
                let fields: serde_json::Value = fields_str.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::json!({}));
                Ok(serde_json::json!({
                    "employer": r.get::<_, Option<String>>(0)?,
                    "employee": fields["employee_name"].as_str(),
                    "wages": fields["box1_wages"],
                    "withheld": fields["box2_fed_withheld"],
                }))
            }).map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };

        // Step 2: Income summary
        let gross_income: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category != 'Federal Withholding'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);
        let withheld: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = ? AND tax_year = ? AND category = 'Federal Withholding'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 3: 1099s
        let form_1099_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type LIKE '1099%' AND tax_year = ? AND status = 'scanned'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 4: 1098 mortgage statements
        let form_1098_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND doc_type = 'mortgage_statement' AND tax_year = ? AND status = 'scanned'",
            rusqlite::params![uid, year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 5: Expenses & deductions
        let total_expenses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE user_id = ? AND expense_date >= ? AND expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);
        let business_expenses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE user_id = ? AND entity = 'business' AND expense_date >= ? AND expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);
        let deductible_expenses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND c.tax_deductible = 1 AND e.expense_date >= ? AND e.expense_date <= ?",
            rusqlite::params![uid, &start, &end], |r| r.get(0)
        ).unwrap_or(0);

        // Step 6: Receipt count
        let receipt_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM receipts WHERE user_id = ?",
            rusqlite::params![uid], |r| r.get(0)
        ).unwrap_or(0);

        // Step 7: Property profile
        let has_property: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='property_profiles'",
            [], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0 && conn.query_row(
            "SELECT COUNT(*) FROM property_profiles WHERE user_id = ?",
            rusqlite::params![uid], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0;

        // Build completeness checklist
        let mut steps: Vec<serde_json::Value> = Vec::new();
        let mut missing: Vec<String> = Vec::new();

        // Filing status
        steps.push(serde_json::json!({
            "step": 1, "title": "Filing Status",
            "status": if w2_count > 0 { "complete" } else { "needs_attention" },
            "detail": if w2_count >= 2 { "2 W-2s found — likely Married Filing Jointly" }
                else if w2_count == 1 { "1 W-2 found — confirm filing status" }
                else { "No W-2s uploaded yet" },
            "data": { "w2_count": w2_count, "w2s": w2_details },
        }));
        if w2_count == 0 { missing.push("W-2 forms".to_string()); }

        // Income
        steps.push(serde_json::json!({
            "step": 2, "title": "Income",
            "status": if gross_income > 0 { "complete" } else { "needs_attention" },
            "detail": format!("Gross income: {} | Withheld: {}", cents_to_display(gross_income), cents_to_display(withheld)),
            "data": { "gross_income_cents": gross_income, "withheld_cents": withheld },
        }));

        // 1099 forms
        steps.push(serde_json::json!({
            "step": 3, "title": "1099 Forms (Interest, Dividends, etc.)",
            "status": if form_1099_count > 0 { "complete" } else { "optional" },
            "detail": format!("{} form(s) uploaded", form_1099_count),
            "data": { "count": form_1099_count },
        }));

        // Mortgage
        steps.push(serde_json::json!({
            "step": 4, "title": "Mortgage Statements (1098)",
            "status": if form_1098_count > 0 { "complete" } else { "needs_attention" },
            "detail": format!("{} statement(s) uploaded", form_1098_count),
            "data": { "count": form_1098_count },
        }));
        if form_1098_count == 0 { missing.push("1098 mortgage statements".to_string()); }

        // Business expenses
        steps.push(serde_json::json!({
            "step": 5, "title": "Business Expenses (Schedule C)",
            "status": if business_expenses > 0 { "complete" } else { "optional" },
            "detail": format!("Business expenses: {} | Total deductible: {}", cents_to_display(business_expenses), cents_to_display(deductible_expenses)),
            "data": { "business_cents": business_expenses, "deductible_cents": deductible_expenses },
        }));

        // Property / Home Office
        steps.push(serde_json::json!({
            "step": 6, "title": "Property & Home Office Deduction",
            "status": if has_property { "complete" } else { "needs_attention" },
            "detail": if has_property { "Property profile configured" } else { "Set up your property profile to calculate home office deduction" },
        }));
        if !has_property { missing.push("Property profile for home office deduction".to_string()); }

        // Receipts
        steps.push(serde_json::json!({
            "step": 7, "title": "Receipts & Documentation",
            "status": if receipt_count > 5 { "complete" } else if receipt_count > 0 { "partial" } else { "needs_attention" },
            "detail": format!("{} receipt(s) on file", receipt_count),
        }));

        // Calculate estimated tax
        let (brackets, top_rate, standard_deduction) = crate::tax::load_brackets(year, "married_jointly");
        let agi = gross_income - business_expenses;
        let deduction = std::cmp::max(standard_deduction, deductible_expenses);
        let taxable = std::cmp::max(agi - deduction, 0);
        let mut tax: i64 = 0;
        let mut prev = 0i64;
        for &(limit, rate_bps) in &brackets {
            let bracket_income = std::cmp::min(taxable, limit) - prev;
            if bracket_income <= 0 { break; }
            tax += bracket_income * rate_bps / 10000;
            prev = limit;
        }
        if taxable > prev { tax += (taxable - prev) * top_rate / 10000; }
        let owed = tax - withheld;

        let completeness = steps.iter().filter(|s| s["status"] == "complete").count() as f64 / steps.len() as f64;

        Ok(serde_json::json!({
            "year": year,
            "completeness": (completeness * 100.0).round() as i64,
            "steps": steps,
            "missing": missing,
            "summary": {
                "gross_income": cents_to_display(gross_income),
                "business_deductions": cents_to_display(business_expenses),
                "agi": cents_to_display(agi),
                "deduction": cents_to_display(deduction),
                "deduction_type": if deductible_expenses > standard_deduction { "itemized" } else { "standard" },
                "taxable_income": cents_to_display(taxable),
                "estimated_tax": cents_to_display(tax),
                "withheld": cents_to_display(withheld),
                "estimated_owed": cents_to_display(owed),
                "refund_or_owe": if owed > 0 { format!("Owe {}", cents_to_display(owed)) } else { format!("Refund {}", cents_to_display(-owed)) },
            },
            "bracket_warning": crate::tax::brackets_stale(),
        }))
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(result))
}

// ── Item 16: Tax Brackets Auto-Fetch ────────────────────────────────────────

/// Auto-fetch latest IRS brackets by searching the web.
/// This is called by an agent tool or can be triggered manually.
pub async fn handle_brackets_auto_fetch(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    let target_year: i64 = params.get("year").and_then(|y| y.parse().ok())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y").to_string().parse::<i64>().unwrap_or(2026));

    // Check if we already have this year's brackets
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let path = format!("{}/.syntaur/tax_brackets.json", home);
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&data) {
            let year_str = target_year.to_string();
            if config.get("brackets").and_then(|b| b.get(&year_str)).is_some() {
                return Ok(Json(serde_json::json!({
                    "status": "already_current",
                    "year": target_year,
                    "message": format!("Tax brackets for {} are already up to date.", target_year),
                })));
            }
        }
    }

    // Use the LLM to search for and parse IRS brackets
    let config = &state.config;
    let provider = config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| config.models.providers.iter().next());
    let (_, provider_config) = provider.ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No LLM provider".to_string()))?;
    let model = if provider_config.base_url.contains("openrouter") { "nvidia/nemotron-3-super-120b-a12b:free" }
        else if provider_config.base_url.contains("anthropic") { "claude-sonnet-4-6" }
        else { "gpt-4o-mini" };
    let url = format!("{}/chat/completions", provider_config.base_url.trim_end_matches('/'));

    let prompt = format!(
        r#"I need the US federal income tax brackets for tax year {}. The IRS publishes these in a Revenue Procedure each November (e.g., Rev. Proc. 2024-40 for 2025).

Please provide the brackets for all three filing statuses (married filing jointly, single, head of household) in this exact JSON format:

{{
  "married_jointly": {{
    "brackets": [[threshold_cents, rate_basis_points], ...],
    "top_rate": 3700,
    "standard_deduction": cents
  }},
  "single": {{ ... same format ... }},
  "head_of_household": {{ ... same format ... }}
}}

Where:
- threshold_cents = upper limit of that bracket in cents (e.g., $23,850 = 2385000)
- rate_basis_points = marginal rate in basis points (e.g., 10% = 1000, 12% = 1200)
- top_rate = rate for income above the highest bracket threshold
- standard_deduction = standard deduction amount in cents

Use the EXACT numbers from the IRS Revenue Procedure. Do NOT use 2024 numbers for {}.
If the {} brackets have not been published yet, respond with: {{"not_available": true, "reason": "..."}}

Respond with ONLY the JSON, no explanation."#,
        target_year, target_year, target_year
    );

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 2000,
        "temperature": 0.0
    });

    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", provider_config.api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("LLM: {}", e)))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("LLM HTTP {}: {}", s, &body[..body.len().min(200)])));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let cleaned = content.trim().trim_start_matches("```json").trim_end_matches("```").trim();
    let parsed: serde_json::Value = serde_json::from_str(cleaned)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Parse brackets: {}", e)))?;

    // Check for "not_available" response
    if parsed.get("not_available").and_then(|v| v.as_bool()).unwrap_or(false) {
        let reason = parsed["reason"].as_str().unwrap_or("Brackets not yet published");
        return Ok(Json(serde_json::json!({
            "status": "not_available",
            "year": target_year,
            "message": format!("{} tax brackets are not yet available: {}", target_year, reason),
            "suggestion": "The IRS typically publishes new brackets each November. Use the update_tax_brackets agent tool as a fallback.",
        })));
    }

    // Validate the response has the expected structure
    for status in &["married_jointly", "single", "head_of_household"] {
        let entry = parsed.get(status).ok_or((StatusCode::INTERNAL_SERVER_ERROR, format!("Missing {} brackets", status)))?;
        let brackets = entry.get("brackets").and_then(|b| b.as_array())
            .ok_or((StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid brackets for {}", status)))?;
        if brackets.is_empty() {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Empty brackets for {}", status)));
        }
    }

    // Save to config
    let mut config_data: serde_json::Value = std::fs::read_to_string(&path).ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or(serde_json::json!({"brackets": {}, "source": "", "notes": ""}));

    let year_str = target_year.to_string();
    config_data["brackets"][&year_str] = parsed.clone();
    config_data["last_updated"] = serde_json::json!(chrono::Utc::now().format("%Y-%m-%d").to_string());
    config_data["source"] = serde_json::json!(format!("Auto-fetched for {} via LLM", target_year));

    save_brackets(&config_data).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    info!("[tax] Auto-fetched {} tax brackets", target_year);

    Ok(Json(serde_json::json!({
        "status": "updated",
        "year": target_year,
        "message": format!("Successfully fetched and saved {} tax brackets.", target_year),
        "brackets": parsed,
    })))
}

// ── Deduction Questionnaire ────────────────────────────────────────────────

pub async fn handle_questionnaire_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok())
        .unwrap_or_else(|| default_tax_year());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT answers_json, completed FROM deduction_questionnaire WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        ) {
            Ok((json_str, completed)) => {
                let answers: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_default();
                Ok(serde_json::json!({ "answers": answers, "completed": completed, "year": year }))
            }
            Err(_) => Ok(serde_json::json!({ "answers": {}, "completed": false, "year": year })),
        }
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "questionnaire": result })))
}

#[derive(Deserialize)]
pub struct QuestionnaireSaveRequest {
    pub token: String,
    pub year: Option<i64>,
    pub answers: serde_json::Value,
    pub completed: Option<bool>,
}

pub async fn handle_questionnaire_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QuestionnaireSaveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());
    let answers_json = serde_json::to_string(&req.answers).unwrap_or_else(|_| "{}".to_string());
    let completed = req.completed.unwrap_or(false);
    let now = chrono::Utc::now().timestamp();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO deduction_questionnaire (user_id, tax_year, answers_json, completed, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, tax_year) DO UPDATE SET answers_json = excluded.answers_json, completed = excluded.completed, updated_at = excluded.updated_at",
            rusqlite::params![uid, year, &answers_json, completed, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "year": year, "completed": completed })))
}

fn default_tax_year() -> i64 {
    let now = chrono::Utc::now();
    let month = now.format("%m").to_string().parse::<i64>().unwrap_or(1);
    let year = now.format("%Y").to_string().parse::<i64>().unwrap_or(2025);
    // Before November, default to prior year (filing season). Nov+ = current year.
    if month < 11 { year - 1 } else { year }
}

// ── Deduction Auto-Scanner ─────────────────────────────────────────────────

struct DeductionRule {
    deduction_type: &'static str,
    keywords: &'static [&'static str],
    category_suggestion: &'static str,
    entity_suggestion: &'static str,
    questionnaire_gate: &'static str, // "" = always active
}

static DEDUCTION_RULES: &[DeductionRule] = &[
    DeductionRule {
        deduction_type: "medical",
        keywords: &["cvs", "walgreens", "rite aid", "pharmacy", "rx", "doctor", "hospital",
            "urgent care", "labcorp", "lab corp", "quest diagnostics", "dental", "dentist",
            "optometry", "optometrist", "vision center", "therapy", "therapist",
            "mental health", "physical therapy", "chiropractic", "hearing aid",
            "medical", "clinic", "dermatology", "radiology", "pediatric", "ortho"],
        category_suggestion: "Medical",
        entity_suggestion: "personal",
        questionnaire_gate: "",
    },
    DeductionRule {
        deduction_type: "health_insurance",
        keywords: &["blue cross", "bcbs", "aetna", "kaiser", "unitedhealth", "united health",
            "cigna", "humana", "anthem", "molina", "ambetter", "oscar health",
            "healthcare.gov", "marketplace", "health insurance", "medical premium",
            "dental premium", "vision premium"],
        category_suggestion: "Medical",
        entity_suggestion: "personal",
        questionnaire_gate: "health_insurance_self",
    },
    DeductionRule {
        deduction_type: "vehicle",
        keywords: &["shell", "chevron", "exxon", "mobil", "bp ", "arco", "76 ", "gas station",
            "fuel", "gasoline", "parking", "park garage", "toll", "e-zpass", "fastrak",
            "autozone", "o'reilly", "napa auto", "jiffy lube", "oil change", "car wash",
            "tire", "mechanic", "midas", "pep boys", "valvoline"],
        category_suggestion: "Vehicle & Mileage",
        entity_suggestion: "business",
        questionnaire_gate: "vehicle_business",
    },
    DeductionRule {
        deduction_type: "home_office",
        keywords: &["comcast", "xfinity", "spectrum", "at&t internet", "verizon fios",
            "t-mobile home", "centurylink", "lumen", "starlink", "electric", "power company",
            "pge", "pg&e", "duke energy", "con edison", "pse", "puget sound energy",
            "natural gas", "water utility", "sewer"],
        category_suggestion: "Rent & Utilities",
        entity_suggestion: "business",
        questionnaire_gate: "home_office",
    },
    DeductionRule {
        deduction_type: "software",
        keywords: &["adobe", "github", "gitlab", "aws", "amazon web services", "google cloud",
            "azure", "microsoft 365", "office 365", "dropbox", "slack", "zoom",
            "notion", "figma", "jetbrains", "openai", "anthropic", "vercel", "heroku",
            "digitalocean", "cloudflare", "netlify", "render.com", "supabase",
            "docker", "1password", "bitwarden", "canva"],
        category_suggestion: "Software & Subscriptions",
        entity_suggestion: "business",
        questionnaire_gate: "self_employed",
    },
    DeductionRule {
        deduction_type: "education",
        keywords: &["udemy", "coursera", "skillshare", "masterclass", "o'reilly",
            "pluralsight", "linkedin learning", "egghead", "tuition", "university",
            "college", "textbook", "academic", "certification", "training course"],
        category_suggestion: "Education & Training",
        entity_suggestion: "business",
        questionnaire_gate: "",
    },
    DeductionRule {
        deduction_type: "charitable",
        keywords: &["church", "parish", "temple", "synagogue", "mosque", "salvation army",
            "goodwill", "red cross", "united way", "habitat for humanity", "st jude",
            "make a wish", "food bank", "american cancer", "heart association",
            "wounded warrior", "sierra club", "aclu", "planned parenthood",
            "doctors without", "unicef", "world vision", "charity", "donation"],
        category_suggestion: "Donations",
        entity_suggestion: "personal",
        questionnaire_gate: "charitable_donations",
    },
    DeductionRule {
        deduction_type: "professional",
        keywords: &["accountant", "cpa", "accounting", "attorney", "lawyer", "legal fee",
            "tax prep", "h&r block", "turbotax", "jackson hewitt", "bookkeeper",
            "bookkeeping", "consulting fee", "notary"],
        category_suggestion: "Professional Services",
        entity_suggestion: "business",
        questionnaire_gate: "self_employed",
    },
    DeductionRule {
        deduction_type: "retirement",
        keywords: &["fidelity", "vanguard", "schwab", "charles schwab", "ira contribution",
            "401k", "roth ira", "sep-ira", "sep ira", "retirement", "td ameritrade",
            "etrade", "e*trade", "betterment", "wealthfront"],
        category_suggestion: "Retirement",
        entity_suggestion: "personal",
        questionnaire_gate: "retirement_contributions",
    },
    DeductionRule {
        deduction_type: "student_loan",
        keywords: &["navient", "nelnet", "mohela", "great lakes", "student loan",
            "fedloan", "aidvantage", "sallie mae", "dept of education"],
        category_suggestion: "Student Loan Interest",
        entity_suggestion: "personal",
        questionnaire_gate: "student_loan_interest",
    },
    DeductionRule {
        deduction_type: "hsa",
        keywords: &["hsa", "health savings", "optum bank", "fidelity hsa", "lively",
            "healthequity", "further hsa"],
        category_suggestion: "HSA Contribution",
        entity_suggestion: "personal",
        questionnaire_gate: "hdhp",
    },
];

pub fn scan_for_deduction_candidates(
    conn: &rusqlite::Connection,
    user_id: i64,
    year: i64,
) -> Result<serde_json::Value, String> {
    // Load questionnaire answers
    let answers: serde_json::Value = conn.query_row(
        "SELECT answers_json FROM deduction_questionnaire WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year],
        |r| r.get::<_, String>(0),
    ).ok()
    .and_then(|s| serde_json::from_str(&s).ok())
    .unwrap_or_else(|| serde_json::json!({}));

    let now = chrono::Utc::now().timestamp();
    let year_start = format!("{}-01-01", year);
    let year_end = format!("{}-12-31", year);
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut total = 0i64;

    // Filter rules by questionnaire gates
    let active_rules: Vec<&DeductionRule> = DEDUCTION_RULES.iter().filter(|r| {
        if r.questionnaire_gate.is_empty() { return true; }
        answers.get(r.questionnaire_gate).and_then(|v| v.as_bool()).unwrap_or(false)
    }).collect();

    // Scan statement_transactions
    {
        let mut stmt = conn.prepare(
            "SELECT id, vendor, description, amount_cents, transaction_date FROM statement_transactions \
             WHERE user_id = ? AND transaction_date >= ? AND transaction_date <= ?"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![user_id, &year_start, &year_end], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, String>(4)?))
        }).map_err(|e| e.to_string())?;

        for row in rows {
            let (id, vendor, desc, amount, date) = row.map_err(|e| e.to_string())?;
            let search_text = format!("{} {}", vendor.as_deref().unwrap_or(""), &desc).to_lowercase();
            for rule in &active_rules {
                let matched: Vec<&&str> = rule.keywords.iter().filter(|kw| search_text.contains(**kw)).collect();
                if matched.is_empty() { continue; }
                let confidence = if matched.len() >= 2 { 0.8 } else { 0.5 };
                let inserted = conn.execute(
                    "INSERT OR IGNORE INTO deduction_candidates \
                     (user_id, tax_year, source_type, source_id, deduction_type, vendor, description, \
                      amount_cents, transaction_date, category_suggestion, entity_suggestion, confidence, match_rule, status, created_at) \
                     VALUES (?, ?, 'statement_transaction', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
                    rusqlite::params![
                        user_id, year, id, rule.deduction_type,
                        vendor.as_deref().unwrap_or(""), &desc, amount.abs(), &date,
                        rule.category_suggestion, rule.entity_suggestion,
                        confidence, matched.iter().map(|k| **k).collect::<Vec<_>>().join(", "), now
                    ],
                ).unwrap_or(0);
                if inserted > 0 {
                    total += 1;
                    *counts.entry(rule.deduction_type.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // Scan financial_transactions
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, COALESCE(merchant_name, ''), amount_cents, date FROM financial_transactions \
             WHERE user_id = ? AND date >= ? AND date <= ?"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![user_id, &year_start, &year_end], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, String>(4)?))
        }).map_err(|e| e.to_string())?;

        for row in rows {
            let (id, name, merchant, amount, date) = row.map_err(|e| e.to_string())?;
            let search_text = format!("{} {}", &name, &merchant).to_lowercase();
            for rule in &active_rules {
                let matched: Vec<&&str> = rule.keywords.iter().filter(|kw| search_text.contains(**kw)).collect();
                if matched.is_empty() { continue; }
                let confidence = if matched.len() >= 2 { 0.8 } else { 0.5 };
                let vendor_display = if merchant.is_empty() { &name } else { &merchant };
                let inserted = conn.execute(
                    "INSERT OR IGNORE INTO deduction_candidates \
                     (user_id, tax_year, source_type, source_id, deduction_type, vendor, description, \
                      amount_cents, transaction_date, category_suggestion, entity_suggestion, confidence, match_rule, status, created_at) \
                     VALUES (?, ?, 'financial_transaction', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
                    rusqlite::params![
                        user_id, year, id, rule.deduction_type,
                        vendor_display, &name, amount.abs(), &date,
                        rule.category_suggestion, rule.entity_suggestion,
                        confidence, matched.iter().map(|k| **k).collect::<Vec<_>>().join(", "), now
                    ],
                ).unwrap_or(0);
                if inserted > 0 {
                    total += 1;
                    *counts.entry(rule.deduction_type.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    Ok(serde_json::json!({
        "candidates_found": total,
        "by_type": counts,
    }))
}

#[derive(Deserialize)]
pub struct DeductionScanRequest {
    pub token: String,
    pub year: Option<i64>,
}

pub async fn handle_deduction_scan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeductionScanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        scan_for_deduction_candidates(&conn, uid, year)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "year": year, "scan": result })))
}

pub async fn handle_deduction_candidates_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());
    let status_filter = params.get("status").cloned().unwrap_or_else(|| "pending".to_string());
    let type_filter = params.get("type").cloned();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Counts
        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = 'pending'",
            rusqlite::params![uid, year], |r| r.get(0)).unwrap_or(0);
        let approved: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = 'approved'",
            rusqlite::params![uid, year], |r| r.get(0)).unwrap_or(0);
        let denied: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = 'denied'",
            rusqlite::params![uid, year], |r| r.get(0)).unwrap_or(0);

        // Build query
        let mut sql = "SELECT id, deduction_type, vendor, description, amount_cents, transaction_date, \
                        category_suggestion, entity_suggestion, confidence, match_rule, status, source_type, source_id \
                        FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = ?".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(uid), Box::new(year), Box::new(status_filter),
        ];
        if let Some(ref t) = type_filter {
            sql.push_str(" AND deduction_type = ?");
            params_vec.push(Box::new(t.clone()));
        }
        sql.push_str(" ORDER BY confidence DESC, amount_cents DESC LIMIT 200");

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec.iter().map(|p| p.as_ref())), |r| {
            let amount: i64 = r.get(4)?;
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "deduction_type": r.get::<_, String>(1)?,
                "vendor": r.get::<_, Option<String>>(2)?,
                "description": r.get::<_, Option<String>>(3)?,
                "amount_cents": amount,
                "amount_display": cents_to_display(amount),
                "transaction_date": r.get::<_, Option<String>>(5)?,
                "category_suggestion": r.get::<_, Option<String>>(6)?,
                "entity_suggestion": r.get::<_, String>(7)?,
                "confidence": r.get::<_, f64>(8)?,
                "match_rule": r.get::<_, Option<String>>(9)?,
                "status": r.get::<_, String>(10)?,
                "source_type": r.get::<_, String>(11)?,
                "source_id": r.get::<_, i64>(12)?,
            }))
        }).map_err(|e| e.to_string())?;

        let candidates: Vec<serde_json::Value> = rows.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({
            "candidates": candidates,
            "counts": { "pending": pending, "approved": approved, "denied": denied },
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

pub async fn handle_deduction_candidate_context(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(candidate_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let row = conn.query_row(
            "SELECT id, deduction_type, vendor, description, amount_cents, transaction_date, \
             category_suggestion, entity_suggestion, confidence, source_type, source_id, status \
             FROM deduction_candidates WHERE id = ? AND user_id = ?",
            rusqlite::params![candidate_id, uid],
            |r| Ok((
                r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?, r.get::<_, String>(7)?, r.get::<_, f64>(8)?,
                r.get::<_, String>(9)?, r.get::<_, i64>(10)?, r.get::<_, String>(11)?,
            )),
        ).map_err(|_| "Candidate not found".to_string())?;

        let (id, ded_type, vendor, desc, amount, date, cat_sug, ent_sug, conf, src_type, src_id, status) = row;

        // Look up source document if from a statement_transaction
        let source_document = if src_type == "statement_transaction" {
            conn.query_row(
                "SELECT st.document_id FROM statement_transactions st WHERE st.id = ?",
                rusqlite::params![src_id],
                |r| r.get::<_, Option<i64>>(0),
            ).ok().flatten().and_then(|doc_id| {
                conn.query_row(
                    "SELECT id, doc_type, image_path FROM tax_documents WHERE id = ?",
                    rusqlite::params![doc_id],
                    |r| Ok(serde_json::json!({
                        "id": r.get::<_, i64>(0)?,
                        "doc_type": r.get::<_, String>(1)?,
                        "image_url": format!("/api/tax/documents/{}/image", r.get::<_, i64>(0)?),
                    })),
                ).ok()
            })
        } else {
            None
        };

        Ok(serde_json::json!({
            "candidate": {
                "id": id, "deduction_type": ded_type, "vendor": vendor, "description": desc,
                "amount_cents": amount, "amount_display": cents_to_display(amount),
                "transaction_date": date, "category_suggestion": cat_sug,
                "entity_suggestion": ent_sug, "confidence": conf,
                "source_type": src_type, "source_id": src_id, "status": status,
            },
            "source_document": source_document,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct DeductionReviewRequest {
    pub token: String,
    pub action: String, // "approve" or "deny"
    pub category: Option<String>,
    pub entity: Option<String>,
}

pub async fn handle_deduction_review(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(candidate_id): axum::extract::Path<i64>,
    Json(req): Json<DeductionReviewRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let action = req.action.clone();
    let category = req.category.clone();
    let entity = req.entity.clone();
    let now = chrono::Utc::now().timestamp();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Verify ownership
        let candidate = conn.query_row(
            "SELECT vendor, description, amount_cents, transaction_date, category_suggestion, entity_suggestion \
             FROM deduction_candidates WHERE id = ? AND user_id = ? AND status = 'pending'",
            rusqlite::params![candidate_id, uid],
            |r| Ok((
                r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?,
            )),
        ).map_err(|_| "Candidate not found or already reviewed".to_string())?;

        let (vendor, desc, amount, date, cat_sug, ent_sug) = candidate;

        if action == "approve" {
            let cat_name = category.as_deref().or(cat_sug.as_deref()).unwrap_or("Other");
            let ent = entity.as_deref().unwrap_or(&ent_sug);
            let vendor_str = vendor.as_deref().unwrap_or("Unknown");
            let date_str = date.as_deref().unwrap_or("2025-01-01");

            let category_id: Option<i64> = conn.query_row(
                "SELECT id FROM expense_categories WHERE name = ?",
                rusqlite::params![cat_name], |r| r.get(0),
            ).ok();

            conn.execute(
                "INSERT INTO expenses (user_id, amount_cents, vendor, category_id, expense_date, description, entity, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![uid, amount, vendor_str, category_id, date_str, desc.as_deref().unwrap_or(""), ent, now],
            ).map_err(|e| e.to_string())?;
            let expense_id = conn.last_insert_rowid();

            conn.execute(
                "UPDATE deduction_candidates SET status = 'approved', reviewed_at = ?, expense_id = ? WHERE id = ?",
                rusqlite::params![now, expense_id, candidate_id],
            ).map_err(|e| e.to_string())?;

            Ok(serde_json::json!({ "success": true, "action": "approved", "expense_id": expense_id }))
        } else {
            conn.execute(
                "UPDATE deduction_candidates SET status = 'denied', reviewed_at = ? WHERE id = ?",
                rusqlite::params![now, candidate_id],
            ).map_err(|e| e.to_string())?;

            Ok(serde_json::json!({ "success": true, "action": "denied" }))
        }
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct BulkReviewRequest {
    pub token: String,
    pub ids: Vec<i64>,
    pub action: String,
    pub category: Option<String>,
    pub entity: Option<String>,
}

pub async fn handle_deduction_bulk_review(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BulkReviewRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let ids = req.ids.clone();
    let action = req.action.clone();
    let category = req.category.clone();
    let entity = req.entity.clone();
    let now = chrono::Utc::now().timestamp();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut approved_count = 0i64;
        let mut denied_count = 0i64;

        for cid in &ids {
            let candidate = conn.query_row(
                "SELECT vendor, description, amount_cents, transaction_date, category_suggestion, entity_suggestion \
                 FROM deduction_candidates WHERE id = ? AND user_id = ? AND status = 'pending'",
                rusqlite::params![cid, uid],
                |r| Ok((
                    r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?,
                )),
            );
            let (vendor, desc, amount, date, cat_sug, ent_sug) = match candidate {
                Ok(c) => c,
                Err(_) => continue,
            };

            if action == "approve" {
                let cat_name = category.as_deref().or(cat_sug.as_deref()).unwrap_or("Other");
                let ent = entity.as_deref().unwrap_or(&ent_sug);
                let category_id: Option<i64> = conn.query_row(
                    "SELECT id FROM expense_categories WHERE name = ?",
                    rusqlite::params![cat_name], |r| r.get(0),
                ).ok();
                conn.execute(
                    "INSERT INTO expenses (user_id, amount_cents, vendor, category_id, expense_date, description, entity, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![uid, amount, vendor.as_deref().unwrap_or(""), category_id,
                        date.as_deref().unwrap_or("2025-01-01"), desc.as_deref().unwrap_or(""), ent, now],
                ).ok();
                let expense_id = conn.last_insert_rowid();
                conn.execute(
                    "UPDATE deduction_candidates SET status = 'approved', reviewed_at = ?, expense_id = ? WHERE id = ?",
                    rusqlite::params![now, expense_id, cid],
                ).ok();
                approved_count += 1;
            } else {
                conn.execute(
                    "UPDATE deduction_candidates SET status = 'denied', reviewed_at = ? WHERE id = ?",
                    rusqlite::params![now, cid],
                ).ok();
                denied_count += 1;
            }
        }

        Ok(serde_json::json!({ "success": true, "approved": approved_count, "denied": denied_count }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

pub async fn handle_deduction_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let quest_complete: bool = conn.query_row(
            "SELECT completed FROM deduction_questionnaire WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get(0),
        ).unwrap_or(false);

        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = 'pending'",
            rusqlite::params![uid, year], |r| r.get(0)).unwrap_or(0);
        let approved_cents: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM deduction_candidates WHERE user_id = ? AND tax_year = ? AND status = 'approved'",
            rusqlite::params![uid, year], |r| r.get(0)).unwrap_or(0);

        // By type breakdown
        let mut stmt = conn.prepare(
            "SELECT deduction_type, status, COUNT(*), COALESCE(SUM(amount_cents), 0) \
             FROM deduction_candidates WHERE user_id = ? AND tax_year = ? GROUP BY deduction_type, status"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        }).map_err(|e| e.to_string())?;

        let mut by_type: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
        for row in rows {
            let (dtype, status, count, amount) = row.map_err(|e| e.to_string())?;
            let entry = by_type.entry(dtype).or_insert_with(|| serde_json::json!({"pending": 0, "approved": 0, "denied": 0, "approved_cents": 0}));
            entry[&status] = serde_json::json!(count);
            if status == "approved" {
                entry["approved_cents"] = serde_json::json!(amount);
            }
        }

        Ok(serde_json::json!({
            "questionnaire_complete": quest_complete,
            "total_pending": pending,
            "total_approved_cents": approved_cents,
            "total_approved_display": cents_to_display(approved_cents),
            "by_type": by_type,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}
