use chrono::Utc;
use log::warn;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Clone)]
pub struct AuditEvent {
    pub ts: String,
    pub event: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

pub struct AuditLogger {
    dir: PathBuf,
    current_date: Mutex<String>,
    file: Mutex<Option<File>>,
    secrets: Vec<String>, // patterns to redact
}

impl AuditLogger {
    pub fn new(dir: PathBuf, secrets: Vec<String>) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self {
            dir,
            current_date: Mutex::new(String::new()),
            file: Mutex::new(None),
            secrets,
        }
    }

    fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for secret in &self.secrets {
            if secret.len() > 8 && result.contains(secret) {
                let replacement = format!("{}...{}", &secret[..4], &secret[secret.len()-4..]);
                result = result.replace(secret, &replacement);
            }
        }
        result
    }

    pub fn log(&self, event: AuditEvent) {
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Rotate file if date changed
        {
            let mut current = self.current_date.lock().unwrap();
            if *current != today {
                *current = today.clone();
                let path = self.dir.join(format!("audit-{}.jsonl", today));
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path);
                let mut f = self.file.lock().unwrap();
                *f = file.ok();
            }
        }

        // Serialize and redact
        if let Ok(json) = serde_json::to_string(&event) {
            let redacted = self.redact(&json);

            let mut file = self.file.lock().unwrap();
            if let Some(ref mut f) = *file {
                let _ = writeln!(f, "{}", redacted);
            }
        }
    }
}
