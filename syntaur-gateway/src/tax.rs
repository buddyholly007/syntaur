//! Tax module — receipt scanning, expense tracking, tax dashboard.
//! Premium add-on module ($49).

use std::collections::HashMap;
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
    // Additional credits (Phase 1A)
    pub eitc: i64,
    pub cdcc: i64,
    pub education_credits: i64,
    pub savers_credit: i64,
    pub energy_credits: i64,
    pub total_credits: i64,
    pub estimated_payments: i64,
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

    // ── Credits ──

    // Child Tax Credit ($2,000 per qualifying child)
    let num_dependents: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dependents WHERE user_id = ? AND tax_year = ? AND qualifies_ctc = 1",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);
    let child_credit = num_dependents * 200_000; // $2,000 per child

    // EITC — Earned Income Tax Credit
    let earned_income = w2_income + std::cmp::max(se_net, 0);
    let eitc = compute_eitc(earned_income, agi, num_dependents, filing_status);

    // CDCC — Child and Dependent Care Credit (Form 2441)
    let cdcc = {
        let care_expenses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(amount_cents), 0) FROM dependent_care_expenses WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![user_id, year], |r| r.get(0),
        ).unwrap_or(0);
        if care_expenses > 0 && num_dependents > 0 {
            let max_qualifying = if num_dependents >= 2 { 600_000 } else { 300_000 }; // $6K or $3K
            let qualifying = std::cmp::min(care_expenses, max_qualifying);
            // Percentage: 35% at AGI ≤ $15K, decreasing 1% per $2K, floor 20%
            let pct = if agi <= 1_500_000 { 3500 }
                else { std::cmp::max(3500 - ((agi - 1_500_000) / 200_000) * 100, 2000) };
            qualifying * pct / 10000
        } else { 0 }
    };

    // Education Credits — AOTC ($2,500/student, 40% refundable) + LLC ($2,000/return)
    let education_credits = {
        let edu_expenses: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT student_name, COALESCE(SUM(tuition_cents + fees_cents + books_cents), 0) \
                 FROM education_expenses WHERE user_id = ? AND tax_year = ? GROUP BY student_name"
            ).ok();
            match stmt {
                Some(ref mut s) => s.query_map(rusqlite::params![user_id, year], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let mut aotc_total = 0i64;
        for (_student, expenses) in &edu_expenses {
            // AOTC: 100% of first $2K + 25% of next $2K = max $2,500
            let first = std::cmp::min(*expenses, 200_000);
            let next = std::cmp::min(std::cmp::max(*expenses - 200_000, 0), 200_000);
            aotc_total += first + next * 2500 / 10000;
        }
        // Phase-out: MFJ $160K-$180K, others $80K-$90K
        let (aotc_start, aotc_range) = match filing_status {
            "married_jointly" => (16_000_000i64, 2_000_000i64),
            _ => (8_000_000i64, 1_000_000i64),
        };
        if agi > aotc_start + aotc_range { 0 }
        else if agi > aotc_start {
            let reduction = ((agi - aotc_start) as f64 / aotc_range as f64).min(1.0);
            ((1.0 - reduction) * aotc_total as f64) as i64
        } else { aotc_total }
    };

    // Saver's Credit — 50%/20%/10% of retirement contributions up to $2K ($4K MFJ)
    let savers_credit = {
        let retirement_contributions: i64 = conn.query_row(
            "SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e \
             JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND (c.name LIKE '%401k%' OR c.name LIKE '%IRA%' OR c.name LIKE '%Retirement%') \
             AND e.expense_date >= ? AND e.expense_date <= ?",
            rusqlite::params![user_id, &start, &end], |r| r.get(0),
        ).unwrap_or(0);
        if retirement_contributions > 0 {
            let max_contribution = if filing_status == "married_jointly" { 400_000 } else { 200_000 };
            let qualifying = std::cmp::min(retirement_contributions, max_contribution);
            // Rate based on AGI (2025 thresholds)
            let (t1, t2) = match filing_status {
                "married_jointly" => (4_600_000i64, 5_000_000i64),
                "head_of_household" => (3_450_000i64, 3_750_000i64),
                _ => (2_300_000i64, 2_500_000i64),
            };
            let rate = if agi <= t1 { 5000 } // 50%
                else if agi <= t2 { 2000 }   // 20%
                else if agi <= t2 + 500_000 { 1000 } // 10%
                else { 0 };
            qualifying * rate / 10000
        } else { 0 }
    };

    // Energy Credits — residential energy property (Form 5695)
    let energy_credits = {
        let improvements: i64 = conn.query_row(
            "SELECT COALESCE(SUM(qualifying_cents), 0) FROM energy_improvements WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![user_id, year], |r| r.get(0),
        ).unwrap_or(0);
        // 30% credit, max $3,200/year for energy property + $2,000 for heat pumps
        std::cmp::min(improvements * 3000 / 10000, 520_000) // $5,200 combined cap
    };

    let total_credits = child_credit + eitc + cdcc + education_credits + savers_credit + energy_credits;

    // ── Estimated tax payments ──
    let estimated_payments: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM estimated_tax_payments \
         WHERE user_id = ? AND tax_year = ? AND status != 'cancelled'",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);

    let total_payments = w2_withheld + fed_paid + total_credits + estimated_payments;
    let owed = total_tax - total_payments;

    let effective_rate = if gross_income > 0 { (total_tax as f64 / gross_income as f64) * 100.0 } else { 0.0 };

    Ok(TaxEstimate {
        gross_income, se_income, w2_income, biz_deductions: biz_deductions_adj,
        meals_total, meals_adjustment, se_tax, half_se_tax, se_health_deduction,
        student_loan_deduction, agi, standard_deduction, itemized_deduction,
        salt_capped, medical_deductible, qbi_deduction, deduction_used, deduction_type: deduction_type.to_string(),
        taxable_income, ordinary_tax, ltcg_tax, total_tax, w2_withheld, fed_paid,
        child_credit, eitc, cdcc, education_credits, savers_credit, energy_credits,
        total_credits, estimated_payments, total_payments, owed, effective_rate,
    })
}

/// Compute EITC based on 2025 thresholds. Returns credit amount in cents.
/// Uses simplified table: earned income, AGI, number of qualifying children, filing status.
fn compute_eitc(earned_income: i64, agi: i64, children: i64, filing_status: &str) -> i64 {
    let is_joint = filing_status == "married_jointly";
    // 2025 EITC parameters: (max_credit, phase_in_end, phase_out_start, phase_out_end)
    // All values in cents
    let (max_credit, phase_in_end, phase_out_start_single, phase_out_end_single) = match children {
        0 => (64_900, 830_000, 1_020_000, 1_910_000),         // $649 max
        1 => (434_100, 1_230_000, 2_190_000, 5_281_000),      // $4,341 max
        2 => (717_000, 1_730_000, 2_190_000, 5_998_000),      // $7,170 max
        _ => (807_000, 1_730_000, 2_190_000, 6_412_000),      // $8,070 max (3+)
    };
    let joint_bump = if is_joint { 740_000 } else { 0 }; // $7,400 MFJ bump
    let phase_out_start = phase_out_start_single + joint_bump;
    let phase_out_end = phase_out_end_single + joint_bump;

    // Use the lesser of earned income or AGI for phase-out
    let income = std::cmp::max(earned_income, agi);
    if income <= 0 || income > phase_out_end { return 0; }

    // Phase-in: credit increases as earned income rises to phase_in_end
    let credit_from_income = if earned_income < phase_in_end {
        max_credit * earned_income / phase_in_end
    } else {
        max_credit
    };

    // Phase-out: credit decreases as income rises from phase_out_start to phase_out_end
    let credit_after_phaseout = if income > phase_out_start {
        let reduction = max_credit * (income - phase_out_start) / (phase_out_end - phase_out_start);
        std::cmp::max(max_credit - reduction, 0)
    } else {
        max_credit
    };

    std::cmp::min(credit_from_income, credit_after_phaseout)
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

    // Call LLM with vision — priority: configured vision provider → local GPU swap → cloud fallback
    let client = &state.client;
    let config = &state.config;

    // 1. Check for user-configured vision provider (from settings)
    let vision_provider = config.models.providers.get("vision");

    // 2. Try local GPU hot-swap (our specific setup)
    let local_gpu_host = "192.168.1.69";
    let local_vision_port = 8900;

    let (url, model, api_key, used_local_swap) = if let Some(vp) = vision_provider {
        // User configured a vision endpoint — use it directly
        info!("[tax] Using configured vision provider: {}", vp.base_url);
        (
            format!("{}/chat/completions", vp.base_url.trim_end_matches('/')),
            vp.models.first().map(|m| m.id.clone()).unwrap_or_else(|| "qwen2.5-vl".to_string()),
            vp.api_key.clone(),
            false,
        )
    } else {
        // Try local GPU hot-swap
        let swap_ok = {
            let swap_result = tokio::process::Command::new("ssh")
                .args([&format!("sean@{}", local_gpu_host), "bash", "/home/sean/swap-to-vision.sh"])
                .output().await;
            match swap_result {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if stdout.contains("READY") {
                        info!("[tax] Local vision model ready on {}:{}", local_gpu_host, local_vision_port);
                        true
                    } else {
                        info!("[tax] Local vision swap failed: {}, falling back to cloud", stdout.trim());
                        false
                    }
                }
                Err(e) => {
                    info!("[tax] Cannot reach local GPU ({}), falling back to cloud", e);
                    false
                }
            }
        };

        if swap_ok {
            (
                format!("http://{}:{}/v1/chat/completions", local_gpu_host, local_vision_port),
                "qwen2.5-vl-7b".to_string(),
                String::new(),
                true,
            )
        } else {
            // Cloud fallback
            let provider = config.models.providers.iter()
                .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
                .or_else(|| config.models.providers.iter().next());
            let (_, pc) = provider.ok_or("No LLM provider configured")?;
            let m = if pc.base_url.contains("openrouter") { "nvidia/nemotron-nano-12b-v2-vl:free" }
                else if pc.base_url.contains("anthropic") { "claude-sonnet-4-6" }
                else { "gpt-4o-mini" };
            (format!("{}/chat/completions", pc.base_url.trim_end_matches('/')), m.to_string(), pc.api_key.clone(), false)
        }
    };

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

    let mut req = client.post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(90));
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let resp = req.send().await.map_err(|e| format!("LLM request: {}", e))?;

    // Swap back to chat model in background (don't block on it)
    if used_local_swap {
        let host = local_gpu_host.to_string();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("ssh")
                .args([&format!("sean@{}", host), "bash", "/home/sean/swap-to-chat.sh"])
                .output().await;
            info!("[tax] Restored chat model on {}", host);
        });
    }

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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();

    let receipts = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT r.id, r.vendor, r.amount_cents, r.receipt_date, r.description, r.status, r.created_at, c.name \
              FROM receipts r LEFT JOIN expense_categories c ON r.category_id = c.id \
              WHERE r.user_id = ? ORDER BY r.created_at DESC LIMIT 100".to_string(),
             vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>])
        } else {
            ("SELECT r.id, r.vendor, r.amount_cents, r.receipt_date, r.description, r.status, r.created_at, c.name \
              FROM receipts r LEFT JOIN expense_categories c ON r.category_id = c.id \
              ORDER BY r.created_at DESC LIMIT 100".to_string(),
             vec![])
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |r| {
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

For W-2 forms, extract: employer_name, employer_ein, employee_name, employee_ssn_last4, employee_ssn_full (the full ###-##-#### if visible — leave null if redacted), employee_address_line1, employee_city, employee_state, employee_zip, box1_wages, box2_fed_withheld, box3_ss_wages, box4_ss_withheld, box5_medicare_wages, box6_medicare_withheld, box12_codes, box14_other, state, box16_state_wages, box17_state_withheld
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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let year_filter = params.get("year").and_then(|y| y.parse::<i64>().ok());

    let docs = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let (sql, p): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match (scope, year_filter) {
            (Some(uid), Some(y)) => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? AND tax_year = ? AND status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(y)],
            ),
            (Some(uid), None) => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE user_id = ? AND status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>],
            ),
            (None, Some(y)) => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE tax_year = ? AND status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
                vec![Box::new(y) as Box<dyn rusqlite::types::ToSql>],
            ),
            (None, None) => (
                "SELECT id, doc_type, tax_year, issuer, extracted_fields, status, created_at, image_path FROM tax_documents WHERE status != 'discarded' ORDER BY doc_type, created_at DESC".to_string(),
                vec![],
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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let new_status = req.status.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        if let Some(uid) = scope {
            conn.execute(
                "UPDATE tax_documents SET status = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![&new_status, id, uid],
            ).map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE tax_documents SET status = ? WHERE id = ?",
                rusqlite::params![&new_status, id],
            ).map_err(|e| e.to_string())?;
        }

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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let field = req.field.clone();
    let value = req.value.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Get current fields
        let current: String = if let Some(uid) = scope {
            conn.query_row(
                "SELECT COALESCE(extracted_fields, '{}') FROM tax_documents WHERE id = ? AND user_id = ?",
                rusqlite::params![id, uid], |r| r.get(0)
            ).map_err(|e| format!("Not found: {}", e))?
        } else {
            conn.query_row(
                "SELECT COALESCE(extracted_fields, '{}') FROM tax_documents WHERE id = ?",
                rusqlite::params![id], |r| r.get(0)
            ).map_err(|e| format!("Not found: {}", e))?
        };

        let mut fields: serde_json::Value = serde_json::from_str(&current).unwrap_or(serde_json::json!({}));

        // Try to parse as number, otherwise store as string
        if let Ok(num) = value.parse::<f64>() {
            fields[&field] = serde_json::json!(num);
        } else {
            fields[&field] = serde_json::json!(value);
        }

        let updated = serde_json::to_string(&fields).unwrap_or("{}".to_string());
        if let Some(uid) = scope {
            conn.execute(
                "UPDATE tax_documents SET extracted_fields = ? WHERE id = ? AND user_id = ?",
                rusqlite::params![&updated, id, uid],
            ).map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE tax_documents SET extracted_fields = ? WHERE id = ?",
                rusqlite::params![&updated, id],
            ).map_err(|e| e.to_string())?;
        }

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
            // For W-2 income record updates, look up the doc's actual owner
            let doc_owner: Option<i64> = conn.query_row(
                "SELECT user_id FROM tax_documents WHERE id = ?", rusqlite::params![id], |r| r.get(0)
            ).ok();

            if field == "box1_wages" {
                if let Ok(cents) = value.parse::<f64>().map(|f| (f * 100.0) as i64) {
                    if let Some(owner) = doc_owner {
                        conn.execute(
                            "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Wages' AND description LIKE ?",
                            rusqlite::params![cents, owner, year, format!("%{}%", issuer)],
                        ).ok();
                    }
                }
            } else if field == "box2_fed_withheld" {
                if let Ok(cents) = value.parse::<f64>().map(|f| (f * 100.0) as i64) {
                    if let Some(owner) = doc_owner {
                        conn.execute(
                            "UPDATE tax_income SET amount_cents = ? WHERE user_id = ? AND tax_year = ? AND source = 'W-2 Withholding' AND description LIKE ?",
                            rusqlite::params![cents, owner, year, format!("%{}%", issuer)],
                        ).ok();
                    }
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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let entity_filter = params.get("entity").cloned();
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();

    let expenses = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut sql = "SELECT e.id, e.amount_cents, e.vendor, e.expense_date, e.description, e.entity, e.receipt_id, e.created_at, c.name \
                       FROM expenses e LEFT JOIN expense_categories c ON e.category_id = c.id \
                       WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![];
        if let Some(uid) = scope {
            sql.push_str(" AND e.user_id = ?");
            params_vec.push(Box::new(uid));
        }

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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();

    // Default to YTD
    let year = chrono::Utc::now().format("%Y").to_string();
    let start = params.get("start").cloned().unwrap_or_else(|| format!("{}-01-01", year));
    let end = params.get("end").cloned().unwrap_or_else(|| format!("{}-12-31", year));

    let summary = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Helper for user scope WHERE fragment
        let uid_clause = |prefix: &str| -> String {
            if let Some(uid) = scope { format!("{}.user_id = {} AND", prefix, uid) } else { String::new() }
        };
        let uid_clause_bare = || -> String {
            if let Some(uid) = scope { format!("user_id = {} AND", uid) } else { String::new() }
        };

        // By category
        let mut stmt = conn.prepare(&format!(
            "SELECT c.name, c.entity, c.tax_deductible, SUM(e.amount_cents), COUNT(*) \
             FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE {} e.expense_date >= ? AND e.expense_date <= ? \
             GROUP BY c.id ORDER BY SUM(e.amount_cents) DESC", uid_clause("e")
        )).map_err(|e| e.to_string())?;
        let categories: Vec<serde_json::Value> = stmt.query_map(
            rusqlite::params![&start, &end],
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
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE {} expense_date >= ? AND expense_date <= ?", uid_clause_bare()),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let total_business: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE {} entity = 'business' AND expense_date >= ? AND expense_date <= ?", uid_clause_bare()),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let total_deductible: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE {} c.tax_deductible = 1 AND e.expense_date >= ? AND e.expense_date <= ?", uid_clause("e")),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);

        let receipt_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM receipts WHERE {} created_at >= ? AND created_at <= ?", uid_clause_bare()),
            rusqlite::params![chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").map(|d| d.and_hms_opt(0,0,0).unwrap().and_utc().timestamp()).unwrap_or(0),
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

#[derive(Deserialize)]
pub struct CategoryCreateRequest {
    pub token: String,
    pub name: String,
    pub entity: Option<String>,
    pub tax_deductible: Option<bool>,
}

pub async fn handle_category_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CategoryCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let name = req.name.clone();
    let entity = req.entity.clone().unwrap_or_else(|| "business".to_string());
    let deductible = req.tax_deductible.unwrap_or(entity == "business");

    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO expense_categories (name, entity, tax_deductible, user_id, is_custom) VALUES (?, ?, ?, ?, 1)",
            rusqlite::params![&name, &entity, deductible, uid],
        ).map_err(|e| e.to_string())?;
        let id = conn.query_row(
            "SELECT id FROM expense_categories WHERE name = ?", rusqlite::params![&name], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        Ok(id)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true, "id": id, "name": req.name })))
}

