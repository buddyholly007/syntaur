//! HTTP API endpoints for the Voice Journal module.
//!
//! - GET /api/journal?date=YYYY-MM-DD — get journal for a date (default today)
//! - GET /api/journal/dates — list available journal dates
//! - GET /api/journal/search?q=term&max=5 — search across journals
//! - GET /api/journal/sessions?limit=20 — list recording sessions

use axum::extract::Query;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::voice_journal;

#[derive(Deserialize)]
pub struct JournalQuery {
    pub date: Option<String>,
}

pub async fn get_journal(Query(q): Query<JournalQuery>) -> Json<Value> {
    let date = q.date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let dir = voice_journal::config().data_dir().join("journal");
    let path = dir.join(format!("{}.md", date));

    match std::fs::read_to_string(&path) {
        Ok(content) => Json(json!({
            "date": date,
            "content": content,
            "entries": content.lines()
                .filter(|l| l.starts_with("**"))
                .count(),
        })),
        Err(_) => Json(json!({
            "date": date,
            "content": null,
            "entries": 0,
        })),
    }
}

pub async fn get_journal_dates() -> Json<Value> {
    let dir = voice_journal::config().data_dir().join("journal");
    let mut dates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
    }
    dates.sort();
    dates.reverse();
    Json(json!({ "dates": dates }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub max: Option<usize>,
}

pub async fn search_journal(Query(q): Query<SearchQuery>) -> Json<Value> {
    let max = q.max.unwrap_or(5);
    let dir = voice_journal::config().data_dir().join("journal");
    let query_lower = q.q.to_lowercase();
    let mut results = Vec::new();

    // Get sorted dates
    let mut dates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
    }
    dates.sort();
    dates.reverse();

    for date in &dates {
        let path = dir.join(format!("{}.md", date));
        if let Ok(content) = std::fs::read_to_string(&path) {
            let matches: Vec<&str> = content.lines()
                .filter(|l| l.to_lowercase().contains(&query_lower))
                .collect();
            if !matches.is_empty() {
                results.push(json!({
                    "date": date,
                    "matches": matches,
                    "count": matches.len(),
                }));
            }
            if results.len() >= max {
                break;
            }
        }
    }

    Json(json!({
        "query": q.q,
        "results": results,
        "total_days": results.len(),
    }))
}

#[derive(Deserialize)]
pub struct SessionsQuery {
    pub limit: Option<usize>,
}

pub async fn get_sessions(Query(q): Query<SessionsQuery>) -> Json<Value> {
    let limit = q.limit.unwrap_or(20);
    let sessions = voice_journal::load_sessions();
    let recent: Vec<_> = sessions.iter().rev().take(limit).collect();

    let total_duration: f64 = sessions.iter().map(|s| s.duration_secs).sum();
    let total_clips: usize = sessions.iter().map(|s| s.training_clips).sum();

    Json(json!({
        "sessions": recent,
        "total": sessions.len(),
        "total_duration_hours": total_duration / 3600.0,
        "total_training_clips": total_clips,
    }))
}
