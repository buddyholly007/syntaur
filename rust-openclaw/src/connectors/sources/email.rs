//! IMAP email connector.
//!
//! Walks the INBOX folder of one configured account and indexes recent
//! messages (last `MAX_AGE_DAYS` days, capped at `MAX_MESSAGES`). Each
//! message becomes one document with the subject as title and a flattened
//! body containing headers + the first text/plain part.
//!
//! v1 limitations:
//!   * INBOX only (no Sent, no other folders) — extending is trivial
//!   * One account per connector instance — register multiple connectors
//!     for multiple accounts
//!   * Plain-text body only — HTML parts are stripped via regex
//!   * No attachments
//!
//! Built using a minimal raw IMAP client (Rust + native-tls). The existing
//! tools/email.rs has full IMAP client code; we duplicate the connection
//! logic here rather than coupling the connector to a tool module.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use log::{debug, warn};

use regex::Regex;
use serde_json::json;

use crate::connectors::{Connector, DocIdOnly, LoadConnector, SlimConnector};
use crate::index::ExternalDoc;

const MAX_AGE_DAYS: i64 = 90;
const MAX_MESSAGES: usize = 200;

pub struct EmailConnector {
    name: String,
    account_id: String,
    host: String,
    port: u16,
    username: String,
    password: String,
}

impl EmailConnector {
    pub fn new(
        account_id: String,
        host: String,
        port: u16,
        username: String,
        password: String,
    ) -> Self {
        Self {
            name: format!("email_{}", account_id),
            account_id,
            host,
            port,
            username,
            password,
        }
    }

    /// Synchronous IMAP fetch (run inside spawn_blocking).
    fn fetch_messages(&self) -> Result<Vec<ExternalDoc>, String> {
        // Email indexing temporarily disabled during rustls migration.
        // Will be re-enabled when IMAP client is ported to rustls.
        Ok(Vec::new())
    }
}

impl Connector for EmailConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl LoadConnector for EmailConnector {
    async fn load_full(&self) -> Result<Vec<ExternalDoc>, String> {
        let host = self.host.clone();
        let port = self.port;
        let username = self.username.clone();
        let password = self.password.clone();
        let account_id = self.account_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = EmailConnector {
                name: format!("email_{}", account_id),
                account_id,
                host,
                port,
                username,
                password,
            };
            conn.fetch_messages()
        })
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?
    }
}

#[async_trait]
impl SlimConnector for EmailConnector {
    async fn list_ids(&self) -> Result<Vec<DocIdOnly>, String> {
        let docs = self.load_full().await?;
        Ok(docs
            .into_iter()
            .map(|d| DocIdOnly {
                external_id: d.external_id,
                updated_at: Some(d.updated_at),
            })
            .collect())
    }
}

// ── IMAP wire helpers ──────────────────────────────────────────────

fn read_until_tag<S: Read>(stream: &mut S, tag: &str) -> Result<String, String> {
    let mut all = String::new();
    let mut buf = [0u8; 4096];
    let mut tries = 0;
    loop {
        tries += 1;
        if tries > 200 {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                all.push_str(&String::from_utf8_lossy(&buf[..n]));
                if all.contains(&format!("{} OK", tag))
                    || all.contains(&format!("{} BAD", tag))
                    || all.contains(&format!("{} NO", tag))
                {
                    break;
                }
            }
            Err(e) => return Err(format!("read: {}", e)),
        }
    }
    Ok(all)
}

fn parse_search_ids(resp: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    for line in resp.lines() {
        if let Some(rest) = line.strip_prefix("* SEARCH") {
            for tok in rest.split_whitespace() {
                if let Ok(n) = tok.parse::<u32>() {
                    ids.push(n);
                }
            }
        }
    }
    ids
}

fn extract_fetch_body(resp: &str) -> String {
    // FETCH responses are: * N FETCH (BODY[] {SIZE}\r\n<DATA>\r\n)\r\n
    if let Some(brace) = resp.find('{') {
        if let Some(close) = resp[brace..].find('}') {
            let after_size = brace + close + 3; // skip "}\r\n"
            if after_size < resp.len() {
                // strip the trailing closing paren
                return resp[after_size..]
                    .trim_end_matches(")\r\n")
                    .to_string();
            }
        }
    }
    String::new()
}

fn parse_email(raw: &str) -> (String, String, String, String) {
    let mut subject = String::new();
    let mut from = String::new();
    let mut date = String::new();
    let mut body = String::new();
    let mut in_body = false;
    let reader = BufReader::new(raw.as_bytes());
    for line in reader.lines().flatten() {
        if !in_body {
            if line.is_empty() {
                in_body = true;
                continue;
            }
            if let Some(rest) = line.to_lowercase().strip_prefix("subject: ") {
                subject = line[9..].chars().take(200).collect::<String>();
                let _ = rest;
            } else if line.to_lowercase().starts_with("from: ") {
                from = line[6..].chars().take(200).collect::<String>();
            } else if line.to_lowercase().starts_with("date: ") {
                date = line[6..].to_string();
            }
        } else {
            body.push_str(&line);
            body.push('\n');
            if body.len() > 8000 {
                break;
            }
        }
    }
    // Strip HTML tags if present
    let stripped = strip_html(&body);
    (subject, from, date, stripped)
}

fn strip_html(s: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(s, "").to_string()
}

fn parse_email_date(date_str: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc2822(date_str)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}