pub async fn handle_category_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(cat_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _ = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let db = state.db_path.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        // Only allow deleting custom categories
        conn.execute("DELETE FROM expense_categories WHERE id = ? AND is_custom = 1", rusqlite::params![cat_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Income ───────────────────────────────────────────────────────────────────

pub async fn handle_income_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
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

        let (list_sql, sum_sql_gross, sum_sql_with) = if let Some(uid) = scope {
            (
                format!("SELECT source, amount_cents, category, description FROM tax_income WHERE user_id = {} AND tax_year = ? ORDER BY amount_cents DESC", uid),
                format!("SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = {} AND tax_year = ? AND category != 'Federal Withholding'", uid),
                format!("SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE user_id = {} AND tax_year = ? AND category = 'Federal Withholding'", uid),
            )
        } else {
            (
                "SELECT source, amount_cents, category, description FROM tax_income WHERE tax_year = ? ORDER BY amount_cents DESC".to_string(),
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE tax_year = ? AND category != 'Federal Withholding'".to_string(),
                "SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE tax_year = ? AND category = 'Federal Withholding'".to_string(),
            )
        };

        let mut stmt = conn.prepare(&list_sql).map_err(|e| e.to_string())?;
        let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![year], |r| {
            Ok(serde_json::json!({
                "source": r.get::<_, String>(0)?,
                "amount_cents": r.get::<_, i64>(1)?,
                "category": r.get::<_, Option<String>>(2)?,
                "description": r.get::<_, Option<String>>(3)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let gross: i64 = conn.query_row(&sum_sql_gross, rusqlite::params![year], |r| r.get(0)).unwrap_or(0);
        let withheld: i64 = conn.query_row(&sum_sql_with, rusqlite::params![year], |r| r.get(0)).unwrap_or(0);

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

/// Identification data we can scrape from already-scanned tax documents
/// when the taxpayer profile is missing or incomplete. Only fields that the
/// vision pipeline reliably extracts (W-2 employee_name + last-4 SSN,
/// mortgage statement property_address) appear here.
#[derive(Default, Debug)]
pub struct ScannedIdentity {
    pub primary_name: Option<String>,
    pub spouse_name: Option<String>,
    /// SSN in the form `XXX-XX-####` — derived from employee_ssn_last4 since
    /// the vision prompt currently only asks for the last 4 digits.
    pub primary_ssn_masked: Option<String>,
    pub spouse_ssn_masked: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
}

/// Walk this user's scanned tax documents for the year and pull out
/// anything that maps onto a Form 4868 identification field. Returns the
/// most-recent non-empty values; missing fields stay None.
///
/// Sources we mine:
/// - **W-2** (`doc_type='w2'`): `employee_name`, `employee_ssn_last4`, `state`
/// - **1095-C** (`doc_type='1095_c'`): `employee` (fallback name)
/// - **Mortgage statement** (`doc_type='mortgage_statement'`):
///   `property_address` — multi-line "STREET\nCITY, ST ZIP" string we parse
pub fn pull_scan_identity(conn: &rusqlite::Connection, uid: i64, year: i64) -> ScannedIdentity {
    let mut id = ScannedIdentity::default();

    // ---- Names + SSN-last-4 from W-2s + 1095-C ----
    // Order by created_at DESC so the *most recent* scan wins. The first
    // W-2 we see becomes the primary; a second one with a different name
    // becomes the spouse. 1095-C is a weaker fallback for the primary name.
    let mut stmt = match conn.prepare(
        "SELECT doc_type, extracted_fields FROM tax_documents \
         WHERE user_id = ? AND tax_year = ? AND status = 'scanned' \
           AND extracted_fields IS NOT NULL \
           AND doc_type IN ('w2', '1095_c', 'mortgage_statement') \
         ORDER BY created_at DESC"
    ) { Ok(s) => s, Err(_) => return id };

    let rows = stmt.query_map(rusqlite::params![uid, year], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    });
    let rows = match rows { Ok(r) => r, Err(_) => return id };

    let pick = |s: &serde_json::Value| -> Option<String> {
        s.as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
    };

    for row in rows.flatten() {
        let (doc_type, fields_json) = row;
        let fields: serde_json::Value = match serde_json::from_str(&fields_json) {
            Ok(v) => v, Err(_) => continue,
        };

        match doc_type.as_str() {
            "w2" => {
                let name = pick(&fields["employee_name"]);
                // Prefer full SSN if vision could read it, fall back to
                // last-4 in masked form. We send "XXX-XX-####" for masked
                // so the form at least shows recognizable digits.
                let ssn = pick(&fields["employee_ssn_full"])
                    .or_else(|| pick(&fields["employee_ssn_last4"]).map(|s| format!("XXX-XX-{}", s)));
                let state_v = pick(&fields["employee_state"]).or_else(|| pick(&fields["state"]));
                let addr_v = pick(&fields["employee_address_line1"]);
                let city_v = pick(&fields["employee_city"]);
                let zip_v = pick(&fields["employee_zip"]);

                if id.primary_name.is_none() {
                    id.primary_name = name.clone();
                    id.primary_ssn_masked = ssn.clone();
                } else if id.spouse_name.is_none() {
                    // Only treat as spouse if the name actually differs.
                    if name.as_deref().map(|n| Some(n.to_lowercase()) != id.primary_name.as_deref().map(str::to_lowercase)).unwrap_or(false) {
                        id.spouse_name = name;
                        id.spouse_ssn_masked = ssn;
                    }
                }
                if id.state.is_none() { id.state = state_v; }
                if id.address.is_none() { id.address = addr_v; }
                if id.city.is_none() { id.city = city_v; }
                if id.zip.is_none() { id.zip = zip_v; }
            }
            "1095_c" => {
                if id.primary_name.is_none() {
                    id.primary_name = pick(&fields["employee"]);
                }
            }
            "mortgage_statement" => {
                // property_address is typically "STREET\nCITY, ST ZIP".
                if let Some(addr) = pick(&fields["property_address"]) {
                    let mut lines = addr.lines();
                    let line1 = lines.next().map(str::trim).unwrap_or("");
                    let line2 = lines.next().map(str::trim).unwrap_or("");
                    if id.address.is_none() && !line1.is_empty() {
                        id.address = Some(line1.to_string());
                    }
                    // Parse "City, ST ZIP" — last whitespace-token is ZIP,
                    // the token before is the 2-char state, the rest before
                    // the comma is the city.
                    if !line2.is_empty() {
                        if let Some((city_part, st_zip)) = line2.rsplit_once(',') {
                            let city = city_part.trim().to_string();
                            let mut tokens: Vec<&str> = st_zip.split_whitespace().collect();
                            let zip_tok = tokens.pop();
                            let st_tok = tokens.pop();
                            if id.city.is_none() && !city.is_empty() { id.city = Some(city); }
                            if id.state.is_none() {
                                if let Some(s) = st_tok { id.state = Some(s.to_string()); }
                            }
                            if id.zip.is_none() {
                                if let Some(z) = zip_tok { id.zip = Some(z.to_string()); }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    id
}

/// Render the user's Form 4868 as a real fillable PDF — bundled blank IRS
/// template with /V values set from the user's profile + tax estimate. The
/// reader regenerates appearances on open thanks to /NeedAppearances=true.
///
/// Query parameters (all optional):
/// - `payment` — amount in cents the user is paying with the extension (line 7).
///   Falls back to the balance due so a user who owes sees their full
///   amount preselected.
/// - `out_of_country=1` — checks line 8 (US citizens/residents abroad get an
///   automatic 2-month extension; this box claims the additional 4 months).
/// - `is_1040nr=1` — checks line 9 (1040-NR filer with no withheld wages).
/// - `name`, `address`, `city`, `state`, `zip`, `ssn`, `spouse_ssn` —
///   override the values pulled from `taxpayer_profiles`. Useful when the
///   profile is incomplete and the user types missing data into the UI.
/// - `fy_begin`, `fy_end`, `fy_end_year` — fiscal-year filers (calendar
///   filers leave blank).
pub async fn handle_extension(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());
    let payment_override: Option<i64> = params.get("payment").and_then(|p| p.parse().ok());
    let out_of_country = params.get("out_of_country").map(|s| s == "1" || s == "true").unwrap_or(false);
    let is_1040nr = params.get("is_1040nr").map(|s| s == "1" || s == "true").unwrap_or(false);

    // Optional UI-supplied overrides for any identification field. Empty
    // strings are treated as "use profile value", not "force blank".
    let q = |k: &str| -> Option<String> {
        params.get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let name_override = q("name");
    let address_override = q("address");
    let city_override = q("city");
    let state_override = q("state");
    let zip_override = q("zip");
    let ssn_override = q("ssn");
    let spouse_ssn_override = q("spouse_ssn");
    let fy_begin = q("fy_begin");
    let fy_end = q("fy_end");
    let fy_end_year = q("fy_end_year");

    let pdf_bytes = tokio::task::spawn_blocking(move || -> Result<(crate::tax_pdf::Form4868Data, bool, String), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Pull filing status so the estimate (and the spouse name + SSN
        // logic) matches what's on file.
        let filing_status: String = conn.query_row(
            "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get(0),
        ).unwrap_or_else(|_| "single".to_string());

        let est = compute_tax_estimate(&conn, uid, year, &filing_status)?;
        let balance_due_cents = std::cmp::max(est.total_tax - est.total_payments, 0);
        // The IRS form has its own dollar sign and column alignment, so we
        // emit bare "1234.56" — no leading $ or thousands separators.
        let dollars = |c: i64| format!("{:.2}", c as f64 / 100.0);
        // If the user didn't say what they're paying, default to the full
        // balance due — that's what most filers want when extending.
        let amount_paying_cents = payment_override.unwrap_or(balance_due_cents);

        // Pull identification from taxpayer_profiles. The "ssn_encrypted"
        // and "spouse_ssn_encrypted" columns currently store plaintext —
        // the encryption layer was never wired in. (Tracked separately;
        // for the extension form we just read whatever's there.)
        type ProfileRow = (
            Option<String>, Option<String>, Option<String>, Option<String>,
            Option<String>, Option<String>, Option<String>, Option<String>,
            Option<String>, Option<String>, Option<String>,
        );
        let profile: ProfileRow = conn.query_row(
            "SELECT first_name, last_name, ssn_encrypted, address_line1, address_line2, \
                    city, state, zip, spouse_first, spouse_last, spouse_ssn_encrypted \
             FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                    r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?))
        ).unwrap_or_else(|_| (None, None, None, None, None, None, None, None, None, None, None));
        let (first, last, ssn_db, addr1, addr2, city, st, zip, sp_first, sp_last, sp_ssn_db) = profile;

        let user_display: String = conn.query_row(
            "SELECT COALESCE(display_name, username, '') FROM users WHERE id = ?",
            rusqlite::params![uid], |r| r.get(0),
        ).unwrap_or_default();

        // Mine scanned tax documents (W-2 / 1095-C / mortgage statement) so
        // a user who's only uploaded paperwork — no profile yet — still
        // gets a fully-filled form.
        let scan = pull_scan_identity(&conn, uid, year);

        // Build the name line: "First Last & Spouse First Last" if joint,
        // otherwise just the primary taxpayer. Prefer the profile, fall
        // back to the W-2/1095-C employee name from scans.
        let primary = match (first.as_deref(), last.as_deref()) {
            (Some(f), Some(l)) if !f.is_empty() || !l.is_empty() => format!("{} {}", f, l).trim().to_string(),
            _ => scan.primary_name.clone().unwrap_or_else(|| user_display.clone()),
        };
        let mut name_line = primary;
        if filing_status == "married_jointly" {
            let spouse = match (sp_first.as_deref(), sp_last.as_deref()) {
                (Some(f), Some(l)) if !f.is_empty() || !l.is_empty() => format!("{} {}", f, l).trim().to_string(),
                _ => scan.spouse_name.clone().unwrap_or_default(),
            };
            if !spouse.is_empty() {
                name_line = format!("{} & {}", name_line, spouse);
            }
        }

        // Combine address_line1 + address_line2 into the single Line 1
        // street field — the form has one ~265pt-wide box, plenty for both.
        let address_combined = match (addr1.as_deref(), addr2.as_deref()) {
            (Some(a1), Some(a2)) if !a2.is_empty() => format!("{}, {}", a1, a2),
            (Some(a1), _) => a1.to_string(),
            _ => String::new(),
        };

        // A SSN is "masked" when it starts with XXX or is shorter than the
        // 9 digits the IRS expects. Treat masked profile values as a weak
        // fallback — a fully-extracted SSN from a re-scanned W-2 should
        // win over the profile's redacted preview.
        let is_full_ssn = |s: &Option<String>| -> bool {
            s.as_deref().map(|v| {
                let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                digits.len() == 9 && !v.to_uppercase().contains('X')
            }).unwrap_or(false)
        };
        let prefer_full_ssn = |a: Option<String>, b: Option<String>| -> Option<String> {
            // If `a` is full, use it; if `a` is masked but `b` is full, use `b`;
            // otherwise fall back to whichever exists.
            if is_full_ssn(&a) { a }
            else if is_full_ssn(&b) { b }
            else { a.or(b) }
        };

        // Spouse SSN is only meaningful on a joint return — leave it blank
        // for non-joint filers so the IRS doesn't reject for an extra SSN.
        // Layer: explicit override > whichever of (profile, scan) is full.
        let spouse_ssn_final = if filing_status == "married_jointly" {
            spouse_ssn_override
                .or_else(|| prefer_full_ssn(sp_ssn_db.clone(), scan.spouse_ssn_masked.clone()))
        } else {
            None
        };

        // Pull the most-recent confirmation number for this year, if the
        // user already filed and recorded it. Lets the form double as a
        // record of the filed extension.
        let confirmation_number: Option<String> = conn.query_row(
            "SELECT confirmation_id FROM tax_extensions \
             WHERE user_id = ? AND tax_year = ? AND confirmation_id IS NOT NULL \
             ORDER BY confirmed_at DESC LIMIT 1",
            rusqlite::params![uid, year], |r| r.get(0),
        ).ok();

        // Identification fallback chain (highest priority first):
        //   1. URL override (?name=, ?address=, etc.) — user typed it now
        //   2. taxpayer_profiles row — saved earlier
        //   3. Scanned tax documents (W-2 / 1095-C / mortgage)
        let or_blank = |s: String| if s.is_empty() { None } else { Some(s) };
        let data = crate::tax_pdf::Form4868Data {
            name: name_override
                .or_else(|| or_blank(name_line))
                .or(scan.primary_name.clone()),
            address: address_override
                .or_else(|| or_blank(address_combined))
                .or(scan.address.clone()),
            city: city_override.or(city).or(scan.city.clone()),
            state: state_override.or(st).or(scan.state.clone()),
            zip: zip_override.or(zip).or(scan.zip.clone()),
            ssn: ssn_override
                .or_else(|| prefer_full_ssn(ssn_db.clone(), scan.primary_ssn_masked.clone())),
            spouse_ssn: spouse_ssn_final,
            total_tax: Some(dollars(est.total_tax)),
            total_payments: Some(dollars(est.total_payments)),
            balance_due: Some(dollars(balance_due_cents)),
            amount_paying: Some(dollars(amount_paying_cents)),
            out_of_country,
            is_1040nr_no_wages: is_1040nr,
            fy_begin,
            fy_end,
            fy_end_year,
            confirmation_number,
        };
        // Return the prepared data + flags. PDF fill (which may await
        // an IRS download) runs outside this blocking task.
        let paying = amount_paying_cents > 0;
        let balance_due_display = format!("${:.2}", balance_due_cents as f64 / 100.0);
        Ok::<_, String>((data, paying, balance_due_display))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let (data, paying, balance_due_display) = pdf_bytes; // tuple from blocking task
    let pdf_bytes = match crate::tax_pdf::fill_form_4868_with_cover(&data, paying, &balance_due_display, year).await {
        Ok(b) => b,
        Err(e) if e.contains("no Form 4868 fields matched") => {
            // Auto-recovery: hardcoded paths don't match this year's PDF
            // (likely IRS redesigned the XFA template). Run the local
            // tax AI analyzer once to derive a fresh field map, persist
            // overrides, and retry the fill.
            log::warn!("[tax] field-map miss for {}; invoking local AI to re-derive: {}", year, e);
            match analyze_form_4868_internal(&state, year).await {
                Ok(ov) => {
                    log::info!("[tax] AI overrides for {} produced {} field paths + {} address groups; retrying fill",
                        year,
                        ov.field_map.as_ref().map(|m| m.len()).unwrap_or(0),
                        ov.addresses.as_ref().map(|a| a.len()).unwrap_or(0));
                    crate::tax_pdf::fill_form_4868_with_cover(&data, paying, &balance_due_display, year)
                        .await
                        .map_err(|e2| (StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Form fill failed even after AI recovery: {}", e2)))?
                }
                Err(ai_err) => {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Form fill failed ({}); AI recovery also failed ({}). \
                                 IRS may have made an unsupported redesign.", e, ai_err)));
                }
            }
        }
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    };

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/pdf".parse().unwrap());
    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"form-4868-{}.pdf\"", year).parse().unwrap(),
    );
    Ok((headers, pdf_bytes))
}

/// AI-assisted Form 4868 maintenance.
///
/// Sends the cached (or freshly fetched) Form 4868 PDF for the requested
/// year to the local tax LLM and asks it to produce a *semantic →
/// AcroForm path* map plus an updated Where-to-File address chart. The
/// result is persisted as `~/.syntaur/forms/f4868-{year}-overrides.json`,
/// which the loader applies on top of our hardcoded defaults the next
/// time someone generates a form for that year.
///
/// Use this after the IRS publishes a new tax year, or when our default
/// field paths stop matching (the warning is logged on every fill).
/// Run the AI form analyzer for a given tax year. Returns the persisted
/// overrides on success. Used by both the manual endpoint and the
/// auto-recovery path inside handle_extension.
pub async fn analyze_form_4868_internal(
    state: &AppState, year: i64,
) -> Result<crate::tax_pdf::FormOverrides, String> {
    // 1. Get the PDF — may trigger an IRS download.
    let pdf_bytes = crate::tax_pdf::load_form_4868(year).await;
    analyze_form_4868_with_pdf(state, year, &pdf_bytes).await
}

/// Same as `analyze_form_4868_internal` but accepts pre-loaded bytes —
/// avoids a second IRS round-trip when the caller already has them.
/// Compute the union of state codes the AI returned in *complete* address
/// groups (states + both 3-line addresses present). A group with state
/// codes but missing payment addresses doesn't count toward coverage —
/// otherwise we'd think the chart was done and not re-prompt.
fn covered_states(parsed: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let addrs = match parsed.get("addresses").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return set,
    };
    let valid3 = |obj: &serde_json::Value, key: &str| -> bool {
        obj.get(key).and_then(|v| v.as_array())
            .map(|a| a.len() >= 3 && a.iter().all(|s| s.as_str().map(|t| !t.is_empty()).unwrap_or(false)))
            .unwrap_or(false)
    };
    for a in addrs {
        if !valid3(a, "with_payment") || !valid3(a, "without_payment") { continue; }
        if let Some(states) = a.get("states").and_then(|v| v.as_array()) {
            for s in states {
                if let Some(code) = s.as_str() {
                    set.insert(code.trim().to_uppercase());
                }
            }
        }
    }
    set
}

/// Single LLM call to the form analyzer. Returns the parsed JSON
/// response. Caller handles retry / completeness logic.
async fn call_form_analyzer_llm(
    state: &AppState, year: i64, field_names: &[String], pages: &[Vec<u8>], extra_hint: &str,
) -> Result<serde_json::Value, String> {
    let provider = state.config.models.providers.iter()
        .find(|(_, p)| p.base_url.contains("openrouter") || p.base_url.contains("openai") || p.base_url.contains("anthropic"))
        .or_else(|| state.config.models.providers.iter().next())
        .ok_or_else(|| "No LLM provider configured".to_string())?;
    let (_, pcfg) = provider;
    let model = if pcfg.base_url.contains("openrouter") { "google/gemini-2.0-flash-001" }
        else if pcfg.base_url.contains("anthropic") { "claude-sonnet-4-6" }
        else { "gpt-4o-mini" };
    let url = format!("{}/chat/completions", pcfg.base_url.trim_end_matches('/'));

    use base64::Engine;
    let prompt = format!(r#"You are maintaining the field-mapping table for IRS Form 4868
(Application for Automatic Extension of Time to File). I've rendered every
page of the {year} edition for you, and I'll give you the list of AcroForm
field paths the PDF declares. Match each visible labeled box on the form
to one of these semantic names:

  name             - Line 1 taxpayer name(s)
  address          - Line 1 street + apt
  city             - Line 1 city
  state            - Line 1 state (2-char USPS code)
  zip              - Line 1 ZIP code
  ssn              - Line 2 your SSN
  spouse_ssn       - Line 3 spouse's SSN (joint returns)
  total_tax        - Line 4 estimate of total tax liability
  total_payments   - Line 5 total payments
  balance_due      - Line 6 balance due
  amount_paying    - Line 7 amount paying with this extension
  confirmation_number - Page-3 "Enter confirmation number here" slot
  fy_begin         - voucher header: fiscal year beginning date
  fy_end           - voucher header: fiscal year ending date
  fy_end_year      - voucher header: 20__ fiscal year end year

And for the two checkboxes:
  out_of_country   - Line 8 (US citizen abroad)
  is_1040nr        - Line 9 (1040-NR no withheld wages)

Available AcroForm field paths on this PDF:
{field_list}

Also extract the COMPLETE "Where to File a Paper Form 4868" chart from
the last page. The chart has multiple rows grouping states by region.
For EACH row, list the USPS state codes plus the full 3-line addresses
for "with payment" and "without payment". Do not stop after one row —
the chart typically has 4-6 US groups + 1-2 foreign groups, and the
union of all state codes must cover all 50 US states + DC.{extra_hint}

Respond ONLY with this JSON shape (use null when a semantic field isn't
present on this form):
{{
  "field_map": {{ "name": "topmostSubform[0]....", "address": "...", ... }},
  "checkbox_map": {{ "out_of_country": "...", "is_1040nr": "..." }},
  "addresses": [
    {{
      "states": ["AL","FL","GA","LA","MS","NC","SC","TN","TX"],
      "with_payment": ["Internal Revenue Service", "P.O. Box 1302", "Charlotte, NC 28201-1302"],
      "without_payment": ["Department of the Treasury", "Internal Revenue Service Center", "Austin, TX 73301-0045"]
    }}
  ],
  "notes": "any caveats or changes you noticed vs prior years"
}}"#,
        year = year,
        field_list = field_names.join("\n"),
        extra_hint = extra_hint);

    let mut content_parts: Vec<serde_json::Value> = vec![serde_json::json!({"type":"text","text":prompt})];
    for png in pages {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        content_parts.push(serde_json::json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}",b64)}}));
    }
    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role":"user","content":content_parts}],
        "max_tokens": 6000,
        "temperature": 0.1,
    });
    let resp = state.client.post(&url)
        .header("Authorization", format!("Bearer {}", pcfg.api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(120))
        .send().await.map_err(|e| format!("LLM: {}", e))?;
    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("LLM HTTP {}: {}", s, &body[..body.len().min(300)]));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let cleaned = content.trim().trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    serde_json::from_str(cleaned)
        .map_err(|e| format!("LLM response parse: {} - first 300 chars: {}", e, &content[..content.len().min(300)]))
}

/// Every USPS jurisdiction code the IRS Where-to-File chart can route to.
/// Used for the AI's address-completeness self-check — the union of
/// returned state groups must cover all of these.
const ALL_US_JURISDICTIONS: &[&str] = &[
    "AL","AK","AZ","AR","CA","CO","CT","DE","FL","GA","HI","ID","IL","IN","IA",
    "KS","KY","LA","ME","MD","MA","MI","MN","MS","MO","MT","NE","NV","NH","NJ",
    "NM","NY","NC","ND","OH","OK","OR","PA","RI","SC","SD","TN","TX","UT","VT",
    "VA","WA","WV","WI","WY","DC",
];

async fn analyze_form_4868_with_pdf(
    state: &AppState, year: i64, pdf_bytes: &[u8],
) -> Result<crate::tax_pdf::FormOverrides, String> {
    let tmp_pdf = format!("/tmp/f4868-analyze-{}.pdf", year);
    std::fs::write(&tmp_pdf, pdf_bytes).map_err(|e| format!("write tmp: {}", e))?;
    let pages = convert_pdf_to_pngs(&tmp_pdf).map_err(|e| format!("render: {}", e))?;
    let field_names = crate::tax_pdf::list_field_names(pdf_bytes)
        .map_err(|e| format!("field walk: {}", e))?;

    // Up to 2 LLM attempts. First with no extra hint; if state coverage
    // is incomplete, retry with an explicit "you missed: X, Y, Z" list.
    let mut last_err: Option<String> = None;
    let mut best: Option<serde_json::Value> = None;
    let mut best_coverage: usize = 0;

    for attempt in 0..2 {
        let missing_hint = if attempt == 0 {
            String::new()
        } else if let Some(prev) = best.as_ref() {
            let covered = covered_states(prev);
            let missing: Vec<&&str> = ALL_US_JURISDICTIONS.iter().filter(|s| !covered.contains(**s)).collect();
            format!("\n\nIMPORTANT: Your previous response only covered {} of {} jurisdictions. \
                     You missed these state codes: {}. \
                     The full Where-to-File chart on the IRS PDF lists every state — please \
                     re-read it and return ALL groups. Each group covers multiple states; \
                     a typical 2025 chart has 4-6 groups for US states + 1-2 for foreign.",
                covered.len(), ALL_US_JURISDICTIONS.len(),
                missing.iter().map(|s| **s).collect::<Vec<_>>().join(", "))
        } else {
            String::new()
        };

        match call_form_analyzer_llm(state, year, &field_names, &pages, &missing_hint).await {
            Ok(parsed) => {
                let coverage = covered_states(&parsed).len();
                if coverage > best_coverage {
                    best_coverage = coverage;
                    best = Some(parsed.clone());
                }
                if coverage >= ALL_US_JURISDICTIONS.len() {
                    log::info!("[tax] AI form analysis: full state coverage on attempt {}", attempt + 1);
                    break;
                }
                log::warn!("[tax] AI form analysis attempt {}: covered {} of {} jurisdictions; retrying",
                    attempt + 1, coverage, ALL_US_JURISDICTIONS.len());
            }
            Err(e) => {
                log::warn!("[tax] AI form analysis attempt {} failed: {}", attempt + 1, e);
                last_err = Some(e);
            }
        }
    }
    let _ = std::fs::remove_file(&tmp_pdf);

    let parsed = best.ok_or_else(|| last_err.unwrap_or_else(|| "AI analysis returned no usable response".to_string()))?;

    // 4. Build + persist the overrides.
    let mut overrides = crate::tax_pdf::FormOverrides::default();
    if let Some(map) = parsed.get("field_map").and_then(|v| v.as_object()) {
        let mut hm = std::collections::HashMap::new();
        for (k, v) in map {
            if let Some(p) = v.as_str() { if !p.is_empty() { hm.insert(k.clone(), p.to_string()); } }
        }
        overrides.field_map = Some(hm);
    }
    if let Some(map) = parsed.get("checkbox_map").and_then(|v| v.as_object()) {
        let mut hm = std::collections::HashMap::new();
        for (k, v) in map {
            if let Some(p) = v.as_str() { if !p.is_empty() { hm.insert(k.clone(), p.to_string()); } }
        }
        overrides.checkbox_map = Some(hm);
    }
    if let Some(addrs) = parsed.get("addresses").and_then(|v| v.as_array()) {
        let groups: Vec<crate::tax_pdf::AddressGroup> = addrs.iter().filter_map(|a| {
            let states = a["states"].as_array()?.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
            let read3 = |k: &str| -> Option<[String; 3]> {
                let arr = a[k].as_array()?;
                if arr.len() < 3 { return None; }
                Some([arr[0].as_str()?.to_string(), arr[1].as_str()?.to_string(), arr[2].as_str()?.to_string()])
            };
            Some(crate::tax_pdf::AddressGroup {
                states,
                with_payment: read3("with_payment")?,
                without_payment: read3("without_payment")?,
            })
        }).collect();
        if !groups.is_empty() { overrides.addresses = Some(groups); }
    }
    overrides.notes = parsed.get("notes").and_then(|v| v.as_str()).map(str::to_string);
    overrides.generated_at = Some(chrono::Utc::now().to_rfc3339());
    crate::tax_pdf::save_overrides(year, &overrides)?;

    let _ = std::fs::remove_file(&tmp_pdf);
    Ok(overrides)
}

pub async fn handle_analyze_form_4868(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let year: i64 = params.get("year").and_then(|y| y.parse().ok())
        .unwrap_or_else(default_tax_year);
    let overrides = analyze_form_4868_internal(&state, year).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "year": year,
        "field_map_size": overrides.field_map.as_ref().map(|m| m.len()).unwrap_or(0),
        "checkbox_map_size": overrides.checkbox_map.as_ref().map(|m| m.len()).unwrap_or(0),
        "address_groups": overrides.addresses.as_ref().map(|a| a.len()).unwrap_or(0),
        "notes": overrides.notes,
        "saved_to": format!("~/.syntaur/forms/f4868-{}-overrides.json", year),
    })))
}


/// Generate a #10 envelope PDF (9.5" × 4.125") with the user's return
/// address top-left and the correct IRS service-center address center-right,
/// positioned per USPS DMM 202.4 so it prints directly on a #10 envelope.
///
/// Optional query params:
/// - `payment=N` (cents) — drives which IRS PO Box appears as recipient
/// - `name`, `address`, `city`, `state`, `zip` — override values from the
///   profile/scans (same semantics as the main extension endpoint)
pub async fn handle_extension_envelope(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(default_tax_year);
    let payment_override: Option<i64> = params.get("payment").and_then(|p| p.parse().ok());

    let q = |k: &str| params.get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let name_q = q("name");
    let addr_q = q("address");
    let city_q = q("city");
    let state_q = q("state");
    let zip_q = q("zip");

    let db = state.db_path.clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let filing_status: String = conn.query_row(
            "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get(0),
        ).unwrap_or_else(|_| "single".to_string());

        // Same identification merge as handle_extension, abbreviated for
        // the four fields we actually print on an envelope.
        type Row = (Option<String>, Option<String>, Option<String>, Option<String>,
                    Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
        let row: Row = conn.query_row(
            "SELECT first_name, last_name, address_line1, address_line2, city, state, zip, \
                    spouse_first, spouse_last \
             FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                    r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        ).unwrap_or_else(|_| (None, None, None, None, None, None, None, None, None));
        let (first, last, a1, a2, city, st, zip, sp_f, sp_l) = row;

        let scan = pull_scan_identity(&conn, uid, year);
        let user_disp: String = conn.query_row(
            "SELECT COALESCE(display_name, username, '') FROM users WHERE id = ?",
            rusqlite::params![uid], |r| r.get(0),
        ).unwrap_or_default();

        let primary = match (first.as_deref(), last.as_deref()) {
            (Some(f), Some(l)) if !f.is_empty() || !l.is_empty() => format!("{} {}", f, l).trim().to_string(),
            _ => scan.primary_name.clone().unwrap_or(user_disp),
        };
        let mut name_line = primary;
        if filing_status == "married_jointly" {
            let spouse = match (sp_f.as_deref(), sp_l.as_deref()) {
                (Some(f), Some(l)) if !f.is_empty() || !l.is_empty() => format!("{} {}", f, l).trim().to_string(),
                _ => scan.spouse_name.clone().unwrap_or_default(),
            };
            if !spouse.is_empty() { name_line = format!("{} & {}", name_line, spouse); }
        }
        let addr_combined = match (a1.as_deref(), a2.as_deref()) {
            (Some(a), Some(b)) if !b.is_empty() => format!("{}, {}", a, b),
            (Some(a), _) => a.to_string(), _ => String::new(),
        };
        let final_name = name_q.unwrap_or(name_line);
        let final_addr = addr_q.unwrap_or_else(|| if !addr_combined.is_empty() { addr_combined } else { scan.address.clone().unwrap_or_default() });
        let final_city = city_q.or(city).or(scan.city.clone()).unwrap_or_default();
        let final_state = state_q.or(st).or(scan.state.clone()).unwrap_or_default();
        let final_zip = zip_q.or(zip).or(scan.zip.clone()).unwrap_or_default();
        let csz = format!("{}, {} {}", final_city.trim(), final_state.trim(), final_zip.trim());
        let csz = csz.trim_matches(|c: char| c == ',' || c.is_whitespace()).to_string();

        // Decide payment vs no-payment to pick the right IRS address.
        let est = compute_tax_estimate(&conn, uid, year, &filing_status)?;
        let balance_due = std::cmp::max(est.total_tax - est.total_payments, 0);
        let amount_paying = payment_override.unwrap_or(balance_due);
        let with_payment = amount_paying > 0;
        let irs = crate::tax_pdf::irs_mailing_address(&final_state, with_payment);

        crate::tax_pdf::build_envelope_10(&final_name, &final_addr, &csz, &irs)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/pdf".parse().unwrap());
    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"form-4868-{}-envelope.pdf\"", year).parse().unwrap(),
    );
    Ok((headers, bytes))
}

/// Re-run the vision extractor on a previously-scanned tax document.
///
/// Useful when the extraction prompt has been extended (e.g., to capture
/// full SSN + employee address on W-2s) and the user wants the existing
/// docs backfilled without re-uploading the originals.
pub async fn handle_tax_doc_rescan(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(doc_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();

    // Verify the doc belongs to this user before touching it.
    let owner: Option<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Option<i64> {
            let conn = rusqlite::Connection::open(&db).ok()?;
            conn.query_row(
                "SELECT user_id FROM tax_documents WHERE id = ?",
                rusqlite::params![doc_id], |r| r.get(0),
            ).ok()
        }).await.ok().flatten()
    };
    if owner != Some(uid) {
        return Err((StatusCode::NOT_FOUND, "Document not found".into()));
    }

    classify_and_extract(&state, doc_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true, "doc_id": doc_id})))
}

/// Re-scan every W-2 and mortgage-statement doc this user has, in
/// sequence (LLM provider rate-limits make parallelism risky). Returns a
/// summary of what was rescanned.
pub async fn handle_tax_docs_rescan_all(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();

    // Gather doc IDs the new prompt is most likely to enrich (W-2 + 1095-C
    // for SSN, mortgage statement for address).
    let ids: Vec<i64> = {
        let db = state.db_path.clone();
        tokio::task::spawn_blocking(move || -> Vec<i64> {
            let conn = match rusqlite::Connection::open(&db) { Ok(c) => c, Err(_) => return vec![] };
            let mut stmt = match conn.prepare(
                "SELECT id FROM tax_documents WHERE user_id = ? AND status = 'scanned' \
                 AND doc_type IN ('w2', '1095_c', 'mortgage_statement') ORDER BY id"
            ) { Ok(s) => s, Err(_) => return vec![] };
            stmt.query_map(rusqlite::params![uid], |r| r.get::<_, i64>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default()
        }).await.unwrap_or_default()
    };

    let mut ok = 0; let mut failed: Vec<i64> = Vec::new();
    for id in &ids {
        match classify_and_extract(&state, *id).await {
            Ok(_) => ok += 1,
            Err(e) => { log::warn!("[tax] rescan doc {} failed: {}", id, e); failed.push(*id); }
        }
    }

    Ok(Json(serde_json::json!({
        "ok": true, "rescanned": ok, "failed": failed, "total": ids.len(),
    })))
}

/// Suggest taxpayer profile values mined from this user's scanned tax
/// documents (W-2, 1095-C, mortgage statement). The UI's "Auto-fill from
/// scans" button uses this to pre-populate empty profile inputs without
/// silently overwriting anything the user has already typed.
pub async fn handle_profile_suggest_from_scans(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(default_tax_year);
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let id = pull_scan_identity(&conn, uid, year);

        // Split "First Last" → first + last so we can populate the two
        // profile inputs separately.
        let split_name = |full: &Option<String>| -> (Option<String>, Option<String>) {
            match full {
                Some(n) => {
                    let trimmed = n.trim();
                    if trimmed.is_empty() { return (None, None); }
                    let mut parts = trimmed.splitn(2, char::is_whitespace);
                    let first = parts.next().map(str::to_string);
                    let last = parts.next().map(str::trim).map(str::to_string);
                    (first, last)
                }
                None => (None, None),
            }
        };
        let (first, last) = split_name(&id.primary_name);
        let (sp_first, sp_last) = split_name(&id.spouse_name);

        // Tell the UI which scanned docs sourced each value so the user can
        // decide whether to trust it.
        let mut sources: Vec<String> = Vec::new();
        if id.primary_name.is_some() { sources.push("W-2 / 1095-C employee name".into()); }
        if id.address.is_some() || id.city.is_some() {
            sources.push("Mortgage statement property address or W-2 employee address".into());
        }
        if id.primary_ssn_masked.is_some() { sources.push("W-2 SSN (last 4)".into()); }

        Ok(serde_json::json!({
            "first_name": first,
            "last_name": last,
            "ssn": id.primary_ssn_masked,
            "address_line1": id.address,
            "city": id.city,
            "state": id.state,
            "zip": id.zip,
            "spouse_first": sp_first,
            "spouse_last": sp_last,
            "spouse_ssn": id.spouse_ssn_masked,
            "sources": sources,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let doc_id_filter = params.get("document_id").and_then(|v| v.parse::<i64>().ok());
    let start = params.get("start").cloned();
    let end = params.get("end").cloned();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let mut sql = "SELECT t.id, t.document_id, t.transaction_date, t.description, t.amount_cents, \
                        t.vendor, t.insurance_type, t.is_deductible, t.status, c.name \
                        FROM statement_transactions t LEFT JOIN expense_categories c ON t.category_id = c.id \
                        WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![];
        if let Some(uid) = scope {
            sql.push_str(" AND t.user_id = ?");
            params_vec.push(Box::new(uid));
        }

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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
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

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT id, address, total_sqft, workshop_sqft, purchase_price_cents, purchase_date, \
              building_value_cents, land_value_cents, land_ratio, assessor_total_cents, assessor_land_cents, \
              annual_property_tax_cents, annual_insurance_cents, mortgage_lender, mortgage_interest_cents, \
              mortgage_principal_cents, depreciation_basis_cents, depreciation_annual_cents, notes \
              FROM property_profiles WHERE user_id = ? ORDER BY id".to_string(),
             vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>])
        } else {
            ("SELECT id, address, total_sqft, workshop_sqft, purchase_price_cents, purchase_date, \
              building_value_cents, land_value_cents, land_ratio, assessor_total_cents, assessor_land_cents, \
              annual_property_tax_cents, annual_insurance_cents, mortgage_lender, mortgage_interest_cents, \
              mortgage_principal_cents, depreciation_basis_cents, depreciation_annual_cents, notes \
              FROM property_profiles ORDER BY id".to_string(),
             vec![])
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows: Vec<serde_json::Value> = stmt.query_map(params_refs.as_slice(), |r| {
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
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or(2025);

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let start = format!("{}-01-01", year);
        let end = format!("{}-12-31", year);

        // Helper: build WHERE clause fragment for user scope
        // When scope is None (shared mode / admin), omit user_id filter
        let uid_filter = |prefix: &str| -> String {
            if let Some(uid) = scope {
                format!("{}.user_id = {} AND", prefix, uid)
            } else {
                String::new()
            }
        };
        let uid_filter_bare = || -> String {
            if let Some(uid) = scope {
                format!("user_id = {} AND", uid)
            } else {
                String::new()
            }
        };

        // Step 1: Filing Status
        // We can infer from W-2 count
        let w2_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM tax_documents WHERE {} doc_type = 'w2' AND tax_year = ? AND status = 'scanned'", uid_filter_bare()),
            rusqlite::params![year], |r| r.get(0)
        ).unwrap_or(0);

        let w2_details: Vec<serde_json::Value> = {
            let mut stmt = conn.prepare(
                &format!("SELECT issuer, extracted_fields FROM tax_documents WHERE {} doc_type = 'w2' AND tax_year = ? AND status = 'scanned'", uid_filter_bare())
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![year], |r| {
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
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE {} tax_year = ? AND category != 'Federal Withholding'", uid_filter_bare()),
            rusqlite::params![year], |r| r.get(0)
        ).unwrap_or(0);
        let withheld: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM tax_income WHERE {} tax_year = ? AND category = 'Federal Withholding'", uid_filter_bare()),
            rusqlite::params![year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 3: 1099s
        let form_1099_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM tax_documents WHERE {} doc_type LIKE '1099%' AND tax_year = ? AND status = 'scanned'", uid_filter_bare()),
            rusqlite::params![year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 4: 1098 mortgage statements
        let form_1098_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM tax_documents WHERE {} doc_type = 'mortgage_statement' AND tax_year = ? AND status = 'scanned'", uid_filter_bare()),
            rusqlite::params![year], |r| r.get(0)
        ).unwrap_or(0);

        // Step 5: Expenses & deductions
        let total_expenses: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE {} expense_date >= ? AND expense_date <= ?", uid_filter_bare()),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);
        let business_expenses: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(amount_cents), 0) FROM expenses WHERE {} entity = 'business' AND expense_date >= ? AND expense_date <= ?", uid_filter_bare()),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);
        let deductible_expenses: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(e.amount_cents), 0) FROM expenses e JOIN expense_categories c ON e.category_id = c.id \
             WHERE {} c.tax_deductible = 1 AND e.expense_date >= ? AND e.expense_date <= ?", uid_filter("e")),
            rusqlite::params![&start, &end], |r| r.get(0)
        ).unwrap_or(0);

        // Step 6: Receipt count
        let receipt_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM receipts WHERE {} 1=1", uid_filter_bare()),
            [], |r| r.get(0)
        ).unwrap_or(0);

        // Step 7: Property profile
        let has_property: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='property_profiles'",
            [], |r| r.get::<_, i64>(0)
        ).unwrap_or(0) > 0 && conn.query_row(
            &format!("SELECT COUNT(*) FROM property_profiles WHERE {} 1=1", uid_filter_bare()),
            [], |r| r.get::<_, i64>(0)
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

// ── Taxpayer Profile + Dependents ───────────────────────────────────────────

pub async fn handle_taxpayer_profile_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let sharing_mode = state.sharing_mode.read().await.clone();
    let scope = principal.scope_with_sharing(&sharing_mode);
    let db = state.db_path.clone();
    let year: i64 = params.get("year").and_then(|y| y.parse().ok()).unwrap_or_else(|| default_tax_year());

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mapper = |r: &rusqlite::Row<'_>| {
            let ssn_raw: Option<String> = r.get(2)?;
            let spouse_ssn_raw: Option<String> = r.get(14)?;
            Ok(serde_json::json!({
            "first_name": r.get::<_, Option<String>>(0)?, "last_name": r.get::<_, Option<String>>(1)?,
            "ssn": ssn_raw.clone(),
            "ssn_last4": ssn_raw.map(|s| if s.len()>=4 { format!("***-**-{}", &s[s.len()-4..]) } else { s }),
            "date_of_birth": r.get::<_, Option<String>>(3)?,
            "address_line1": r.get::<_, Option<String>>(4)?, "address_line2": r.get::<_, Option<String>>(5)?,
            "city": r.get::<_, Option<String>>(6)?, "state": r.get::<_, Option<String>>(7)?, "zip": r.get::<_, Option<String>>(8)?,
            "phone": r.get::<_, Option<String>>(9)?, "email": r.get::<_, Option<String>>(10)?,
            "filing_status": r.get::<_, String>(11)?,
            "spouse_first": r.get::<_, Option<String>>(12)?, "spouse_last": r.get::<_, Option<String>>(13)?,
            "spouse_ssn": spouse_ssn_raw.clone(),
            "spouse_ssn_last4": spouse_ssn_raw.map(|s| if s.len()>=4 { format!("***-**-{}", &s[s.len()-4..]) } else { s }),
            "spouse_dob": r.get::<_, Option<String>>(15)?,
            "occupation": r.get::<_, Option<String>>(16)?, "spouse_occupation": r.get::<_, Option<String>>(17)?,
            }))
        };
        let profile = if let Some(uid) = scope {
            conn.query_row(
                "SELECT first_name, last_name, ssn_encrypted, date_of_birth, address_line1, address_line2, \
                 city, state, zip, phone, email, filing_status, spouse_first, spouse_last, spouse_ssn_encrypted, \
                 spouse_dob, occupation, spouse_occupation FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
                rusqlite::params![uid, year], mapper,
            ).ok()
        } else {
            conn.query_row(
                "SELECT first_name, last_name, ssn_encrypted, date_of_birth, address_line1, address_line2, \
                 city, state, zip, phone, email, filing_status, spouse_first, spouse_last, spouse_ssn_encrypted, \
                 spouse_dob, occupation, spouse_occupation FROM taxpayer_profiles WHERE tax_year = ?",
                rusqlite::params![year], mapper,
            ).ok()
        };

        let (dep_sql, dep_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT id, first_name, last_name, ssn_encrypted, date_of_birth, relationship, months_lived, \
              qualifies_ctc, qualifies_odc, is_student, is_disabled FROM dependents WHERE user_id = ? AND tax_year = ?".to_string(),
             vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(year)])
        } else {
            ("SELECT id, first_name, last_name, ssn_encrypted, date_of_birth, relationship, months_lived, \
              qualifies_ctc, qualifies_odc, is_student, is_disabled FROM dependents WHERE tax_year = ?".to_string(),
             vec![Box::new(year) as Box<dyn rusqlite::types::ToSql>])
        };
        let mut dep_stmt = conn.prepare(&dep_sql).map_err(|e| e.to_string())?;
        let dep_refs: Vec<&dyn rusqlite::types::ToSql> = dep_params.iter().map(|p| p.as_ref()).collect();
        let deps: Vec<serde_json::Value> = dep_stmt.query_map(dep_refs.as_slice(), |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?, "first_name": r.get::<_, String>(1)?, "last_name": r.get::<_, String>(2)?,
                "ssn_last4": r.get::<_, Option<String>>(3)?.map(|s| if s.len()>=4 { format!("***-**-{}", &s[s.len()-4..]) } else { s }),
                "date_of_birth": r.get::<_, Option<String>>(4)?, "relationship": r.get::<_, String>(5)?,
                "months_lived": r.get::<_, i64>(6)?, "qualifies_ctc": r.get::<_, bool>(7)?,
                "is_student": r.get::<_, bool>(9)?, "is_disabled": r.get::<_, bool>(10)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let (inc_sql, inc_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) = scope {
            ("SELECT category, source, SUM(amount_cents) FROM tax_income WHERE user_id = ? AND tax_year = ? GROUP BY category, source".to_string(),
             vec![Box::new(uid) as Box<dyn rusqlite::types::ToSql>, Box::new(year)])
        } else {
            ("SELECT category, source, SUM(amount_cents) FROM tax_income WHERE tax_year = ? GROUP BY category, source".to_string(),
             vec![Box::new(year) as Box<dyn rusqlite::types::ToSql>])
        };
        let mut inc_stmt = conn.prepare(&inc_sql).map_err(|e| e.to_string())?;
        let inc_refs: Vec<&dyn rusqlite::types::ToSql> = inc_params.iter().map(|p| p.as_ref()).collect();
        let income: Vec<serde_json::Value> = inc_stmt.query_map(inc_refs.as_slice(), |r| {
            Ok(serde_json::json!({ "category": r.get::<_, String>(0)?, "source": r.get::<_, Option<String>>(1)?, "amount": cents_to_display(r.get::<_, i64>(2)?) }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        Ok(serde_json::json!({ "profile": profile, "dependents": deps, "income_sources": income, "year": year }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct TaxpayerProfileSave {
    pub token: String, pub year: Option<i64>,
    pub first_name: Option<String>, pub last_name: Option<String>, pub ssn: Option<String>,
    pub date_of_birth: Option<String>, pub address_line1: Option<String>, pub address_line2: Option<String>,
    pub city: Option<String>, pub state: Option<String>, pub zip: Option<String>,
    pub phone: Option<String>, pub email: Option<String>, pub filing_status: Option<String>,
    pub spouse_first: Option<String>, pub spouse_last: Option<String>, pub spouse_ssn: Option<String>,
    pub spouse_dob: Option<String>, pub occupation: Option<String>, pub spouse_occupation: Option<String>,
}

pub async fn handle_taxpayer_profile_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TaxpayerProfileSave>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());
    let now = chrono::Utc::now().timestamp();
    let r = req;
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO taxpayer_profiles (user_id, tax_year, first_name, last_name, ssn_encrypted, \
             date_of_birth, address_line1, address_line2, city, state, zip, phone, email, filing_status, \
             spouse_first, spouse_last, spouse_ssn_encrypted, spouse_dob, occupation, spouse_occupation, \
             created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(user_id, tax_year) DO UPDATE SET \
             first_name=COALESCE(excluded.first_name,first_name), last_name=COALESCE(excluded.last_name,last_name), \
             ssn_encrypted=COALESCE(excluded.ssn_encrypted,ssn_encrypted), date_of_birth=COALESCE(excluded.date_of_birth,date_of_birth), \
             address_line1=COALESCE(excluded.address_line1,address_line1), address_line2=COALESCE(excluded.address_line2,address_line2), \
             city=COALESCE(excluded.city,city), state=COALESCE(excluded.state,state), zip=COALESCE(excluded.zip,zip), \
             phone=COALESCE(excluded.phone,phone), email=COALESCE(excluded.email,email), \
             filing_status=COALESCE(excluded.filing_status,filing_status), \
             spouse_first=COALESCE(excluded.spouse_first,spouse_first), spouse_last=COALESCE(excluded.spouse_last,spouse_last), \
             spouse_ssn_encrypted=COALESCE(excluded.spouse_ssn_encrypted,spouse_ssn_encrypted), \
             spouse_dob=COALESCE(excluded.spouse_dob,spouse_dob), \
             occupation=COALESCE(excluded.occupation,occupation), spouse_occupation=COALESCE(excluded.spouse_occupation,spouse_occupation), \
             updated_at=excluded.updated_at",
            rusqlite::params![uid, year, r.first_name, r.last_name, r.ssn, r.date_of_birth,
                r.address_line1, r.address_line2, r.city, r.state, r.zip, r.phone, r.email,
                r.filing_status.as_deref().unwrap_or("single"),
                r.spouse_first, r.spouse_last, r.spouse_ssn, r.spouse_dob,
                r.occupation, r.spouse_occupation, now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
pub struct DependentSaveReq {
    pub token: String, pub year: Option<i64>, pub id: Option<i64>,
    pub first_name: String, pub last_name: String, pub ssn: Option<String>,
    pub date_of_birth: Option<String>, pub relationship: Option<String>,
    pub months_lived: Option<i64>, pub is_student: Option<bool>, pub is_disabled: Option<bool>,
}

pub async fn handle_dependent_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DependentSaveReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());
    let now = chrono::Utc::now().timestamp();
    let dep_id = req.id;
    let first = req.first_name.clone(); let last = req.last_name.clone();
    let ssn = req.ssn.clone(); let dob = req.date_of_birth.clone();
    let rel = req.relationship.clone().unwrap_or_else(|| "child".to_string());
    let months = req.months_lived.unwrap_or(12);
    let student = req.is_student.unwrap_or(false);
    let disabled = req.is_disabled.unwrap_or(false);

    let id = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let qualifies_ctc = dob.as_deref().map(|d| {
            let birth_year: i64 = d.split('-').next().and_then(|y| y.parse().ok()).unwrap_or(2020);
            (year - birth_year) < 17
        }).unwrap_or(true);
        if let Some(eid) = dep_id {
            conn.execute(
                "UPDATE dependents SET first_name=?, last_name=?, ssn_encrypted=?, date_of_birth=?, \
                 relationship=?, months_lived=?, qualifies_ctc=?, is_student=?, is_disabled=? WHERE id=? AND user_id=?",
                rusqlite::params![&first, &last, &ssn, &dob, &rel, months, qualifies_ctc, student, disabled, eid, uid],
            ).map_err(|e| e.to_string())?;
            Ok(eid)
        } else {
            conn.execute(
                "INSERT INTO dependents (user_id, tax_year, first_name, last_name, ssn_encrypted, date_of_birth, \
                 relationship, months_lived, qualifies_ctc, is_student, is_disabled, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![uid, year, &first, &last, &ssn, &dob, &rel, months, qualifies_ctc, student, disabled, now],
            ).map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

pub async fn handle_dependent_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(dep_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM dependents WHERE id = ? AND user_id = ?", rusqlite::params![dep_id, uid]).map_err(|e| e.to_string())?;
        Ok(())
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({ "success": true })))
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

    // Filter rules by questionnaire gates.
    // If the questionnaire isn't completed, still activate rules where the user
    // said "yes" to a specific question, plus always-on rules. Also activate
    // charitable/retirement/student_loan by default since they're common.
    let has_any_answers = !answers.as_object().map(|o| o.is_empty()).unwrap_or(true);
    let active_rules: Vec<&DeductionRule> = DEDUCTION_RULES.iter().filter(|r| {
        if r.questionnaire_gate.is_empty() { return true; }
        // If user explicitly answered this gate, use their answer
        if let Some(v) = answers.get(r.questionnaire_gate).and_then(|v| v.as_bool()) {
            return v;
        }
        // If no questionnaire at all, activate common categories
        if !has_any_answers {
            return matches!(r.questionnaire_gate, "charitable_donations" | "retirement_contributions" | "student_loan_interest");
        }
        false
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

    // Scan expenses (user's existing expense entries)
    // Exclude income/wage/tax entries — these are not deductible expenses
    {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.vendor, COALESCE(e.description, ''), e.amount_cents, e.expense_date \
             FROM expenses e LEFT JOIN expense_categories c ON e.category_id = c.id \
             WHERE e.user_id = ? AND e.expense_date >= ? AND e.expense_date <= ? \
             AND COALESCE(c.name, '') NOT LIKE '%Wages%' AND COALESCE(c.name, '') NOT LIKE '%FICA%' \
             AND COALESCE(c.name, '') NOT LIKE '%Income Tax%' AND COALESCE(c.name, '') NOT LIKE '%Withholding%' \
             AND COALESCE(c.name, '') NOT LIKE '%Revenue%' AND COALESCE(c.name, '') NOT LIKE '%Income%' \
             AND COALESCE(c.name, '') NOT LIKE '%Capital Gains%' AND COALESCE(c.name, '') NOT LIKE '%Dividend%'"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![user_id, &year_start, &year_end], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, String>(4)?))
        }).map_err(|e| e.to_string())?;

        for row in rows {
            let (id, vendor, desc, amount, date) = row.map_err(|e| e.to_string())?;
            let search_text = format!("{} {}", &vendor, &desc).to_lowercase();
            for rule in &active_rules {
                let matched: Vec<&&str> = rule.keywords.iter().filter(|kw| search_text.contains(**kw)).collect();
                if matched.is_empty() { continue; }
                let confidence = if matched.len() >= 2 { 0.8 } else { 0.5 };
                let inserted = conn.execute(
                    "INSERT OR IGNORE INTO deduction_candidates \
                     (user_id, tax_year, source_type, source_id, deduction_type, vendor, description, \
                      amount_cents, transaction_date, category_suggestion, entity_suggestion, confidence, match_rule, status, created_at) \
                     VALUES (?, ?, 'expense', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
                    rusqlite::params![
                        user_id, year, id, rule.deduction_type,
                        &vendor, &desc, amount.abs(), &date,
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

pub async fn handle_deduction_deep_scan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeductionScanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    require_tax_module(&state, principal.user_id()).await?;

    let uid = principal.user_id();
    let db = state.db_path.clone();
    let year = req.year.unwrap_or_else(|| default_tax_year());
    let start = format!("{}-01-01", year);
    let end = format!("{}-12-31", year);

    // First, run the quick scan
    let quick_result = {
        let db2 = db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            scan_for_deduction_candidates(&conn, uid, year)
        }).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
    };

    // Gather all expenses for AI analysis
    let expenses_summary = {
        let db2 = db.clone();
        let s = start.clone();
        let e = end.clone();
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare(
                "SELECT e.vendor, e.amount_cents, e.expense_date, COALESCE(c.name, 'Uncategorized'), e.entity, e.description \
                 FROM expenses e LEFT JOIN expense_categories c ON e.category_id = c.id \
                 WHERE e.user_id = ? AND e.expense_date >= ? AND e.expense_date <= ? ORDER BY e.expense_date"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![uid, &s, &e], |r| {
                Ok(format!("{}: {} ({}) [{}] - {}",
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(0)?,
                    cents_to_display(r.get::<_, i64>(1)?),
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?))
            }).map_err(|e| e.to_string())?;
            let lines: Vec<String> = rows.filter_map(|r| r.ok()).collect();
            Ok(lines.join("\n"))
        }).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
    };

    // Send to LLM for deep analysis
    let prompt = format!(
        "You are a tax deduction expert. Analyze these {} tax year expenses and identify ANY missed deductions or misclassified items.\n\n\
         EXPENSES:\n{}\n\n\
         For each potential deduction you find, respond with EXACTLY this JSON format (one per line):\n\
         {{\"vendor\":\"...\",\"amount\":\"...\",\"type\":\"medical|vehicle|home_office|software|education|charitable|professional|retirement|student_loan|hsa|health_insurance\",\"reason\":\"...\",\"category\":\"...\",\"entity\":\"business|personal\"}}\n\n\
         Look for:\n\
         - Expenses that could be business deductions but are marked personal\n\
         - Medical expenses mixed in with other categories\n\
         - Home office related utilities not properly categorized\n\
         - Software/subscriptions that could be business expenses\n\
         - Vehicle expenses for business use\n\
         - Professional services (legal, accounting) that are deductible\n\
         - Charitable contributions\n\n\
         If no missed deductions found, respond with: NONE\n\
         Only include items you are confident about. Do not guess.",
        year, expenses_summary
    );

    let chain = crate::llm::LlmChain::from_config(&state.config, "main", state.client.clone());
    let llm_response = chain.call(&[
        crate::llm::ChatMessage::system("You are a tax deduction expert. Respond only with JSON lines or NONE."),
        crate::llm::ChatMessage::user(&prompt),
    ]).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("LLM error: {}", e)))?;

    // Parse LLM suggestions and create candidates
    let now = chrono::Utc::now().timestamp();
    let mut ai_found = 0i64;
    let db2 = db.clone();

    if !llm_response.contains("NONE") {
        let candidates: Vec<serde_json::Value> = llm_response.lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    serde_json::from_str(trimmed).ok()
                } else { None }
            }).collect();

        if !candidates.is_empty() {
            let count = candidates.len();
            tokio::task::spawn_blocking(move || -> Result<i64, String> {
                let conn = rusqlite::Connection::open(&db2).map_err(|e| e.to_string())?;
                let mut inserted = 0i64;
                for (i, c) in candidates.iter().enumerate() {
                    let vendor = c.get("vendor").and_then(|v| v.as_str()).unwrap_or("Unknown");
                    let reason = c.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                    let dtype = c.get("type").and_then(|v| v.as_str()).unwrap_or("medical");
                    let category = c.get("category").and_then(|v| v.as_str()).unwrap_or("Other");
                    let entity = c.get("entity").and_then(|v| v.as_str()).unwrap_or("personal");
                    let amount_str = c.get("amount").and_then(|v| v.as_str()).unwrap_or("0");
                    let amount_cents = (amount_str.replace(['$', ','], "").parse::<f64>().unwrap_or(0.0) * 100.0) as i64;

                    let r = conn.execute(
                        "INSERT OR IGNORE INTO deduction_candidates \
                         (user_id, tax_year, source_type, source_id, deduction_type, vendor, description, \
                          amount_cents, transaction_date, category_suggestion, entity_suggestion, confidence, match_rule, status, created_at) \
                         VALUES (?, ?, 'ai_deep_scan', ?, ?, ?, ?, ?, ?, ?, ?, 0.9, ?, 'pending', ?)",
                        rusqlite::params![uid, year, i as i64, dtype, vendor, reason, amount_cents.abs(),
                            format!("{}-01-01", year), category, entity, format!("AI: {}", reason), now],
                    ).unwrap_or(0);
                    if r > 0 { inserted += 1; }
                }
                Ok(inserted)
            }).await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            ai_found = count as i64;
        }
    }

    let quick_found = quick_result.get("candidates_found").and_then(|v| v.as_i64()).unwrap_or(0);

    Ok(Json(serde_json::json!({
        "success": true,
        "documents_analyzed": 1,
        "expenses_analyzed": expenses_summary.lines().count(),
        "candidates_found": quick_found + ai_found,
        "quick_scan": quick_found,
        "ai_found": ai_found,
    })))
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

// ── Tax Credits API ─────────────────────────────────────────────────────────

pub async fn handle_credits_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025));
    let filing_status = params.get("filing_status").cloned().unwrap_or_else(|| "single".to_string());

    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let estimate = compute_tax_estimate(&conn, uid, year, &filing_status)?;
        Ok(serde_json::json!({
            "year": year,
            "child_credit": cents_to_display(estimate.child_credit),
            "eitc": cents_to_display(estimate.eitc),
            "cdcc": cents_to_display(estimate.cdcc),
            "education_credits": cents_to_display(estimate.education_credits),
            "savers_credit": cents_to_display(estimate.savers_credit),
            "energy_credits": cents_to_display(estimate.energy_credits),
            "total_credits": cents_to_display(estimate.total_credits),
            "total_credits_cents": estimate.total_credits,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_credits_eligibility(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025));
    let filing_status = params.get("filing_status").cloned().unwrap_or_else(|| "single".to_string());

    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let est = compute_tax_estimate(&conn, uid, year, &filing_status)?;
        let credits = vec![
            serde_json::json!({"name":"Child Tax Credit","amount":cents_to_display(est.child_credit),"eligible":est.child_credit>0,
                "reason": if est.child_credit>0 {"Qualifying children found"} else {"No qualifying dependents"}}),
            serde_json::json!({"name":"Earned Income Credit","amount":cents_to_display(est.eitc),"eligible":est.eitc>0,
                "reason": if est.eitc>0 {"Income qualifies"} else {"Income above EITC threshold"}}),
            serde_json::json!({"name":"Child/Dependent Care","amount":cents_to_display(est.cdcc),"eligible":est.cdcc>0,
                "reason": if est.cdcc>0 {"Care expenses found"} else {"No care expenses entered"}}),
            serde_json::json!({"name":"Education (AOTC/LLC)","amount":cents_to_display(est.education_credits),"eligible":est.education_credits>0,
                "reason": if est.education_credits>0 {"Education expenses found"} else {"No education expenses"}}),
            serde_json::json!({"name":"Saver's Credit","amount":cents_to_display(est.savers_credit),"eligible":est.savers_credit>0,
                "reason": if est.savers_credit>0 {"Retirement contributions qualify"} else {"No qualifying contributions or income too high"}}),
            serde_json::json!({"name":"Residential Energy","amount":cents_to_display(est.energy_credits),"eligible":est.energy_credits>0,
                "reason": if est.energy_credits>0 {"Energy improvements found"} else {"No energy improvements"}}),
        ];
        Ok(serde_json::json!({"credits": credits, "total": cents_to_display(est.total_credits)}))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

// ── Education / Childcare / Energy CRUD ─────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct EducationExpenseReq { token: String, tax_year: i64, student_name: String, institution: String, tuition_cents: i64, fees_cents: Option<i64>, books_cents: Option<i64> }

pub async fn handle_education_expense_create(
    State(state): State<Arc<AppState>>, Json(req): Json<EducationExpenseReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO education_expenses (user_id, tax_year, student_name, institution, tuition_cents, fees_cents, books_cents, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, req.tax_year, req.student_name, req.institution, req.tuition_cents, req.fees_cents.unwrap_or(0), req.books_cents.unwrap_or(0), now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(serde::Deserialize)]
pub struct ChildcareExpenseReq { token: String, tax_year: i64, provider_name: String, amount_cents: i64, dependent_id: Option<i64> }

pub async fn handle_childcare_expense_create(
    State(state): State<Arc<AppState>>, Json(req): Json<ChildcareExpenseReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO dependent_care_expenses (user_id, tax_year, provider_name, amount_cents, dependent_id, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, req.tax_year, req.provider_name, req.amount_cents, req.dependent_id, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(serde::Deserialize)]
pub struct EnergyImprovementReq { token: String, tax_year: i64, improvement_type: String, cost_cents: i64, qualifying_cents: Option<i64>, vendor: Option<String> }

pub async fn handle_energy_improvement_create(
    State(state): State<Arc<AppState>>, Json(req): Json<EnergyImprovementReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let qualifying = req.qualifying_cents.unwrap_or(req.cost_cents);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO energy_improvements (user_id, tax_year, improvement_type, cost_cents, qualifying_cents, vendor, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, req.tax_year, req.improvement_type, req.cost_cents, qualifying, req.vendor, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

// ── Estimated Tax Payments ──────────────────────────────────────────────────

pub async fn handle_estimated_payments_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(2025));
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, quarter, amount_cents, payment_date, payment_method, confirmation_id, status \
             FROM estimated_tax_payments WHERE user_id = ? AND tax_year = ? ORDER BY quarter"
        ).map_err(|e| e.to_string())?;
        let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"quarter":r.get::<_,i64>(1)?,
                "amount":cents_to_display(r.get::<_,i64>(2)?),"amount_cents":r.get::<_,i64>(2)?,
                "payment_date":r.get::<_,Option<String>>(3)?,"method":r.get::<_,Option<String>>(4)?,
                "confirmation":r.get::<_,Option<String>>(5)?,"status":r.get::<_,String>(6)?}))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        let deadlines = [format!("{year}-04-15"),format!("{year}-06-15"),format!("{year}-09-15"),format!("{}-01-15",year+1)];
        Ok(serde_json::json!({"payments": rows, "deadlines": deadlines, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct EstimatedPaymentReq { token: String, tax_year: i64, quarter: i64, amount_cents: i64, payment_date: Option<String>, payment_method: Option<String>, confirmation_id: Option<String> }

pub async fn handle_estimated_payment_create(
    State(state): State<Arc<AppState>>, Json(req): Json<EstimatedPaymentReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    if req.quarter < 1 || req.quarter > 4 { return Ok(Json(serde_json::json!({"ok":false,"error":"Quarter must be 1-4"}))); }
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT OR REPLACE INTO estimated_tax_payments (user_id, tax_year, quarter, amount_cents, payment_date, payment_method, confirmation_id, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'paid', ?)",
            rusqlite::params![uid, req.tax_year, req.quarter, req.amount_cents, req.payment_date, req.payment_method, req.confirmation_id, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn handle_estimated_recommended(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let fs = params.get("filing_status").cloned().unwrap_or_else(|| "single".to_string());
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let est = compute_tax_estimate(&conn, uid, year, &fs)?;
        let prior_tax = compute_tax_estimate(&conn, uid, year - 1, &fs).map(|e| e.total_tax).unwrap_or(0);
        let safe_harbor_pct = if est.agi > 15_000_000 { 11000 } else { 10000 };
        let safe_harbor = prior_tax * safe_harbor_pct / 10000;
        let required = std::cmp::min(est.total_tax * 9000 / 10000, safe_harbor);
        let paid = est.w2_withheld + est.estimated_payments;
        let remaining = std::cmp::max(required - paid, 0);
        Ok(serde_json::json!({"annual_required":cents_to_display(required),"already_paid":cents_to_display(paid),
            "remaining":cents_to_display(remaining),"per_quarter":cents_to_display(remaining/4),
            "safe_harbor":cents_to_display(safe_harbor),"projected_tax":cents_to_display(est.total_tax),
            "projected_owed":cents_to_display(est.owed)}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_tax_projection(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let fs = params.get("filing_status").cloned().unwrap_or_else(|| "single".to_string());
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let cur = compute_tax_estimate(&conn, uid, year, &fs)?;
        let month = chrono::Utc::now().format("%m").to_string().parse::<f64>().unwrap_or(6.0);
        let factor = 12.0 / month;
        let proj_income = (cur.gross_income as f64 * factor) as i64;
        let proj_tax = (cur.total_tax as f64 * factor) as i64;
        let mut cmp = serde_json::json!({});
        if let Ok(p) = compute_tax_estimate(&conn, uid, year - 1, &fs) {
            cmp = serde_json::json!({"prior_income":cents_to_display(p.gross_income),"prior_tax":cents_to_display(p.total_tax),
                "prior_rate":format!("{:.1}%",p.effective_rate),"income_change":cents_to_display(proj_income-p.gross_income),
                "tax_change":cents_to_display(proj_tax-p.total_tax)});
        }
        Ok(serde_json::json!({"year":year,"ytd_income":cents_to_display(cur.gross_income),"projected_income":cents_to_display(proj_income),
            "ytd_tax":cents_to_display(cur.total_tax),"projected_tax":cents_to_display(proj_tax),
            "effective_rate":format!("{:.1}%",cur.effective_rate),"owed":cents_to_display(cur.owed),"comparison":cmp}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

// ── MACRS Depreciation Engine ───────────────────────────────────────────────

/// MACRS GDS half-year convention percentages (in basis points).
/// Index 0 = year 1, etc. Source: IRS Publication 946 Table A-1 through A-6.
fn macrs_rates(life_years: i64) -> Vec<i64> {
    match life_years {
        3 => vec![3333, 4445, 1481, 741],
        5 => vec![2000, 3200, 1920, 1152, 1152, 576],
        7 => vec![1429, 2449, 1749, 1249, 893, 892, 893, 446],
        10 => vec![1000, 1800, 1440, 1152, 922, 737, 655, 655, 656, 655, 328],
        15 => vec![500, 950, 855, 770, 693, 623, 590, 590, 591, 590, 591, 590, 591, 590, 591, 295],
        20 => vec![375, 722, 668, 618, 571, 528, 489, 452, 447, 447, 446, 447, 446, 447, 446, 447, 446, 447, 446, 447, 223],
        // 27.5 year residential (mid-month, simplified to monthly fraction)
        275 | 27 => {
            let mut rates = vec![3636; 28]; // ~3.636% per year
            rates[0] = 3485; // first year (mid-month)
            rates[27] = 1515; // last partial year
            rates
        }
        // 39 year non-residential (mid-month, simplified)
        39 | 390 => {
            let mut rates = vec![2564; 40]; // ~2.564% per year
            rates[0] = 2461; // first year
            rates[39] = 1068; // last partial year
            rates
        }
        _ => vec![10000], // fallback: expense fully in year 1
    }
}

/// Bonus depreciation percentage for the placed-in-service year (2022-2027 schedule).
fn bonus_depreciation_pct(year: i64) -> i64 {
    match year {
        ..=2022 => 10000, // 100%
        2023 => 8000,     // 80%
        2024 => 6000,     // 60%
        2025 => 4000,     // 40%
        2026 => 2000,     // 20%
        _ => 0,           // 0% after 2026
    }
}

/// Section 179 annual limit (2025: $1,250,000; phase-out at $3,130,000).
fn section_179_limit(year: i64) -> (i64, i64) {
    // Returns (max_deduction_cents, phase_out_start_cents)
    match year {
        2025 => (125_000_000, 313_000_000),
        2026 => (130_000_000, 320_000_000), // estimated
        _ => (125_000_000, 313_000_000),    // fallback to 2025
    }
}

/// Compute current-year depreciation for a single asset.
fn compute_asset_depreciation(
    cost_basis_cents: i64,
    section_179: i64,
    bonus_depr: i64,
    prior_depr: i64,
    macrs_life: i64,
    placed_year: i64,
    current_year: i64,
    business_use_pct: i64,
) -> i64 {
    let depreciable_basis = cost_basis_cents - section_179 - bonus_depr;
    if depreciable_basis <= 0 { return 0; }

    let year_index = (current_year - placed_year) as usize;
    let rates = macrs_rates(macrs_life);
    if year_index >= rates.len() { return 0; }

    let raw = depreciable_basis * rates[year_index] / 10000;
    let accumulated = prior_depr + raw;
    let max_remaining = depreciable_basis - prior_depr;
    let capped = std::cmp::min(raw, max_remaining).max(0);

    // Apply business use percentage
    capped * business_use_pct / 100
}

/// Standard mileage rate per mile in cents (2025: $0.70/mile).
fn standard_mileage_rate(year: i64) -> i64 {
    match year {
        2025 => 70,  // $0.70
        2024 => 67,  // $0.67
        2023 => 66,  // $0.655 rounded
        _ => 70,     // default to 2025
    }
}

// ── Depreciation API ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct AddAssetReq {
    token: String,
    description: String,
    asset_class: String,
    macrs_life_years: i64,
    cost_basis_cents: i64,
    placed_in_service: String,
    business_use_pct: Option<i64>,
    section_179_cents: Option<i64>,
    use_bonus: Option<bool>,
    is_vehicle: Option<bool>,
}

pub async fn handle_asset_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddAssetReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    let placed_year: i64 = req.placed_in_service.split('-').next()
        .and_then(|s| s.parse().ok()).unwrap_or(2025);
    let biz_pct = req.business_use_pct.unwrap_or(100).clamp(0, 100);
    let s179 = req.section_179_cents.unwrap_or(0);
    let bonus = if req.use_bonus.unwrap_or(false) {
        let pct = bonus_depreciation_pct(placed_year);
        (req.cost_basis_cents - s179) * pct / 10000
    } else { 0 };

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO depreciable_assets (user_id, description, asset_class, macrs_life_years, \
             convention, cost_basis_cents, placed_in_service, business_use_pct, section_179_cents, \
             bonus_depr_cents, is_vehicle, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'half_year', ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            rusqlite::params![uid, req.description, req.asset_class, req.macrs_life_years,
                req.cost_basis_cents, req.placed_in_service, biz_pct, s179, bonus,
                req.is_vehicle.unwrap_or(false) as i64, now, now],
        ).map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();

        // Generate depreciation schedule for all years
        let depreciable = req.cost_basis_cents - s179 - bonus;
        if depreciable > 0 {
            let rates = macrs_rates(req.macrs_life_years);
            let mut accumulated = 0i64;
            for (i, rate) in rates.iter().enumerate() {
                let year = placed_year + i as i64;
                let raw = depreciable * rate / 10000;
                let remaining = depreciable - accumulated;
                let depr = std::cmp::min(raw, remaining).max(0) * biz_pct / 100;
                accumulated += depr;
                conn.execute(
                    "INSERT INTO depreciation_schedule (asset_id, tax_year, depreciation_cents, method, accumulated_cents, remaining_cents) \
                     VALUES (?, ?, ?, 'MACRS_GDS', ?, ?)",
                    rusqlite::params![id, year, depr, accumulated, depreciable - accumulated],
                ).map_err(|e| e.to_string())?;
            }
        }

        // First-year total: section 179 + bonus + MACRS year 1
        let year1_macrs = if depreciable > 0 {
            let rates = macrs_rates(req.macrs_life_years);
            depreciable * rates[0] / 10000 * biz_pct / 100
        } else { 0 };
        let first_year_total = s179 + bonus + year1_macrs;

        Ok(serde_json::json!({
            "ok": true, "asset_id": id,
            "section_179": cents_to_display(s179),
            "bonus_depreciation": cents_to_display(bonus),
            "first_year_macrs": cents_to_display(year1_macrs),
            "first_year_total": cents_to_display(first_year_total),
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_asset_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT a.id, a.description, a.asset_class, a.macrs_life_years, a.cost_basis_cents, \
             a.placed_in_service, a.business_use_pct, a.section_179_cents, a.bonus_depr_cents, \
             a.is_vehicle, a.status, \
             COALESCE((SELECT depreciation_cents FROM depreciation_schedule WHERE asset_id = a.id AND tax_year = ?), 0), \
             COALESCE((SELECT accumulated_cents FROM depreciation_schedule WHERE asset_id = a.id AND tax_year = ?), 0), \
             COALESCE((SELECT remaining_cents FROM depreciation_schedule WHERE asset_id = a.id AND tax_year = ?), 0) \
             FROM depreciable_assets a WHERE a.user_id = ? AND a.status = 'active' ORDER BY a.placed_in_service"
        ).map_err(|e| e.to_string())?;
        let assets: Vec<serde_json::Value> = stmt.query_map(
            rusqlite::params![year, year, year, uid], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_,i64>(0)?, "description": r.get::<_,String>(1)?,
                "asset_class": r.get::<_,String>(2)?, "macrs_life": r.get::<_,i64>(3)?,
                "cost_basis": cents_to_display(r.get::<_,i64>(4)?),
                "placed_in_service": r.get::<_,String>(5)?,
                "business_use_pct": r.get::<_,i64>(6)?,
                "section_179": cents_to_display(r.get::<_,i64>(7)?),
                "bonus_depr": cents_to_display(r.get::<_,i64>(8)?),
                "is_vehicle": r.get::<_,i64>(9)? != 0,
                "current_year_depr": cents_to_display(r.get::<_,i64>(11)?),
                "accumulated_depr": cents_to_display(r.get::<_,i64>(12)?),
                "remaining_basis": cents_to_display(r.get::<_,i64>(13)?),
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let total_current_year: i64 = assets.iter()
            .filter_map(|a| a.get("current_year_depr").and_then(|v| v.as_str())
                .and_then(|s| s.replace(['$',','], "").parse::<f64>().ok())
                .map(|f| (f * 100.0) as i64))
            .sum();

        Ok(serde_json::json!({
            "assets": assets,
            "total_current_year": cents_to_display(total_current_year),
            "year": year,
        }))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_depreciation_schedule(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(asset_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT tax_year, depreciation_cents, method, accumulated_cents, remaining_cents \
             FROM depreciation_schedule WHERE asset_id = ? ORDER BY tax_year"
        ).map_err(|e| e.to_string())?;
        let schedule: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![asset_id], |r| {
            Ok(serde_json::json!({
                "year": r.get::<_,i64>(0)?, "depreciation": cents_to_display(r.get::<_,i64>(1)?),
                "method": r.get::<_,String>(2)?, "accumulated": cents_to_display(r.get::<_,i64>(3)?),
                "remaining": cents_to_display(r.get::<_,i64>(4)?),
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"schedule": schedule}))
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct VehicleUsageReq {
    token: String,
    tax_year: i64,
    vehicle_description: String,
    total_miles: i64,
    business_miles: i64,
    commuting_miles: Option<i64>,
    actual_expenses_cents: Option<i64>,
}

pub async fn handle_vehicle_usage_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VehicleUsageReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    let rate = standard_mileage_rate(req.tax_year);
    let standard_deduction = req.business_miles * rate;
    let actual = req.actual_expenses_cents.unwrap_or(0);
    let method = if actual > standard_deduction && actual > 0 { "actual" } else { "standard" };
    let deduction = if method == "actual" { actual } else { standard_deduction };

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO vehicle_usage (user_id, tax_year, vehicle_description, \
             total_miles, business_miles, commuting_miles, personal_miles, standard_rate_cents, \
             actual_expenses_cents, method_used, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, req.tax_year, req.vehicle_description, req.total_miles,
                req.business_miles, req.commuting_miles.unwrap_or(0),
                req.total_miles - req.business_miles - req.commuting_miles.unwrap_or(0),
                rate, actual, method, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "standard_deduction": cents_to_display(standard_deduction),
        "actual_deduction": cents_to_display(actual),
        "method_used": method,
        "deduction": cents_to_display(deduction),
        "rate_per_mile": format!("${:.2}", rate as f64 / 100.0),
    })))
}

// ── Investment Tax Engine ───────────────────────────────────────────────────

/// Detect wash sales: any loss sale where the same symbol was purchased
/// within 30 days before or after the sale date.
fn detect_wash_sales(conn: &rusqlite::Connection, user_id: i64, tax_year: i64) -> Vec<serde_json::Value> {
    let start = format!("{}-01-01", tax_year);
    let end = format!("{}-12-31", tax_year);
    let mut results = Vec::new();

    // Find all loss dispositions for the year
    let mut stmt = match conn.prepare(
        "SELECT d.id, d.lot_id, d.sell_date, d.gain_loss_cents, l.symbol, d.quantity_sold \
         FROM lot_dispositions d JOIN tax_lots l ON l.id = d.lot_id \
         WHERE d.user_id = ? AND d.sell_date >= ? AND d.sell_date <= ? AND d.gain_loss_cents < 0"
    ) { Ok(s) => s, Err(_) => return results };

    let losses: Vec<(i64, i64, String, i64, String, f64)> = stmt.query_map(
        rusqlite::params![user_id, &start, &end], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_,String>(2)?, r.get(3)?, r.get::<_,String>(4)?, r.get(5)?))
    }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default();

    for (disp_id, _lot_id, sell_date, loss, symbol, _qty) in &losses {
        // Check for purchases of same symbol within 30 days
        let mut check = match conn.prepare(
            "SELECT id, acquisition_date, quantity FROM tax_lots \
             WHERE user_id = ? AND symbol = ? AND status = 'open' \
             AND julianday(acquisition_date) BETWEEN julianday(?) - 30 AND julianday(?) + 30"
        ) { Ok(s) => s, Err(_) => continue };

        let replacements: Vec<(i64, String)> = check.query_map(
            rusqlite::params![user_id, symbol, sell_date, sell_date], |r| {
            Ok((r.get(0)?, r.get::<_,String>(1)?))
        }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default();

        for (repl_lot_id, repl_date) in &replacements {
            results.push(serde_json::json!({
                "disposition_id": disp_id,
                "sell_date": sell_date,
                "symbol": symbol,
                "loss_cents": loss.abs(),
                "loss": cents_to_display(loss.abs()),
                "replacement_lot_id": repl_lot_id,
                "replacement_date": repl_date,
                "disallowed": true,
            }));
        }
    }
    results
}

/// Generate Form 8949 data from lot dispositions.
fn generate_form_8949(conn: &rusqlite::Connection, user_id: i64, tax_year: i64) -> serde_json::Value {
    let start = format!("{}-01-01", tax_year);
    let end = format!("{}-12-31", tax_year);

    let mut stmt = match conn.prepare(
        "SELECT l.symbol, l.acquisition_date, d.sell_date, d.proceeds_cents, d.cost_basis_cents, \
         d.wash_sale_adj_cents, d.gain_loss_cents, d.holding_period, d.form_8949_code \
         FROM lot_dispositions d JOIN tax_lots l ON l.id = d.lot_id \
         WHERE d.user_id = ? AND d.sell_date >= ? AND d.sell_date <= ? ORDER BY d.sell_date"
    ) { Ok(s) => s, Err(_) => return serde_json::json!({"short_term": [], "long_term": []}) };

    let mut short_term = Vec::new();
    let mut long_term = Vec::new();
    let mut st_total = 0i64;
    let mut lt_total = 0i64;

    let rows: Vec<_> = stmt.query_map(rusqlite::params![user_id, &start, &end], |r| {
        Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?,
            r.get::<_,i64>(3)?, r.get::<_,i64>(4)?, r.get::<_,i64>(5)?,
            r.get::<_,i64>(6)?, r.get::<_,String>(7)?, r.get::<_,String>(8)?))
    }).ok().map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default();

    for (sym, acq, sell, proceeds, basis, wash_adj, gain, period, code) in rows {
        let entry = serde_json::json!({
            "symbol": sym, "acquired": acq, "sold": sell,
            "proceeds": cents_to_display(proceeds), "basis": cents_to_display(basis),
            "wash_sale_adj": cents_to_display(wash_adj),
            "gain_loss": cents_to_display(gain), "code": code,
        });
        if period == "short" {
            st_total += gain;
            short_term.push(entry);
        } else {
            lt_total += gain;
            long_term.push(entry);
        }
    }

    serde_json::json!({
        "short_term": short_term, "long_term": long_term,
        "short_term_total": cents_to_display(st_total),
        "long_term_total": cents_to_display(lt_total),
        "net_gain_loss": cents_to_display(st_total + lt_total),
    })
}

// ── Investment API endpoints ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct AddLotReq {
    token: String,
    symbol: String,
    asset_type: Option<String>,
    quantity: f64,
    cost_per_unit_cents: i64,
    acquisition_date: String,
    acquisition_method: Option<String>,
    broker: Option<String>,
}

pub async fn handle_lot_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddLotReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let total_basis = (req.quantity * req.cost_per_unit_cents as f64) as i64;

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO tax_lots (user_id, symbol, asset_type, quantity, cost_per_unit_cents, \
             acquisition_date, acquisition_method, broker, adjusted_basis_cents, status, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?)",
            rusqlite::params![uid, req.symbol, req.asset_type.as_deref().unwrap_or("stock"),
                req.quantity, req.cost_per_unit_cents, req.acquisition_date,
                req.acquisition_method.as_deref().unwrap_or("purchase"),
                req.broker, total_basis, now],
        ).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"ok": true, "lot_id": conn.last_insert_rowid(),
            "total_basis": cents_to_display(total_basis)}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct SellLotReq {
    token: String,
    lot_id: i64,
    sell_date: String,
    quantity_sold: f64,
    proceeds_cents: i64,
}

pub async fn handle_lot_sell(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SellLotReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Get the lot
        let (symbol, qty, cpu, acq_date): (String, f64, i64, String) = conn.query_row(
            "SELECT symbol, quantity, cost_per_unit_cents, acquisition_date FROM tax_lots WHERE id = ? AND user_id = ?",
            rusqlite::params![req.lot_id, uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_,String>(3)?)),
        ).map_err(|e| format!("lot not found: {e}"))?;

        let cost_basis = (req.quantity_sold * cpu as f64) as i64;
        let gain_loss = req.proceeds_cents - cost_basis;

        // Determine holding period
        let holding = if days_between(&acq_date, &req.sell_date) > 365 { "long" } else { "short" };
        let code = if holding == "short" { "A" } else { "D" }; // Box A=short reported, D=long reported

        conn.execute(
            "INSERT INTO lot_dispositions (lot_id, user_id, sell_date, quantity_sold, proceeds_cents, \
             cost_basis_cents, gain_loss_cents, holding_period, form_8949_code, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![req.lot_id, uid, req.sell_date, req.quantity_sold,
                req.proceeds_cents, cost_basis, gain_loss, holding, code, now],
        ).map_err(|e| e.to_string())?;

        // Update lot status if fully sold
        let remaining = qty - req.quantity_sold;
        if remaining <= 0.001 {
            conn.execute("UPDATE tax_lots SET status = 'closed' WHERE id = ?",
                rusqlite::params![req.lot_id]).map_err(|e| e.to_string())?;
        } else {
            conn.execute("UPDATE tax_lots SET quantity = ? WHERE id = ?",
                rusqlite::params![remaining, req.lot_id]).map_err(|e| e.to_string())?;
        }

        Ok(serde_json::json!({
            "ok": true, "symbol": symbol,
            "proceeds": cents_to_display(req.proceeds_cents),
            "cost_basis": cents_to_display(cost_basis),
            "gain_loss": cents_to_display(gain_loss),
            "holding_period": holding,
            "type": if gain_loss >= 0 { "gain" } else { "loss" },
        }))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_lots_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let status = params.get("status").cloned().unwrap_or_else(|| "open".to_string());
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, symbol, asset_type, quantity, cost_per_unit_cents, acquisition_date, \
             acquisition_method, broker, adjusted_basis_cents, wash_sale_adj_cents, status \
             FROM tax_lots WHERE user_id = ? AND status = ? ORDER BY symbol, acquisition_date"
        ).map_err(|e| e.to_string())?;
        let lots: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, &status], |r| {
            let qty: f64 = r.get(3)?;
            let cpu: i64 = r.get(4)?;
            Ok(serde_json::json!({
                "id": r.get::<_,i64>(0)?, "symbol": r.get::<_,String>(1)?,
                "asset_type": r.get::<_,String>(2)?, "quantity": qty,
                "cost_per_unit": cents_to_display(cpu),
                "total_basis": cents_to_display((qty * cpu as f64) as i64),
                "acquisition_date": r.get::<_,String>(5)?,
                "method": r.get::<_,String>(6)?, "broker": r.get::<_,Option<String>>(7)?,
                "wash_sale_adj": cents_to_display(r.get::<_,i64>(9)?),
                "status": r.get::<_,String>(10)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"lots": lots}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_wash_sales(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let matches = detect_wash_sales(&conn, uid, year);
        Ok(serde_json::json!({"wash_sales": matches, "count": matches.len(), "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_form_8949(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        Ok(generate_form_8949(&conn, uid, year))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct K1IncomeReq {
    token: String,
    tax_year: i64,
    entity_name: String,
    entity_type: Option<String>,
    ordinary_cents: Option<i64>,
    rental_cents: Option<i64>,
    interest_cents: Option<i64>,
    dividend_cents: Option<i64>,
    capital_gain_cents: Option<i64>,
    section_179_cents: Option<i64>,
    se_income_cents: Option<i64>,
}

pub async fn handle_k1_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<K1IncomeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO k1_income (user_id, tax_year, entity_name, entity_type, ordinary_cents, \
             rental_cents, interest_cents, dividend_cents, capital_gain_cents, section_179_cents, \
             se_income_cents, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![uid, req.tax_year, req.entity_name,
                req.entity_type.as_deref().unwrap_or("partnership"),
                req.ordinary_cents.unwrap_or(0), req.rental_cents.unwrap_or(0),
                req.interest_cents.unwrap_or(0), req.dividend_cents.unwrap_or(0),
                req.capital_gain_cents.unwrap_or(0), req.section_179_cents.unwrap_or(0),
                req.se_income_cents.unwrap_or(0), now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn handle_k1_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, entity_name, entity_type, ordinary_cents, rental_cents, interest_cents, \
             dividend_cents, capital_gain_cents, section_179_cents, se_income_cents \
             FROM k1_income WHERE user_id = ? AND tax_year = ? ORDER BY entity_name"
        ).map_err(|e| e.to_string())?;
        let k1s: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_,i64>(0)?, "entity": r.get::<_,String>(1)?,
                "type": r.get::<_,String>(2)?,
                "ordinary": cents_to_display(r.get::<_,i64>(3)?),
                "rental": cents_to_display(r.get::<_,i64>(4)?),
                "interest": cents_to_display(r.get::<_,i64>(5)?),
                "dividends": cents_to_display(r.get::<_,i64>(6)?),
                "capital_gains": cents_to_display(r.get::<_,i64>(7)?),
                "section_179": cents_to_display(r.get::<_,i64>(8)?),
                "se_income": cents_to_display(r.get::<_,i64>(9)?),
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"k1s": k1s, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_capital_gains_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let start = format!("{year}-01-01");
        let end = format!("{year}-12-31");

        let st_gains: i64 = conn.query_row(
            "SELECT COALESCE(SUM(gain_loss_cents),0) FROM lot_dispositions WHERE user_id=? AND sell_date>=? AND sell_date<=? AND holding_period='short' AND gain_loss_cents>0",
            rusqlite::params![uid, &start, &end], |r| r.get(0)).unwrap_or(0);
        let st_losses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(gain_loss_cents),0) FROM lot_dispositions WHERE user_id=? AND sell_date>=? AND sell_date<=? AND holding_period='short' AND gain_loss_cents<0",
            rusqlite::params![uid, &start, &end], |r| r.get(0)).unwrap_or(0);
        let lt_gains: i64 = conn.query_row(
            "SELECT COALESCE(SUM(gain_loss_cents),0) FROM lot_dispositions WHERE user_id=? AND sell_date>=? AND sell_date<=? AND holding_period='long' AND gain_loss_cents>0",
            rusqlite::params![uid, &start, &end], |r| r.get(0)).unwrap_or(0);
        let lt_losses: i64 = conn.query_row(
            "SELECT COALESCE(SUM(gain_loss_cents),0) FROM lot_dispositions WHERE user_id=? AND sell_date>=? AND sell_date<=? AND holding_period='long' AND gain_loss_cents<0",
            rusqlite::params![uid, &start, &end], |r| r.get(0)).unwrap_or(0);

        let net = st_gains + st_losses + lt_gains + lt_losses;
        // Capital loss limitation: $3,000/year
        let loss_limit = -300_000i64;
        let usable = if net < loss_limit { loss_limit } else { net };
        let carryforward = if net < loss_limit { net - loss_limit } else { 0 };

        let wash_sales = detect_wash_sales(&conn, uid, year);

        Ok(serde_json::json!({
            "year": year,
            "short_term_gains": cents_to_display(st_gains),
            "short_term_losses": cents_to_display(st_losses.abs()),
            "long_term_gains": cents_to_display(lt_gains),
            "long_term_losses": cents_to_display(lt_losses.abs()),
            "net_gain_loss": cents_to_display(net),
            "usable_loss": cents_to_display(usable),
            "carryforward_loss": cents_to_display(carryforward.abs()),
            "wash_sale_count": wash_sales.len(),
        }))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

/// Helper: approximate days between two YYYY-MM-DD dates.
fn days_between(d1: &str, d2: &str) -> i64 {
    let parse = |s: &str| -> i64 {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 { return 0; }
        let y: i64 = parts[0].parse().unwrap_or(0);
        let m: i64 = parts[1].parse().unwrap_or(1);
        let d: i64 = parts[2].parse().unwrap_or(1);
        y * 365 + m * 30 + d // rough approximation
    };
    (parse(d2) - parse(d1)).abs()
}

// ── AI Tax Advisor ──────────────────────────────────────────────────────────

/// Build a comprehensive tax context summary for injection into the agent's
/// system prompt. This gives the AI full awareness of the user's financial
/// situation so it can answer tax questions with real numbers.
pub fn build_tax_context(conn: &rusqlite::Connection, user_id: i64, year: i64) -> String {
    let fs_row: Option<String> = conn.query_row(
        "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).ok();
    let filing_status = fs_row.as_deref().unwrap_or("single");

    let estimate = match compute_tax_estimate(conn, user_id, year, filing_status) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };

    let deps: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dependents WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);

    let receipts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM receipts WHERE user_id = ?",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);

    let docs: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tax_documents WHERE user_id = ? AND tax_year = ? AND status = 'scanned'",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or(0);

    let lots: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tax_lots WHERE user_id = ? AND status = 'open'",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);

    let assets: i64 = conn.query_row(
        "SELECT COUNT(*) FROM depreciable_assets WHERE user_id = ? AND status = 'active'",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);

    format!(
        "# User's Tax Situation ({year})\n\
         Filing status: {filing_status} | Dependents: {deps}\n\
         Gross income: {gi} | W-2: {w2} | Self-employment: {se}\n\
         Business deductions: {bd} | AGI: {agi}\n\
         Deduction: {dt} ({dd})\n\
         Taxable income: {ti}\n\
         Tax: {tt} (ordinary {ot} + LTCG {lt} + SE {st})\n\
         Credits: {tc} (CTC {ctc}, EITC {eitc}, CDCC {cdcc}, education {edu}, saver's {sav}, energy {eng})\n\
         Payments: {tp} (W-2 withheld {wh} + estimated {ep})\n\
         {owed_label}: {owed}\n\
         Effective rate: {er}\n\
         Documents: {docs} scanned | Receipts: {receipts} | Open lots: {lots} | Assets: {assets}",
        gi=cents_to_display(estimate.gross_income), w2=cents_to_display(estimate.w2_income),
        se=cents_to_display(estimate.se_income), bd=cents_to_display(estimate.biz_deductions),
        agi=cents_to_display(estimate.agi), dt=estimate.deduction_type,
        dd=cents_to_display(estimate.deduction_used), ti=cents_to_display(estimate.taxable_income),
        tt=cents_to_display(estimate.total_tax), ot=cents_to_display(estimate.ordinary_tax),
        lt=cents_to_display(estimate.ltcg_tax), st=cents_to_display(estimate.se_tax),
        tc=cents_to_display(estimate.total_credits), ctc=cents_to_display(estimate.child_credit),
        eitc=cents_to_display(estimate.eitc), cdcc=cents_to_display(estimate.cdcc),
        edu=cents_to_display(estimate.education_credits), sav=cents_to_display(estimate.savers_credit),
        eng=cents_to_display(estimate.energy_credits),
        tp=cents_to_display(estimate.total_payments), wh=cents_to_display(estimate.w2_withheld),
        ep=cents_to_display(estimate.estimated_payments),
        owed_label=if estimate.owed >= 0 { "Owed" } else { "Refund" },
        owed=cents_to_display(estimate.owed.abs()),
        er=format!("{:.1}%", estimate.effective_rate),
    )
}

/// Compute audit risk factors for a user's return.
pub fn compute_audit_risk(conn: &rusqlite::Connection, user_id: i64, year: i64) -> Vec<serde_json::Value> {
    let fs: String = conn.query_row(
        "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or_else(|_| "single".to_string());

    let estimate = match compute_tax_estimate(conn, user_id, year, &fs) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut factors = Vec::new();

    // High income (AGI > $200K increases audit rate ~4x)
    if estimate.agi > 20_000_000 {
        factors.push(serde_json::json!({
            "factor": "high_income", "risk": "medium",
            "description": format!("AGI of {} is above $200K — IRS audit rate increases significantly at this level", cents_to_display(estimate.agi)),
        }));
    }

    // Schedule C with high deduction ratio
    if estimate.se_income > 0 && estimate.biz_deductions > 0 {
        let ratio = estimate.biz_deductions as f64 / estimate.se_income as f64;
        if ratio > 0.80 {
            factors.push(serde_json::json!({
                "factor": "high_sch_c_deductions", "risk": "high",
                "description": format!("Business deductions are {:.0}% of SE income — ratios above 80% draw IRS attention", ratio * 100.0),
            }));
        }
    }

    // Home office deduction
    let home_office: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents),0) FROM expenses e JOIN expense_categories c ON e.category_id=c.id \
         WHERE e.user_id=? AND c.name LIKE '%Home Office%' AND e.entity='business'",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);
    if home_office > 0 {
        factors.push(serde_json::json!({
            "factor": "home_office", "risk": "low",
            "description": format!("Home office deduction of {} — ensure exclusive business use documentation", cents_to_display(home_office)),
        }));
    }

    // Large charitable deductions relative to income
    let charitable: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents),0) FROM expenses e JOIN expense_categories c ON e.category_id=c.id \
         WHERE e.user_id=? AND (c.name LIKE '%Charit%' OR c.name LIKE '%Donat%') AND e.entity='personal'",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);
    if estimate.agi > 0 && charitable > 0 {
        let pct = charitable as f64 / estimate.agi as f64;
        if pct > 0.05 {
            factors.push(serde_json::json!({
                "factor": "large_charitable", "risk": if pct > 0.10 { "medium" } else { "low" },
                "description": format!("Charitable deductions are {:.1}% of AGI ({}) — keep receipts for all donations over $250", pct * 100.0, cents_to_display(charitable)),
            }));
        }
    }

    // Unreported income risk (large cash business)
    if estimate.se_income > 10_000_000 {
        let has_1099: bool = conn.query_row(
            "SELECT COUNT(*) FROM tax_documents WHERE user_id=? AND tax_year=? AND doc_type LIKE '1099%'",
            rusqlite::params![user_id, year], |r| r.get::<_,i64>(0),
        ).unwrap_or(0) > 0;
        if !has_1099 {
            factors.push(serde_json::json!({
                "factor": "no_1099_docs", "risk": "medium",
                "description": "Self-employment income over $100K with no 1099 documents uploaded — ensure all income is reported",
            }));
        }
    }

    // Zero tax liability
    if estimate.total_tax == 0 && estimate.gross_income > 5_000_000 {
        factors.push(serde_json::json!({
            "factor": "zero_tax", "risk": "medium",
            "description": format!("Zero tax on {} income — while legal, this pattern may attract scrutiny", cents_to_display(estimate.gross_income)),
        }));
    }

    // Overall risk score
    let risk_score: usize = factors.iter().map(|f| {
        match f.get("risk").and_then(|r| r.as_str()).unwrap_or("low") {
            "high" => 3, "medium" => 2, "low" => 1, _ => 0,
        }
    }).sum();
    let overall = if risk_score >= 6 { "high" } else if risk_score >= 3 { "medium" } else { "low" };

    // Prepend overall summary
    let mut result = vec![serde_json::json!({
        "factor": "overall", "risk": overall,
        "description": format!("{} risk factors identified (score: {})", factors.len(), risk_score),
    })];
    result.extend(factors);
    result
}

/// Generate optimization insights based on the user's current tax data.
pub fn generate_tax_insights(conn: &rusqlite::Connection, user_id: i64, year: i64) -> Vec<serde_json::Value> {
    let fs: String = conn.query_row(
        "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year], |r| r.get(0),
    ).unwrap_or_else(|_| "single".to_string());

    let estimate = match compute_tax_estimate(conn, user_id, year, &fs) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut insights = Vec::new();

    // Itemized vs standard comparison
    if estimate.deduction_type == "Standard" && estimate.itemized_deduction > 0 {
        let gap = estimate.standard_deduction - estimate.itemized_deduction;
        if gap < 500_000 { // within $5K of itemizing
            insights.push(serde_json::json!({
                "type": "optimization", "priority": 8,
                "title": "Close to itemizing",
                "body": format!("Your itemized deductions ({}) are {} below the standard deduction. Consider bunching charitable donations or prepaying property tax to cross the threshold.",
                    cents_to_display(estimate.itemized_deduction), cents_to_display(gap)),
                "estimated_savings_cents": gap * estimate.ordinary_tax / std::cmp::max(estimate.taxable_income, 1),
            }));
        }
    }

    // Medical expense threshold
    let medical: i64 = conn.query_row(
        "SELECT COALESCE(SUM(e.amount_cents),0) FROM expenses e JOIN expense_categories c ON e.category_id=c.id \
         WHERE e.user_id=? AND c.name='Medical' AND e.entity='personal'",
        rusqlite::params![user_id], |r| r.get(0),
    ).unwrap_or(0);
    if medical > 0 && estimate.agi > 0 {
        let floor = (estimate.agi as f64 * 0.075) as i64;
        let gap = floor - medical;
        if gap > 0 && gap < 200_000 {
            insights.push(serde_json::json!({
                "type": "optimization", "priority": 7,
                "title": "Medical expense threshold",
                "body": format!("You have {} in medical expenses — {} more would cross the 7.5% AGI floor for deductibility. Schedule elective procedures before year-end if applicable.",
                    cents_to_display(medical), cents_to_display(gap)),
            }));
        }
    }

    // Retirement contribution opportunity
    if estimate.savers_credit == 0 && estimate.agi < 4_000_000 {
        insights.push(serde_json::json!({
            "type": "optimization", "priority": 9,
            "title": "Saver's Credit available",
            "body": "You may qualify for the Saver's Credit (up to $1,000) by contributing to an IRA or 401(k). Even a small contribution could reduce your tax.",
            "estimated_savings_cents": 100_000,
        }));
    }

    // EITC eligibility check
    if estimate.eitc > 0 {
        insights.push(serde_json::json!({
            "type": "milestone", "priority": 10,
            "title": format!("EITC: {} credit", cents_to_display(estimate.eitc)),
            "body": "You qualify for the Earned Income Tax Credit. This is a refundable credit — you'll receive it even if you owe no tax.",
        }));
    }

    // Estimated tax underpayment warning
    if estimate.owed > 100_000 { // owes more than $1K
        insights.push(serde_json::json!({
            "type": "warning", "priority": 9,
            "title": format!("Projected underpayment: {}", cents_to_display(estimate.owed)),
            "body": "You're on track to owe at filing. Consider making an estimated tax payment to avoid underpayment penalties. Go to IRS Direct Pay to pay online.",
        }));
    }

    // Large refund = overwithholding
    if estimate.owed < -200_000 { // refund over $2K
        insights.push(serde_json::json!({
            "type": "optimization", "priority": 6,
            "title": format!("Large refund projected: {}", cents_to_display(estimate.owed.abs())),
            "body": "A large refund means you're lending money to the IRS interest-free. Consider adjusting your W-4 to reduce withholding and increase your take-home pay.",
        }));
    }

    // HSA contribution opportunity
    if estimate.se_income > 0 || estimate.w2_income > 0 {
        let hsa: i64 = conn.query_row(
            "SELECT COALESCE(SUM(e.amount_cents),0) FROM expenses e JOIN expense_categories c ON e.category_id=c.id \
             WHERE e.user_id=? AND c.name LIKE '%HSA%'",
            rusqlite::params![user_id], |r| r.get(0),
        ).unwrap_or(0);
        if hsa == 0 {
            insights.push(serde_json::json!({
                "type": "optimization", "priority": 7,
                "title": "HSA contribution opportunity",
                "body": "If you have a high-deductible health plan, HSA contributions are triple-tax-advantaged: deductible, grow tax-free, and withdrawals for medical expenses are tax-free. 2025 limit: $4,300 individual / $8,550 family.",
            }));
        }
    }

    insights.sort_by(|a, b| {
        let pa = a.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
        let pb = b.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
        pb.cmp(&pa)
    });
    insights
}

// ── AI Tax Advisor API ──────────────────────────────────────────────────────

pub async fn handle_tax_context(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let context = build_tax_context(&conn, uid, year);
        Ok(serde_json::json!({"context": context, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_audit_risk(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let factors = compute_audit_risk(&conn, uid, year);
        Ok(serde_json::json!({"factors": factors, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_tax_insights(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let insights = generate_tax_insights(&conn, uid, year);
        Ok(serde_json::json!({"insights": insights, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct WhatIfReq {
    token: String,
    tax_year: i64,
    scenario_name: String,
    additional_income_cents: Option<i64>,
    additional_deduction_cents: Option<i64>,
    filing_status_override: Option<String>,
    additional_dependents: Option<i64>,
    retirement_contribution_cents: Option<i64>,
}

pub async fn handle_what_if(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WhatIfReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // Get current filing status
        let current_fs: String = conn.query_row(
            "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, req.tax_year], |r| r.get(0),
        ).unwrap_or_else(|_| "single".to_string());

        // Baseline
        let baseline = compute_tax_estimate(&conn, uid, req.tax_year, &current_fs)?;

        // For the what-if, we compute a modified estimate
        // This is simplified: we adjust the baseline numbers directly
        let scenario_fs = req.filing_status_override.as_deref().unwrap_or(&current_fs);
        let scenario = compute_tax_estimate(&conn, uid, req.tax_year, scenario_fs)?;

        // Apply what-if adjustments to the scenario
        let adj_income = req.additional_income_cents.unwrap_or(0);
        let adj_deduction = req.additional_deduction_cents.unwrap_or(0);
        let adj_retirement = req.retirement_contribution_cents.unwrap_or(0);

        // Rough recalculation with adjustments
        let new_agi = scenario.agi + adj_income - adj_deduction - adj_retirement;
        let new_taxable = std::cmp::max(new_agi - scenario.deduction_used, 0);

        // Re-estimate tax at the new taxable income level using marginal rate
        let marginal_rate = if scenario.taxable_income > 0 {
            (scenario.ordinary_tax as f64 / scenario.taxable_income as f64 * 10000.0) as i64
        } else { 2200 }; // default 22% bracket

        let tax_change = (new_taxable - scenario.taxable_income) * marginal_rate / 10000;
        let new_tax = scenario.total_tax + tax_change;
        let new_owed = new_tax - scenario.total_payments;

        Ok(serde_json::json!({
            "scenario": req.scenario_name,
            "baseline": {
                "agi": cents_to_display(baseline.agi),
                "taxable_income": cents_to_display(baseline.taxable_income),
                "total_tax": cents_to_display(baseline.total_tax),
                "owed": cents_to_display(baseline.owed),
                "effective_rate": format!("{:.1}%", baseline.effective_rate),
            },
            "scenario_result": {
                "agi": cents_to_display(new_agi),
                "taxable_income": cents_to_display(new_taxable),
                "total_tax": cents_to_display(new_tax),
                "owed": cents_to_display(new_owed),
            },
            "difference": {
                "tax_change": cents_to_display(tax_change),
                "owed_change": cents_to_display(new_owed - baseline.owed),
                "savings": if tax_change < 0 { cents_to_display(tax_change.abs()) } else { "$0.00".to_string() },
            },
        }))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

// ── State Tax Engine ────────────────────────────────────────────────────────

/// State tax bracket data: (upper_limit_cents, rate_basis_points).
/// Returns (brackets, standard_deduction_cents, personal_exemption_cents).
/// 2025 rates for top 10 states by population + common others.
fn state_brackets(state: &str, filing_status: &str) -> (Vec<(i64, i64)>, i64, i64) {
    let is_joint = filing_status == "married_jointly";
    match state {
        // California — 9 brackets, top 13.3%
        "CA" => {
            let brackets = if is_joint {
                vec![(2_168_400, 100), (5_144_200, 200), (8_108_400, 400), (10_055_400, 600),
                     (12_714_600, 800), (32_587_400, 930), (39_104_800, 1030),
                     (65_174_600, 1130), (130_349_200, 1230), (i64::MAX, 1330)]
            } else {
                vec![(1_084_200, 100), (2_572_100, 200), (4_054_200, 400), (5_027_700, 600),
                     (6_357_300, 800), (16_293_700, 930), (19_552_400, 1030),
                     (32_587_300, 1130), (100_000_000, 1230), (i64::MAX, 1330)]
            };
            let std_ded = if is_joint { 1_108_400 } else { 554_200 };
            (brackets, std_ded, 0)
        }
        // New York — 8 brackets, top 10.9%
        "NY" => {
            let brackets = if is_joint {
                vec![(1_740_000, 400), (2_390_000, 450), (5_540_000, 525),
                     (10_750_000, 585), (16_150_000, 625), (21_550_000, 685),
                     (32_350_000, 965), (i64::MAX, 1090)]
            } else {
                vec![(870_000, 400), (1_195_000, 450), (2_770_000, 525),
                     (5_375_000, 585), (8_075_000, 625), (10_775_000, 685),
                     (2_500_000_000, 965), (i64::MAX, 1090)]
            };
            let std_ded = if is_joint { 1_610_000 } else { 805_000 };
            (brackets, std_ded, 0)
        }
        // Pennsylvania — flat 3.07%
        "PA" => (vec![(i64::MAX, 307)], 0, 0),
        // Illinois — flat 4.95%
        "IL" => {
            let exemption = if is_joint { 475_000 } else { 237_500 };
            (vec![(i64::MAX, 495)], 0, exemption)
        }
        // Ohio — 4 brackets, top 3.5% (reduced 2025)
        "OH" => {
            let brackets = vec![(2_600_000, 0), (4_600_000, 275), (9_200_000, 321), (i64::MAX, 350)];
            (brackets, 0, 0)
        }
        // Georgia — 6 brackets, top 5.39% (2025 phase-down)
        "GA" => {
            let brackets = if is_joint {
                vec![(1_000_000, 100), (3_000_000, 200), (5_000_000, 300),
                     (7_000_000, 400), (10_000_000, 500), (i64::MAX, 539)]
            } else {
                vec![(75_000, 100), (250_000, 200), (375_000, 300),
                     (500_000, 400), (700_000, 500), (i64::MAX, 539)]
            };
            let std_ded = if is_joint { 1_200_000 } else { 600_000 };
            (brackets, std_ded, 0)
        }
        // North Carolina — flat 4.5% (2025)
        "NC" => {
            let std_ded = if is_joint { 2_550_000 } else { 1_275_000 };
            (vec![(i64::MAX, 450)], std_ded, 0)
        }
        // Michigan — flat 4.05%
        "MI" => {
            let exemption = if is_joint { 1_040_000 } else { 520_000 };
            (vec![(i64::MAX, 405)], 0, exemption)
        }
        // New Jersey — 7 brackets, top 10.75%
        "NJ" => {
            let brackets = if is_joint {
                vec![(2_000_000, 140), (5_000_000, 175), (7_000_000, 245),
                     (8_000_000, 350), (15_000_000, 553), (100_000_000, 897),
                     (i64::MAX, 1075)]
            } else {
                vec![(2_000_000, 140), (3_500_000, 175), (4_000_000, 245),
                     (7_500_000, 350), (50_000_000, 553), (100_000_000, 897),
                     (i64::MAX, 1075)]
            };
            (brackets, 0, 100_000)
        }
        // Virginia — 4 brackets, top 5.75%
        "VA" => {
            let brackets = vec![(300_000, 200), (500_000, 300), (1_700_000, 500), (i64::MAX, 575)];
            let std_ded = if is_joint { 1_600_000 } else { 800_000 };
            (brackets, std_ded, 93_000)
        }
        // Massachusetts — flat 5%
        "MA" => {
            let exemption = if is_joint { 880_000 } else { 440_000 };
            (vec![(i64::MAX, 500)], 0, exemption)
        }
        // Colorado — flat 4.4%
        "CO" => {
            let std_ded = if is_joint { 2_750_000 } else { 1_375_000 };
            (vec![(i64::MAX, 440)], std_ded, 0)
        }
        // Arizona — flat 2.5%
        "AZ" => {
            let std_ded = if is_joint { 2_792_000 } else { 1_396_000 };
            (vec![(i64::MAX, 250)], std_ded, 0)
        }
        // No income tax states
        "TX" | "FL" | "NV" | "WA" | "WY" | "SD" | "AK" | "TN" | "NH" => {
            (vec![], 0, 0)
        }
        _ => {
            // Unknown state — return empty (no tax)
            (vec![], 0, 0)
        }
    }
}

/// Compute state income tax for one state.
pub fn compute_state_tax(
    federal_agi: i64,
    state: &str,
    filing_status: &str,
    state_wages: i64,
    state_withheld: i64,
    residency: &str,
    months: i64,
) -> serde_json::Value {
    let (brackets, std_ded, exemption) = state_brackets(state, filing_status);

    // No income tax states
    if brackets.is_empty() {
        return serde_json::json!({
            "state": state, "has_income_tax": false,
            "state_tax": "$0.00", "state_tax_cents": 0,
            "owed": "$0.00", "owed_cents": 0,
        });
    }

    // State AGI: start from federal AGI (most states)
    // Apply residency proration for part-year
    let proration = if residency == "part_year" && months < 12 {
        months as f64 / 12.0
    } else { 1.0 };

    let state_agi = (federal_agi as f64 * proration) as i64;
    let state_taxable = std::cmp::max(state_agi - std_ded - exemption, 0);

    // Compute tax through brackets
    let mut tax = 0i64;
    let mut prev = 0i64;
    for &(limit, rate) in &brackets {
        let bracket_income = std::cmp::min(state_taxable, limit) - prev;
        if bracket_income <= 0 { break; }
        tax += bracket_income * rate / 10000;
        prev = limit;
    }

    let owed = tax - state_withheld;
    let effective = if state_agi > 0 { (tax as f64 / state_agi as f64) * 100.0 } else { 0.0 };

    serde_json::json!({
        "state": state, "has_income_tax": true,
        "state_agi": cents_to_display(state_agi),
        "state_taxable": cents_to_display(state_taxable),
        "standard_deduction": cents_to_display(std_ded),
        "exemption": cents_to_display(exemption),
        "state_tax": cents_to_display(tax), "state_tax_cents": tax,
        "state_withheld": cents_to_display(state_withheld),
        "owed": cents_to_display(owed), "owed_cents": owed,
        "effective_rate": format!("{:.2}%", effective),
        "residency": residency, "months": months,
    })
}

// ── State Tax API ───────────────────────────────────────────────────────────

pub async fn handle_state_tax_estimate(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        let fs: String = conn.query_row(
            "SELECT filing_status FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
            rusqlite::params![uid, year], |r| r.get(0),
        ).unwrap_or_else(|_| "single".to_string());

        let federal = compute_tax_estimate(&conn, uid, year, &fs)?;

        // Get all state profiles for this user/year
        let mut stmt = conn.prepare(
            "SELECT state, residency_type, months_resident, state_wages_cents, state_withheld_cents \
             FROM state_tax_profiles WHERE user_id = ? AND tax_year = ? ORDER BY state"
        ).map_err(|e| e.to_string())?;

        let states: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, year], |r| {
            let st: String = r.get(0)?;
            let residency: String = r.get(1)?;
            let months: i64 = r.get(2)?;
            let wages: i64 = r.get(3)?;
            let withheld: i64 = r.get(4)?;
            Ok(compute_state_tax(federal.agi, &st, &fs, wages, withheld, &residency, months))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

        let total_state_tax: i64 = states.iter()
            .filter_map(|s| s.get("state_tax_cents").and_then(|v| v.as_i64()))
            .sum();
        let total_state_owed: i64 = states.iter()
            .filter_map(|s| s.get("owed_cents").and_then(|v| v.as_i64()))
            .sum();

        Ok(serde_json::json!({
            "year": year,
            "federal_agi": cents_to_display(federal.agi),
            "states": states,
            "total_state_tax": cents_to_display(total_state_tax),
            "total_state_owed": cents_to_display(total_state_owed),
            "combined_tax": cents_to_display(federal.total_tax + total_state_tax),
            "combined_effective_rate": if federal.gross_income > 0 {
                format!("{:.1}%", ((federal.total_tax + total_state_tax) as f64 / federal.gross_income as f64) * 100.0)
            } else { "0.0%".to_string() },
        }))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct StateProfileReq {
    token: String,
    tax_year: i64,
    state: String,
    residency_type: Option<String>,
    months_resident: Option<i64>,
    state_wages_cents: Option<i64>,
    state_withheld_cents: Option<i64>,
}

pub async fn handle_state_profile_save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StateProfileReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO state_tax_profiles (user_id, tax_year, state, residency_type, months_resident, \
             state_wages_cents, state_withheld_cents, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, tax_year, state) DO UPDATE SET \
             residency_type=excluded.residency_type, months_resident=excluded.months_resident, \
             state_wages_cents=excluded.state_wages_cents, state_withheld_cents=excluded.state_withheld_cents, \
             updated_at=excluded.updated_at",
            rusqlite::params![uid, req.tax_year, req.state.to_uppercase(),
                req.residency_type.as_deref().unwrap_or("full_year"),
                req.months_resident.unwrap_or(12),
                req.state_wages_cents.unwrap_or(0),
                req.state_withheld_cents.unwrap_or(0),
                now, now],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn handle_state_profile_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT state, residency_type, months_resident, state_wages_cents, state_withheld_cents \
             FROM state_tax_profiles WHERE user_id = ? AND tax_year = ? ORDER BY state"
        ).map_err(|e| e.to_string())?;
        let profiles: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid, year], |r| {
            Ok(serde_json::json!({
                "state": r.get::<_,String>(0)?, "residency": r.get::<_,String>(1)?,
                "months": r.get::<_,i64>(2)?,
                "wages": cents_to_display(r.get::<_,i64>(3)?),
                "withheld": cents_to_display(r.get::<_,i64>(4)?),
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"profiles": profiles, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

/// List all supported states with their tax type (income tax / no income tax).
pub async fn handle_supported_states(
    State(_state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let states = vec![
        ("AL", "Alabama", true), ("AK", "Alaska", false), ("AZ", "Arizona", true),
        ("AR", "Arkansas", true), ("CA", "California", true), ("CO", "Colorado", true),
        ("CT", "Connecticut", true), ("DE", "Delaware", true), ("FL", "Florida", false),
        ("GA", "Georgia", true), ("HI", "Hawaii", true), ("ID", "Idaho", true),
        ("IL", "Illinois", true), ("IN", "Indiana", true), ("IA", "Iowa", true),
        ("KS", "Kansas", true), ("KY", "Kentucky", true), ("LA", "Louisiana", true),
        ("ME", "Maine", true), ("MD", "Maryland", true), ("MA", "Massachusetts", true),
        ("MI", "Michigan", true), ("MN", "Minnesota", true), ("MS", "Mississippi", true),
        ("MO", "Missouri", true), ("MT", "Montana", true), ("NE", "Nebraska", true),
        ("NV", "Nevada", false), ("NH", "New Hampshire", false), ("NJ", "New Jersey", true),
        ("NM", "New Mexico", true), ("NY", "New York", true), ("NC", "North Carolina", true),
        ("ND", "North Dakota", true), ("OH", "Ohio", true), ("OK", "Oklahoma", true),
        ("OR", "Oregon", true), ("PA", "Pennsylvania", true), ("RI", "Rhode Island", true),
        ("SC", "South Carolina", true), ("SD", "South Dakota", false), ("TN", "Tennessee", false),
        ("TX", "Texas", false), ("UT", "Utah", true), ("VT", "Vermont", true),
        ("VA", "Virginia", true), ("WA", "Washington", false), ("WV", "West Virginia", true),
        ("WI", "Wisconsin", true), ("WY", "Wyoming", false), ("DC", "District of Columbia", true),
    ];
    let list: Vec<serde_json::Value> = states.iter().map(|(code, name, has_tax)| {
        serde_json::json!({"code": code, "name": name, "has_income_tax": has_tax})
    }).collect();
    Json(serde_json::json!({"states": list, "brackets_available": [
        "CA","NY","PA","IL","OH","GA","NC","MI","NJ","VA","MA","CO","AZ"
    ]}))
}

// ── Business Entity Engine ──────────────────────────────────────────────────

/// Compute P&L for a business entity.
fn compute_entity_pnl(conn: &rusqlite::Connection, entity_id: i64, year: i64) -> serde_json::Value {
    let income: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents),0) FROM entity_income WHERE entity_id=? AND tax_year=?",
        rusqlite::params![entity_id, year], |r| r.get(0),
    ).unwrap_or(0);
    let expenses: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents),0) FROM entity_expenses WHERE entity_id=? AND tax_year=?",
        rusqlite::params![entity_id, year], |r| r.get(0),
    ).unwrap_or(0);
    let net = income - expenses;

    // Get entity type for tax calculation
    let entity_type: String = conn.query_row(
        "SELECT entity_type FROM business_entities WHERE id=?",
        rusqlite::params![entity_id], |r| r.get(0),
    ).unwrap_or_else(|_| "sole_prop".to_string());

    // S-Corp/Partnership: pass-through to shareholders
    // C-Corp: entity-level tax at 21% flat
    let entity_tax = if entity_type == "c_corp" {
        std::cmp::max(net, 0) * 2100 / 10000 // 21% flat
    } else { 0 };

    // Shareholder distributions
    let mut sh_stmt = conn.prepare(
        "SELECT name, ownership_pct, salary_cents, distribution_cents \
         FROM entity_shareholders WHERE entity_id=? AND tax_year=? ORDER BY ownership_pct DESC"
    ).ok();
    let shareholders: Vec<serde_json::Value> = sh_stmt.as_mut().map(|s| {
        s.query_map(rusqlite::params![entity_id, year], |r| {
            let pct: i64 = r.get(1)?;
            let k1_share = net * pct / 100;
            Ok(serde_json::json!({
                "name": r.get::<_,String>(0)?, "ownership_pct": pct,
                "salary": cents_to_display(r.get::<_,i64>(2)?),
                "distributions": cents_to_display(r.get::<_,i64>(3)?),
                "k1_ordinary": cents_to_display(k1_share),
                "k1_ordinary_cents": k1_share,
            }))
        }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }).unwrap_or_default();

    // Expense breakdown by category
    let mut cat_stmt = conn.prepare(
        "SELECT category, SUM(amount_cents) FROM entity_expenses WHERE entity_id=? AND tax_year=? GROUP BY category ORDER BY SUM(amount_cents) DESC"
    ).ok();
    let categories: Vec<serde_json::Value> = cat_stmt.as_mut().map(|s| {
        s.query_map(rusqlite::params![entity_id, year], |r| {
            Ok(serde_json::json!({"category": r.get::<_,String>(0)?, "amount": cents_to_display(r.get::<_,i64>(1)?)}))
        }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }).unwrap_or_default();

    serde_json::json!({
        "income": cents_to_display(income), "expenses": cents_to_display(expenses),
        "net_income": cents_to_display(net), "entity_tax": cents_to_display(entity_tax),
        "entity_type": entity_type,
        "is_pass_through": entity_type != "c_corp",
        "shareholders": shareholders, "expense_categories": categories,
    })
}

// ── Business Entity API ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct CreateEntityReq {
    token: String,
    entity_name: String,
    entity_type: String,
    ein: Option<String>,
    state_of_formation: Option<String>,
    formation_date: Option<String>,
    ownership_pct: Option<i64>,
}

pub async fn handle_entity_create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateEntityReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let principal = crate::resolve_principal(&state, &req.token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let valid_types = ["sole_prop", "s_corp", "partnership", "c_corp", "llc_single", "llc_multi"];
    if !valid_types.contains(&req.entity_type.as_str()) {
        return Ok(Json(serde_json::json!({"ok": false, "error": format!("Invalid type. Use: {}", valid_types.join(", "))})));
    }
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO business_entities (user_id, entity_name, entity_type, ein_encrypted, \
             state_of_formation, formation_date, ownership_pct, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            rusqlite::params![uid, req.entity_name, req.entity_type, req.ein,
                req.state_of_formation, req.formation_date,
                req.ownership_pct.unwrap_or(100), now, now],
        ).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"ok": true, "entity_id": conn.last_insert_rowid()}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_entity_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, entity_name, entity_type, state_of_formation, formation_date, \
             ownership_pct, status FROM business_entities WHERE user_id=? ORDER BY entity_name"
        ).map_err(|e| e.to_string())?;
        let entities: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![uid], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_,i64>(0)?, "name": r.get::<_,String>(1)?,
                "type": r.get::<_,String>(2)?, "state": r.get::<_,Option<String>>(3)?,
                "formed": r.get::<_,Option<String>>(4)?,
                "ownership_pct": r.get::<_,i64>(5)?, "status": r.get::<_,String>(6)?,
            }))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"entities": entities}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

pub async fn handle_entity_summary(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(entity_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        Ok(compute_entity_pnl(&conn, entity_id, year))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct EntityIncomeReq { token: String, entity_id: i64, tax_year: i64, income_type: String, amount_cents: i64, description: Option<String> }

pub async fn handle_entity_income_create(
    State(state): State<Arc<AppState>>, Json(req): Json<EntityIncomeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let _p = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO entity_income (entity_id, tax_year, income_type, amount_cents, description, created_at) VALUES (?,?,?,?,?,?)",
            rusqlite::params![req.entity_id, req.tax_year, req.income_type, req.amount_cents, req.description, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(serde::Deserialize)]
pub struct EntityExpenseReq { token: String, entity_id: i64, tax_year: i64, category: String, amount_cents: i64, vendor: Option<String>, expense_date: Option<String>, description: Option<String> }

pub async fn handle_entity_expense_create(
    State(state): State<Arc<AppState>>, Json(req): Json<EntityExpenseReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let _p = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO entity_expenses (entity_id, tax_year, category, amount_cents, vendor, expense_date, description, created_at) VALUES (?,?,?,?,?,?,?,?)",
            rusqlite::params![req.entity_id, req.tax_year, req.category, req.amount_cents, req.vendor, req.expense_date, req.description, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(serde::Deserialize)]
pub struct ShareholderReq { token: String, entity_id: i64, tax_year: i64, name: String, ownership_pct: i64, salary_cents: Option<i64>, distribution_cents: Option<i64> }

pub async fn handle_shareholder_save(
    State(state): State<Arc<AppState>>, Json(req): Json<ShareholderReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let _p = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO entity_shareholders (entity_id, name, ownership_pct, salary_cents, distribution_cents, tax_year, created_at) \
             VALUES (?,?,?,?,?,?,?) ON CONFLICT(entity_id, name, tax_year) DO UPDATE SET \
             ownership_pct=excluded.ownership_pct, salary_cents=excluded.salary_cents, distribution_cents=excluded.distribution_cents",
            rusqlite::params![req.entity_id, req.name, req.ownership_pct,
                req.salary_cents.unwrap_or(0), req.distribution_cents.unwrap_or(0), req.tax_year, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// Generate K-1 data for all shareholders of an entity.
pub async fn handle_entity_k1_generate(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(entity_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _p = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let pnl = compute_entity_pnl(&conn, entity_id, year);
        let entity_name: String = conn.query_row(
            "SELECT entity_name FROM business_entities WHERE id=?",
            rusqlite::params![entity_id], |r| r.get(0),
        ).unwrap_or_else(|_| "Unknown".to_string());
        let entity_type: String = conn.query_row(
            "SELECT entity_type FROM business_entities WHERE id=?",
            rusqlite::params![entity_id], |r| r.get(0),
        ).unwrap_or_else(|_| "partnership".to_string());

        let shareholders = pnl.get("shareholders").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let k1s: Vec<serde_json::Value> = shareholders.iter().map(|sh| {
            serde_json::json!({
                "entity_name": entity_name,
                "entity_type": entity_type,
                "form": if entity_type == "s_corp" { "1120-S K-1" } else { "1065 K-1" },
                "shareholder": sh.get("name"),
                "ownership_pct": sh.get("ownership_pct"),
                "ordinary_income": sh.get("k1_ordinary"),
                "salary": sh.get("salary"),
                "distributions": sh.get("distributions"),
                "tax_year": year,
            })
        }).collect();
        Ok(serde_json::json!({"k1s": k1s, "entity": entity_name, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct Issue1099Req { token: String, entity_id: i64, tax_year: i64, recipient_name: String, recipient_address: Option<String>, amount_cents: i64 }

pub async fn handle_1099_issue(
    State(state): State<Arc<AppState>>, Json(req): Json<Issue1099Req>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let _p = crate::resolve_principal(&state, &req.token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    if req.amount_cents < 60_000 { // $600 threshold
        return Ok(Json(serde_json::json!({"ok": false, "error": "1099-NEC only required for payments of $600 or more"})));
    }
    let db = state.db_path.clone();
    let now = chrono::Utc::now().timestamp();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO entity_1099s (entity_id, tax_year, recipient_name, recipient_address, amount_cents, form_type, status, created_at) \
             VALUES (?,?,?,?,?,'1099-NEC','draft',?)",
            rusqlite::params![req.entity_id, req.tax_year, req.recipient_name, req.recipient_address, req.amount_cents, now]).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn handle_1099_list(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(entity_id): axum::extract::Path<i64>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _p = crate::resolve_principal(&state, token).await.map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, recipient_name, amount_cents, form_type, status FROM entity_1099s WHERE entity_id=? AND tax_year=? ORDER BY recipient_name"
        ).map_err(|e| e.to_string())?;
        let forms: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![entity_id, year], |r| {
            Ok(serde_json::json!({"id": r.get::<_,i64>(0)?, "recipient": r.get::<_,String>(1)?,
                "amount": cents_to_display(r.get::<_,i64>(2)?), "form": r.get::<_,String>(3)?, "status": r.get::<_,String>(4)?}))
        }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();
        Ok(serde_json::json!({"forms": forms, "year": year}))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

/// Compare entity structures: sole prop vs S-Corp vs LLC.
pub async fn handle_entity_comparison(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let income_cents: i64 = params.get("income").and_then(|s| s.parse().ok()).unwrap_or(0);
    let db = state.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;

        // If no income specified, use actual SE income
        let se_income = if income_cents > 0 { income_cents } else {
            conn.query_row(
                "SELECT COALESCE(SUM(amount_cents),0) FROM tax_income WHERE user_id=? AND tax_year=? AND (category LIKE '%1099%' OR category LIKE '%Self%')",
                rusqlite::params![uid, year], |r| r.get(0),
            ).unwrap_or(0)
        };

        if se_income == 0 {
            return Ok(serde_json::json!({"error": "No self-employment income to compare. Pass ?income=CENTS to model."}));
        }

        // Sole prop: full SE tax on all income
        let se_taxable = (se_income as f64 * 0.9235) as i64;
        let sp_se_tax = std::cmp::min(se_taxable, 17_610_000) * 1240 / 10000 + se_taxable * 290 / 10000;

        // S-Corp: reasonable salary (60% of income) gets employment tax, rest is distribution
        let reasonable_salary = se_income * 60 / 100;
        let sc_employer_fica = std::cmp::min(reasonable_salary, 17_610_000) * 765 / 10000; // 7.65%
        let sc_employee_fica = sc_employer_fica; // employee pays same
        let sc_total_fica = sc_employer_fica + sc_employee_fica;
        let sc_savings = sp_se_tax - sc_total_fica;

        Ok(serde_json::json!({
            "income": cents_to_display(se_income),
            "sole_proprietorship": {
                "se_tax": cents_to_display(sp_se_tax),
                "notes": "Full 15.3% SE tax on all net earnings",
            },
            "s_corp": {
                "reasonable_salary": cents_to_display(reasonable_salary),
                "employer_fica": cents_to_display(sc_employer_fica),
                "employee_fica": cents_to_display(sc_employee_fica),
                "total_fica": cents_to_display(sc_total_fica),
                "distribution": cents_to_display(se_income - reasonable_salary),
                "savings_vs_sole_prop": cents_to_display(sc_savings),
                "notes": "Salary taxed at FICA rates; distributions avoid SE tax",
            },
            "recommendation": if sc_savings > 500_000 {
                format!("S-Corp could save {} annually in SE tax. Consider if income is consistently above $50K.", cents_to_display(sc_savings))
            } else {
                "At this income level, the administrative cost of an S-Corp may not justify the tax savings. Stay as sole proprietor.".to_string()
            },
        }))
    }).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

// ── Paper Filing Package (Printable 1040 + Schedules) ───────────────────────

/// Generate a complete printable tax return package as HTML.
/// Includes Form 1040, applicable schedules, filing checklist, and mailing instructions.
pub fn generate_print_return(conn: &rusqlite::Connection, user_id: i64, year: i64) -> Result<String, String> {
    // Load taxpayer profile
    let (first, last, ssn_last4, address, city, st, zip, filing_status, occupation,
     spouse_first, spouse_last) = conn.query_row(
        "SELECT first_name, last_name, SUBSTR(ssn_encrypted,-4), address_line1, city, state, zip, \
         filing_status, occupation, spouse_first, spouse_last \
         FROM taxpayer_profiles WHERE user_id = ? AND tax_year = ?",
        rusqlite::params![user_id, year],
        |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?,
                r.get::<_,Option<String>>(3)?, r.get::<_,Option<String>>(4)?,
                r.get::<_,Option<String>>(5)?, r.get::<_,Option<String>>(6)?,
                r.get::<_,String>(7)?, r.get::<_,Option<String>>(8)?,
                r.get::<_,Option<String>>(9)?, r.get::<_,Option<String>>(10)?)),
    ).map_err(|e| format!("Taxpayer profile not found: {e}. Complete your profile first."))?;

    let est = compute_tax_estimate(conn, user_id, year, &filing_status)?;

    // Dependents
    let mut dep_stmt = conn.prepare(
        "SELECT first_name, last_name, relationship, months_lived, qualifies_ctc \
         FROM dependents WHERE user_id = ? AND tax_year = ?"
    ).map_err(|e| e.to_string())?;
    let deps: Vec<(String, String, String, i64, bool)> = dep_stmt.query_map(
        rusqlite::params![user_id, year], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    }).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect();

    let fs_check = |s: &str| if filing_status == s { "X" } else { "&nbsp;" };
    let d = |cents: i64| cents_to_display(cents);
    let has_se = est.se_income > 0;
    let has_itemized = est.deduction_type == "Itemized";
    let has_ltcg = est.ltcg_tax > 0;

    let mut html = String::new();
    html.push_str(&format!(r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<title>Tax Return {year} — {first} {last}</title>
<style>
@media print {{ @page {{ margin: 0.5in; size: letter; }} body {{ font-size: 10pt; }} .no-print {{ display: none; }} }}
body {{ font-family: 'Courier New', monospace; max-width: 8in; margin: 0 auto; padding: 20px; color: #000; background: #fff; }}
h1 {{ text-align: center; font-size: 16pt; border-bottom: 3px double #000; padding-bottom: 8px; }}
h2 {{ font-size: 12pt; border-bottom: 1px solid #000; margin-top: 24px; }}
h3 {{ font-size: 11pt; margin-top: 16px; }}
.form-header {{ text-align: center; font-weight: bold; font-size: 14pt; margin: 20px 0 5px; }}
.form-sub {{ text-align: center; font-size: 9pt; color: #555; }}
.line {{ display: flex; justify-content: space-between; padding: 2px 0; border-bottom: 1px dotted #ccc; }}
.line-num {{ width: 30px; font-weight: bold; color: #333; }}
.line-desc {{ flex: 1; }}
.line-val {{ text-align: right; min-width: 100px; font-weight: bold; }}
.info-box {{ border: 1px solid #000; padding: 8px; margin: 8px 0; }}
.check {{ display: inline-block; width: 14px; height: 14px; border: 1px solid #000; text-align: center; font-size: 10pt; margin-right: 4px; }}
.section {{ margin: 16px 0; }}
.page-break {{ page-break-before: always; }}
.checklist li {{ padding: 4px 0; }}
.signature-line {{ border-bottom: 1px solid #000; width: 300px; display: inline-block; height: 20px; margin-top: 20px; }}
</style></head><body>"#));

    // ── Print button (hidden when printing) ──
    html.push_str(r#"<div class="no-print" style="text-align:center;margin-bottom:20px;">
        <button onclick="window.print()" style="padding:10px 30px;font-size:14pt;cursor:pointer;">Print Tax Return</button>
        <p style="color:#666;font-size:9pt;">This document is formatted for US Letter paper. Print or Save as PDF.</p>
    </div>"#);

    // ── FORM 1040 ──
    html.push_str(&format!(r#"
<div class="form-header">Form 1040 — U.S. Individual Income Tax Return</div>
<div class="form-sub">Department of the Treasury — Internal Revenue Service | Tax Year {year}</div>

<div class="info-box">
<strong>Filing Status:</strong>
<span class="check">{}</span> Single
<span class="check">{}</span> Married filing jointly
<span class="check">{}</span> Married filing separately
<span class="check">{}</span> Head of household
</div>

<div class="info-box">
<strong>Your first name and middle initial:</strong> {first} &nbsp;&nbsp; <strong>Last name:</strong> {last} &nbsp;&nbsp; <strong>SSN:</strong> ***-**-{ssn}<br>
<strong>Address:</strong> {addr} &nbsp;&nbsp; <strong>City:</strong> {cty} &nbsp;&nbsp; <strong>State:</strong> {stt} &nbsp;&nbsp; <strong>ZIP:</strong> {zp}<br>
<strong>Occupation:</strong> {occ}
</div>"#,
        fs_check("single"), fs_check("married_jointly"), fs_check("married_separately"), fs_check("head_of_household"),
        ssn = ssn_last4.as_deref().unwrap_or("----"),
        addr = address.as_deref().unwrap_or(""), cty = city.as_deref().unwrap_or(""),
        stt = st.as_deref().unwrap_or(""), zp = zip.as_deref().unwrap_or(""),
        occ = occupation.as_deref().unwrap_or(""),
    ));

    if let (Some(sf), Some(sl)) = (&spouse_first, &spouse_last) {
        if !sf.is_empty() {
            html.push_str(&format!(r#"<div class="info-box"><strong>Spouse:</strong> {sf} {sl}</div>"#));
        }
    }

    // Dependents
    if !deps.is_empty() {
        html.push_str(r#"<div class="info-box"><strong>Dependents:</strong><br>"#);
        for (df, dl, rel, months, ctc) in &deps {
            html.push_str(&format!("{df} {dl} — {rel}, {months} months, CTC: {}<br>",
                if *ctc { "Yes" } else { "No" }));
        }
        html.push_str("</div>");
    }

    // Income section
    html.push_str(r#"<h2>Income</h2><div class="section">"#);
    let ded_label = format!("{} deduction", est.deduction_type);
    let lines: Vec<(&str, &str, String)> = vec![
        ("1", "Wages, salaries, tips (W-2 box 1)", d(est.w2_income)),
        ("2a", "Tax-exempt interest", "$0.00".into()),
        ("3a", "Qualified dividends", "$0.00".into()),
        ("8", "Other income (Schedule 1)", d(est.se_income)),
        ("9", "Total income", d(est.gross_income)),
        ("10", "Adjustments (Schedule 1)", d(est.half_se_tax + est.se_health_deduction + est.student_loan_deduction)),
        ("11", "Adjusted gross income (AGI)", d(est.agi)),
        ("12", &ded_label, d(est.deduction_used)),
        ("13", "Qualified business income deduction", d(est.qbi_deduction)),
        ("15", "Taxable income", d(est.taxable_income)),
    ];
    for (num, desc, val) in &lines {
        html.push_str(&format!(r#"<div class="line"><span class="line-num">{num}</span><span class="line-desc">{desc}</span><span class="line-val">{val}</span></div>"#));
    }
    html.push_str("</div>");

    // Tax & Credits
    html.push_str(r#"<h2>Tax and Credits</h2><div class="section">"#);
    let tax_lines: Vec<(&str, &str, String)> = vec![
        ("16", "Tax (from tax table or Schedule D)", d(est.ordinary_tax + est.ltcg_tax)),
        ("23", "Other taxes (Schedule 2 — SE tax)", d(est.se_tax)),
        ("24", "Total tax", d(est.total_tax)),
    ];
    for (num, desc, val) in &tax_lines {
        html.push_str(&format!(r#"<div class="line"><span class="line-num">{num}</span><span class="line-desc">{desc}</span><span class="line-val">{val}</span></div>"#));
    }
    html.push_str("</div>");

    // Payments
    html.push_str(r#"<h2>Payments</h2><div class="section">"#);
    let pay_lines: Vec<(&str, &str, String)> = vec![
        ("25a", "Federal income tax withheld (W-2)", d(est.w2_withheld)),
        ("26", "Estimated tax payments", d(est.estimated_payments)),
        ("27", "Earned income credit (EITC)", d(est.eitc)),
        ("28", "Child tax credit", d(est.child_credit)),
        ("29", "Other credits (Schedule 3)", d(est.cdcc + est.education_credits + est.savers_credit + est.energy_credits)),
        ("33", "Total payments", d(est.total_payments)),
    ];
    for (num, desc, val) in &pay_lines {
        html.push_str(&format!(r#"<div class="line"><span class="line-num">{num}</span><span class="line-desc">{desc}</span><span class="line-val">{val}</span></div>"#));
    }
    if est.owed >= 0 {
        html.push_str(&format!(r#"<div class="line"><span class="line-num">37</span><span class="line-desc"><strong>Amount you owe</strong></span><span class="line-val"><strong>{}</strong></span></div>"#, d(est.owed)));
    } else {
        html.push_str(&format!(r#"<div class="line"><span class="line-num">35a</span><span class="line-desc"><strong>Overpaid (Refund)</strong></span><span class="line-val"><strong>{}</strong></span></div>"#, d(est.owed.abs())));
    }
    html.push_str("</div>");

    // Signature
    html.push_str(&format!(r#"
<div class="section" style="margin-top:30px;">
<strong>Sign Here:</strong><br>
Your signature: <span class="signature-line"></span> Date: <span class="signature-line" style="width:120px;"></span><br>
Occupation: {}<br>
</div>"#, occupation.as_deref().unwrap_or("")));

    // ── SCHEDULE C (if SE income) ──
    if has_se {
        html.push_str(r#"<div class="page-break"></div>"#);
        html.push_str(&format!(r#"
<div class="form-header">Schedule C — Profit or Loss From Business</div>
<div class="form-sub">Sole Proprietorship | {first} {last} | {year}</div>
<div class="section">
<div class="line"><span class="line-num">1</span><span class="line-desc">Gross receipts</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">28</span><span class="line-desc">Total expenses</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">29</span><span class="line-desc">Meals adjustment (50%)</span><span class="line-val">({})</span></div>
<div class="line"><span class="line-num">31</span><span class="line-desc"><strong>Net profit (loss)</strong></span><span class="line-val"><strong>{}</strong></span></div>
</div>"#, d(est.se_income), d(est.biz_deductions + est.meals_adjustment), d(est.meals_adjustment),
            d(std::cmp::max(est.se_income - est.biz_deductions, 0))));
    }

    // ── SCHEDULE SE (if SE income) ──
    if has_se {
        let se_net = std::cmp::max(est.se_income - est.biz_deductions, 0);
        let se_taxable = (se_net as f64 * 0.9235) as i64;
        html.push_str(&format!(r#"
<div class="form-header">Schedule SE — Self-Employment Tax</div>
<div class="section">
<div class="line"><span class="line-num">2</span><span class="line-desc">Net earnings from self-employment</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">4a</span><span class="line-desc">92.35% of line 2</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">10</span><span class="line-desc">Social Security tax</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">11</span><span class="line-desc">Medicare tax</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">12</span><span class="line-desc"><strong>Total SE tax</strong></span><span class="line-val"><strong>{}</strong></span></div>
<div class="line"><span class="line-num">13</span><span class="line-desc">Deductible part (50%)</span><span class="line-val">{}</span></div>
</div>"#, d(se_net), d(se_taxable),
            d(std::cmp::min(se_taxable, 17_610_000) * 1240 / 10000),
            d(se_taxable * 290 / 10000), d(est.se_tax), d(est.half_se_tax)));
    }

    // ── SCHEDULE A (if itemizing) ──
    if has_itemized {
        html.push_str(r#"<div class="page-break"></div>"#);
        html.push_str(&format!(r#"
<div class="form-header">Schedule A — Itemized Deductions</div>
<div class="section">
<div class="line"><span class="line-num">4</span><span class="line-desc">State/local taxes (SALT, capped)</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">1</span><span class="line-desc">Medical (above 7.5% AGI)</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">17</span><span class="line-desc"><strong>Total itemized deductions</strong></span><span class="line-val"><strong>{}</strong></span></div>
</div>"#, d(est.salt_capped), d(est.medical_deductible), d(est.itemized_deduction)));
    }

    // ── SCHEDULE 3 (credits) ──
    if est.total_credits > est.child_credit + est.eitc {
        html.push_str(&format!(r#"
<div class="form-header">Schedule 3 — Additional Credits and Payments</div>
<div class="section">
<div class="line"><span class="line-num">2</span><span class="line-desc">Child and dependent care credit</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">3</span><span class="line-desc">Education credits</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">4</span><span class="line-desc">Retirement savings credit</span><span class="line-val">{}</span></div>
<div class="line"><span class="line-num">5</span><span class="line-desc">Residential energy credit</span><span class="line-val">{}</span></div>
</div>"#, d(est.cdcc), d(est.education_credits), d(est.savers_credit), d(est.energy_credits)));
    }

    // ── FILING CHECKLIST ──
    html.push_str(r#"<div class="page-break"></div>"#);
    html.push_str(&format!(r#"
<h1>Filing Checklist — {year} Tax Return</h1>
<div class="section">
<h3>Before Mailing</h3>
<ul class="checklist">
<li>☐ Sign and date Form 1040</li>
{spouse_sign}
<li>☐ Attach W-2(s) to the front of Form 1040</li>
<li>☐ Attach all schedules in order (1, 2, 3, A, B, C, D, SE)</li>
<li>☐ Include payment check if you owe (payable to "United States Treasury")</li>
<li>☐ Write SSN, tax year, and "Form 1040" on check memo line</li>
<li>☐ Keep a copy of everything for your records</li>
</ul>

<h3>Mailing Address</h3>
<div class="info-box">
{mail_addr}
</div>

<h3>Deadlines</h3>
<ul>
<li><strong>April 15, {year_plus}</strong> — Filing deadline (or October 15 with extension)</li>
<li>Singed. Singed via USPS Certified Mail recommended for proof of filing.</li>
</ul>

<h3>Summary</h3>
<div class="info-box">
<strong>Total Tax:</strong> {total_tax}<br>
<strong>Total Payments/Credits:</strong> {total_pay}<br>
<strong>{owed_label}:</strong> <strong>{owed}</strong><br>
<strong>Effective Rate:</strong> {rate}
</div>
</div>
</body></html>"#,
        spouse_sign = if spouse_first.is_some() { "<li>☐ Spouse signs and dates Form 1040</li>" } else { "" },
        mail_addr = irs_mailing_address(&st.as_deref().unwrap_or(""), est.owed >= 0),
        year_plus = year + 1,
        total_tax = d(est.total_tax), total_pay = d(est.total_payments),
        owed_label = if est.owed >= 0 { "Amount Owed" } else { "Refund" },
        owed = d(est.owed.abs()), rate = format!("{:.1}%", est.effective_rate),
    ));

    Ok(html)
}

/// IRS mailing address based on state of residence and whether payment is enclosed.
fn irs_mailing_address(state: &str, has_payment: bool) -> &'static str {
    // Simplified — IRS assigns different addresses by state group
    match (state, has_payment) {
        ("CT"|"DE"|"DC"|"GA"|"IL"|"IN"|"KY"|"ME"|"MD"|"MA"|"MI"|"NH"|"NJ"|"NY"|"NC"|"OH"|"PA"|"RI"|"SC"|"TN"|"VT"|"VA"|"WV"|"WI", true) =>
            "Internal Revenue Service<br>P.O. Box 931000<br>Louisville, KY 40293-1000",
        ("CT"|"DE"|"DC"|"GA"|"IL"|"IN"|"KY"|"ME"|"MD"|"MA"|"MI"|"NH"|"NJ"|"NY"|"NC"|"OH"|"PA"|"RI"|"SC"|"TN"|"VT"|"VA"|"WV"|"WI", false) =>
            "Department of the Treasury<br>Internal Revenue Service<br>Kansas City, MO 64999-0002",
        (_, true) =>
            "Internal Revenue Service<br>P.O. Box 802501<br>Cincinnati, OH 45280-2501",
        (_, false) =>
            "Department of the Treasury<br>Internal Revenue Service<br>Austin, TX 73301-0002",
    }
}

// ── Print Return API ────────────────────────────────────────────────────────

pub async fn handle_print_return(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<axum::response::Html<String>, (StatusCode, String)> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let principal = crate::resolve_principal(&state, token).await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthorized".to_string()))?;
    let uid = principal.user_id();
    require_tax_module(&state, uid).await?;
    let year: i64 = params.get("year").and_then(|s| s.parse().ok()).unwrap_or(2025);
    let db = state.db_path.clone();

    let html = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        generate_print_return(&conn, uid, year)
    }).await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task error".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(axum::response::Html(html))
}
