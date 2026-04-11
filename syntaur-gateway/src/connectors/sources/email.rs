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
use native_tls::TlsConnector;
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
        let connector = TlsConnector::new()
            .map_err(|e| format!("tls: {}", e))?;
        let stream = TcpStream::connect((self.host.as_str(), self.port))
            .map_err(|e| format!("connect {}: {}", self.host, e))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .ok();
        let mut tls = connector
            .connect(&self.host, stream)
            .map_err(|e| format!("tls handshake: {}", e))?;

        let mut buf = [0u8; 4096];
        // Read greeting
        let _ = tls.read(&mut buf);

        // LOGIN
        let login_cmd = format!(
            "a001 LOGIN \"{}\" \"{}\"\r\n",
            self.username.replace('"', "\\\""),
            self.password.replace('"', "\\\"")
        );
        tls.write_all(login_cmd.as_bytes())
            .map_err(|e| format!("login write: {}", e))?;
        let resp = read_until_tag(&mut tls, "a001")?;
        if !resp.contains(" OK") {
            return Err(format!("LOGIN failed: {}", resp));
        }

        // SELECT INBOX
        tls.write_all(b"a002 SELECT INBOX\r\n")
            .map_err(|e| format!("select write: {}", e))?;
        let _ = read_until_tag(&mut tls, "a002")?;

        // SEARCH SINCE date
        let cutoff = Utc::now() - Duration::days(MAX_AGE_DAYS);
        let since = cutoff.format("%d-%b-%Y").to_string();
        let search_cmd = format!("a003 SEARCH SINCE {}\r\n", since);
        tls.write_all(search_cmd.as_bytes())
            .map_err(|e| format!("search write: {}", e))?;
        let search_resp = read_until_tag(&mut tls, "a003")?;
        let ids = parse_search_ids(&search_resp);
        debug!(
            "[email:{}] {} messages since {}",
            self.account_id,
            ids.len(),
            since
        );
        let take = ids.iter().rev().take(MAX_MESSAGES).cloned().collect::<Vec<_>>();

        let mut docs = Vec::new();
        for id in take {
            // FETCH RFC822
            let fetch_cmd = format!("a{} FETCH {} BODY.PEEK[]\r\n", id + 100, id);
            let tag = format!("a{}", id + 100);
            tls.write_all(fetch_cmd.as_bytes())
                .map_err(|e| format!("fetch write: {}", e))?;
            let resp = match read_until_tag(&mut tls, &tag) {
                Ok(r) => r,
                Err(_) => continue,
            };
            // Extract message body between { and the closing tag
            let raw = extract_fetch_body(&resp);
            if raw.is_empty() {
                continue;
            }
            let (subject, from, date, body) = parse_email(&raw);
            if body.trim().is_empty() {
                continue;
            }
            let updated_at = parse_email_date(&date);
            docs.push(ExternalDoc {
                source: "email".to_string(),
                external_id: format!("{}/{}", self.account_id, id),
                title: format!("{} ({})", subject, from),
                body: format!(
                    "From: {}\nSubject: {}\nDate: {}\nAccount: {}\n\n{}",
                    from, subject, date, self.account_id, body
                ),
                updated_at,
                metadata: json!({
                    "account": self.account_id,
                    "imap_uid": id,
                }),
            });
        }

        // LOGOUT
        let _ = tls.write_all(b"a999 LOGOUT\r\n");
        debug!("[email:{}] indexed {} messages", self.account_id, docs.len());
        Ok(docs)
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
