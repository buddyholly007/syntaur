//! Unified calendar tool — Google Calendar + iCloud Calendar.
//!
//! Supports `read` (list upcoming events) and `add` (create an event)
//! actions across both providers. The `provider` parameter selects which
//! calendar backend to use; defaults to "google" if only Google is
//! configured, "icloud" if only iCloud is, and requires explicit selection
//! if both are configured.
//!
//! ## Credential requirements (placeholder until Sean provides them)
//!
//! ### Google Calendar
//! Requires an OAuth2 provider entry in `~/.syntaur/syntaur.json`:
//! ```json
//! "oauth": {
//!   "providers": {
//!     "google_calendar": {
//!       "authorization_url": "https://accounts.google.com/o/oauth2/v2/auth",
//!       "token_url": "https://oauth2.googleapis.com/token",
//!       "client_id": "<from Google Cloud Console>",
//!       "client_secret": "<from Google Cloud Console>",
//!       "scopes": "https://www.googleapis.com/auth/calendar",
//!       "redirect_uri": "https://<tailscale-funnel>/api/oauth/callback"
//!     }
//!   }
//! }
//! ```
//! Then connect via: `POST /api/oauth/start {token, provider: "google_calendar"}`
//! → open the returned URL in a browser → authorize → callback lands the token.
//!
//! ### iCloud Calendar
//! Requires a connector entry in `~/.syntaur/syntaur.json`:
//! ```json
//! "connectors": {
//!   "icloud": {
//!     "email": "<apple-id-email>",
//!     "app_password": "<app-specific-password from appleid.apple.com>",
//!     "enabled": true
//!   }
//! }
//! ```
//! Generate the app-specific password at https://appleid.apple.com → Sign-In
//! and Security → App-Specific Passwords.

use async_trait::async_trait;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use log::{info, warn};
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolCapabilities, ToolContext};

pub struct CalendarTool;

#[async_trait]
impl Tool for CalendarTool {
    fn name(&self) -> &str {
        "calendar"
    }

    fn description(&self) -> &str {
        "Read upcoming events or add new events to Google Calendar or iCloud Calendar. \
         Use action=read to see what's on the calendar today or this week. \
         Use action=add to create a new event with a title and time."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "add"],
                    "description": "Read upcoming events or add a new event."
                },
                "provider": {
                    "type": "string",
                    "enum": ["google", "icloud"],
                    "description": "Which calendar. Default: whichever is configured."
                },
                "days_ahead": {
                    "type": "integer",
                    "description": "For action=read: how many days ahead to show. Default: 1 (today only). Use 7 for this week."
                },
                "title": {
                    "type": "string",
                    "description": "For action=add: event title/summary."
                },
                "date": {
                    "type": "string",
                    "description": "For action=add: date as YYYY-MM-DD. Use 'tomorrow' or 'today' as shortcuts."
                },
                "start_time": {
                    "type": "string",
                    "description": "For action=add: start time as HH:MM (24h). Default: 09:00."
                },
                "end_time": {
                    "type": "string",
                    "description": "For action=add: end time as HH:MM (24h). Default: start_time + 1 hour."
                },
                "description": {
                    "type": "string",
                    "description": "For action=add: optional event description/notes."
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            read_only: false,
            network: true,
            idempotent: false,
            ..ToolCapabilities::default()
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
        let provider = args
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("google");

        match provider {
            "google" => match action {
                "read" => google_read_events(&args, ctx).await,
                "add" => google_add_event(&args, ctx).await,
                _ => Err(format!("calendar: unknown action '{}'", action)),
            },
            "icloud" => match action {
                "read" => icloud_read_events(&args, ctx).await,
                "add" => icloud_add_event(&args, ctx).await,
                _ => Err(format!("calendar: unknown action '{}'", action)),
            },
            other => Err(format!(
                "calendar: unknown provider '{}'. Use 'google' or 'icloud'.",
                other
            )),
        }
    }
}

// ── Google Calendar (via OAuth2 token from Syntaur) ────────────────────────

/// Check if Google Calendar OAuth2 is connected. Returns the access token
/// if available, or a helpful error message if not.
async fn google_get_token() -> Result<String, String> {
    // The OAuth2 token cache is in the process-global state. We need to
    // access it via the AppState, but tools don't have a direct AppState
    // reference. For now, use the OnceLock pattern similar to HA_CLIENT.
    //
    // TODO: when the OAuth token cache is wired into ToolContext (Phase 3+),
    // read from ctx instead of this placeholder path.
    //
    // For now, check if there's a persisted token file at a known location.
    let token_path = "/tmp/syntaur/google_calendar_token.json";
    match std::fs::read_to_string(token_path) {
        Ok(content) => {
            let v: Value = serde_json::from_str(&content)
                .map_err(|e| format!("parse google token: {}", e))?;
            v.get("access_token")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    "Google Calendar token file exists but has no access_token field. \
                     Re-authorize via /api/oauth/start with provider=google_calendar."
                        .to_string()
                })
        }
        Err(_) => Err(
            "Google Calendar is not connected yet. To set up:\n\
             1. Add a 'google_calendar' entry to oauth.providers in ~/.syntaur/syntaur.json \
                (needs client_id + client_secret from Google Cloud Console)\n\
             2. POST /api/oauth/start with provider=google_calendar\n\
             3. Open the returned URL in a browser and authorize\n\
             4. The callback stores the token automatically.\n\
             Sean will provide the API credentials later."
                .to_string(),
        ),
    }
}

async fn google_read_events(
    args: &Value,
    ctx: &ToolContext<'_>,
) -> Result<RichToolResult, String> {
    let token = google_get_token().await?;
    let client = ctx.http.as_ref().ok_or("no HTTP client")?;

    let days = args
        .get("days_ahead")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as i64;
    let now = Utc::now();
    let time_min = now.to_rfc3339();
    let time_max = (now + Duration::days(days)).to_rfc3339();

    let resp = client
        .get("https://www.googleapis.com/calendar/v3/calendars/primary/events")
        .query(&[
            ("timeMin", time_min.as_str()),
            ("timeMax", time_max.as_str()),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            ("maxResults", "20"),
        ])
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Google Calendar API: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            return Err(
                "Google Calendar token expired. Re-authorize via /api/oauth/start \
                 with provider=google_calendar."
                    .to_string(),
            );
        }
        return Err(format!("Google Calendar API: HTTP {} — {}", status, body.chars().take(200).collect::<String>()));
    }

    let body: Value = resp.json().await.map_err(|e| format!("parse: {}", e))?;
    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        let label = if days <= 1 { "today" } else { &format!("the next {} days", days) };
        return Ok(RichToolResult::text(format!(
            "No events on your Google Calendar for {}.",
            label
        )));
    }

    let mut lines = Vec::new();
    for item in &items {
        let summary = item
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("(no title)");
        let start = item
            .get("start")
            .and_then(|s| {
                s.get("dateTime")
                    .or(s.get("date"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("");
        // Format time nicely
        let time_str = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(start) {
            dt.format("%a %b %d %I:%M %p").to_string()
        } else {
            start.to_string()
        };
        lines.push(format!("- {} at {}", summary, time_str));
    }

    let label = if days <= 1 { "today" } else { &format!("next {} days", days) };
    Ok(RichToolResult::text(format!(
        "{} event{} {}:\n{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" },
        label,
        lines.join("\n")
    )))
}

async fn google_add_event(
    args: &Value,
    ctx: &ToolContext<'_>,
) -> Result<RichToolResult, String> {
    let token = google_get_token().await?;
    let client = ctx.http.as_ref().ok_or("no HTTP client")?;

    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("calendar add: 'title' is required")?;
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let date_str = args
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or("today");
    let date = resolve_date(date_str)?;

    let start_time = args
        .get("start_time")
        .and_then(|v| v.as_str())
        .unwrap_or("09:00");
    let end_time = args.get("end_time").and_then(|v| v.as_str());

    let start_naive = NaiveDateTime::new(
        date,
        NaiveTime::parse_from_str(start_time, "%H:%M")
            .map_err(|_| format!("bad start_time '{}', use HH:MM", start_time))?,
    );
    let end_naive = match end_time {
        Some(et) => NaiveDateTime::new(
            date,
            NaiveTime::parse_from_str(et, "%H:%M")
                .map_err(|_| format!("bad end_time '{}', use HH:MM", et))?,
        ),
        None => start_naive + Duration::hours(1),
    };

    // Use Pacific time zone offset (-7 for PDT)
    let tz_offset = "-07:00";
    let start_rfc = format!("{}T{}{}", date, start_time, tz_offset);
    let end_rfc = format!(
        "{}T{}{}",
        end_naive.date(),
        end_naive.time().format("%H:%M"),
        tz_offset
    );

    let event = json!({
        "summary": title,
        "description": description,
        "start": { "dateTime": start_rfc, "timeZone": "America/Los_Angeles" },
        "end": { "dateTime": end_rfc, "timeZone": "America/Los_Angeles" },
    });

    let resp = client
        .post("https://www.googleapis.com/calendar/v3/calendars/primary/events")
        .bearer_auth(&token)
        .json(&event)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Google Calendar API: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Google Calendar create event: HTTP {} — {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    info!(
        "[calendar:google] created event '{}' on {} at {}",
        title, date, start_time
    );
    Ok(RichToolResult::text(format!(
        "Added '{}' to Google Calendar on {} at {}.",
        title,
        date.format("%A %B %d"),
        start_time
    )))
}

// ── iCloud Calendar (CalDAV) ───────────────────────────────────────────────

/// Read iCloud CalDAV credentials from Syntaur config.
/// Returns (email, app_password) or a helpful setup error.
fn icloud_get_creds() -> Result<(String, String), String> {
    let config_path = format!(
        "{}/.syntaur/syntaur.json"/* legacy path; new installs use ~/.syntaur/ */,
        std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string())
    );
    let content = std::fs::read_to_string(&config_path)
        .map_err(|_| "Cannot read syntaur.json".to_string())?;
    let config: Value =
        serde_json::from_str(&content).map_err(|_| "Cannot parse syntaur.json".to_string())?;

    let icloud = config
        .get("connectors")
        .and_then(|c| c.get("icloud"))
        .ok_or_else(|| {
            "iCloud Calendar is not configured yet. To set up:\n\
             1. Add a 'connectors.icloud' block to ~/.syntaur/syntaur.json with:\n\
                { \"email\": \"<apple-id>\", \"app_password\": \"<app-specific-password>\", \"enabled\": true }\n\
             2. Generate app-specific password at appleid.apple.com → Sign-In and Security\n\
             3. Restart syntaur.\n\
             Sean will provide the credentials later."
                .to_string()
        })?;

    let enabled = icloud
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return Err("iCloud connector is disabled in syntaur.json (enabled=false)".to_string());
    }

    let email = icloud
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("iCloud email not set in connectors.icloud.email")?
        .to_string();
    let password = icloud
        .get("app_password")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("iCloud app_password not set in connectors.icloud.app_password")?
        .to_string();

    Ok((email, password))
}

async fn icloud_read_events(
    args: &Value,
    ctx: &ToolContext<'_>,
) -> Result<RichToolResult, String> {
    let (email, password) = icloud_get_creds()?;
    let client = ctx.http.as_ref().ok_or("no HTTP client")?;

    let days = args
        .get("days_ahead")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as i64;
    let now = Utc::now();
    let start = now.format("%Y%m%dT%H%M%SZ").to_string();
    let end = (now + Duration::days(days))
        .format("%Y%m%dT%H%M%SZ")
        .to_string();

    // CalDAV REPORT to fetch events in a time range
    let report_body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{}" end="{}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
        start, end
    );

    // iCloud CalDAV endpoint — the principal URL for calendars
    let caldav_url = format!(
        "https://caldav.icloud.com/{}/calendars/",
        urlencoded_email(&email)
    );

    let resp = client
        .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), &caldav_url)
        .header("Content-Type", "application/xml; charset=utf-8")
        .header("Depth", "1")
        .basic_auth(&email, Some(&password))
        .body(report_body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("iCloud CalDAV: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(
                "iCloud CalDAV authentication failed. Check email + app_password \
                 in connectors.icloud config."
                    .to_string(),
            );
        }
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "iCloud CalDAV: HTTP {} — {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    let body = resp.text().await.unwrap_or_default();

    // Parse VEVENT summaries + dates from the iCalendar data embedded in
    // the CalDAV multistatus response. This is a minimal regex-based parser
    // that extracts SUMMARY and DTSTART from each VEVENT block.
    let events = parse_ical_events(&body);

    if events.is_empty() {
        let label = if days <= 1 {
            "today"
        } else {
            &format!("the next {} days", days)
        };
        return Ok(RichToolResult::text(format!(
            "No events on your iCloud Calendar for {}.",
            label
        )));
    }

    let lines: Vec<String> = events
        .iter()
        .map(|(summary, dtstart)| format!("- {} at {}", summary, dtstart))
        .collect();

    Ok(RichToolResult::text(format!(
        "{} event{} on iCloud Calendar:\n{}",
        events.len(),
        if events.len() == 1 { "" } else { "s" },
        lines.join("\n")
    )))
}

async fn icloud_add_event(
    args: &Value,
    ctx: &ToolContext<'_>,
) -> Result<RichToolResult, String> {
    let (email, password) = icloud_get_creds()?;
    let client = ctx.http.as_ref().ok_or("no HTTP client")?;

    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("calendar add: 'title' is required")?;
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let date_str = args
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or("today");
    let date = resolve_date(date_str)?;
    let start_time = args
        .get("start_time")
        .and_then(|v| v.as_str())
        .unwrap_or("09:00");
    let end_time_str = args.get("end_time").and_then(|v| v.as_str());

    let start_naive = NaiveDateTime::new(
        date,
        NaiveTime::parse_from_str(start_time, "%H:%M")
            .map_err(|_| format!("bad start_time '{}'", start_time))?,
    );
    let end_naive = match end_time_str {
        Some(et) => NaiveDateTime::new(
            date,
            NaiveTime::parse_from_str(et, "%H:%M")
                .map_err(|_| format!("bad end_time '{}'", et))?,
        ),
        None => start_naive + Duration::hours(1),
    };

    let uid = format!("syntaur-{}", uuid::Uuid::new_v4());
    let now_stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let dtstart = start_naive.format("%Y%m%dT%H%M%S");
    let dtend = end_naive.format("%Y%m%dT%H%M%S");

    let ical = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//syntaur//syntaur//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         DTSTAMP:{now_stamp}\r\n\
         DTSTART;TZID=America/Los_Angeles:{dtstart}\r\n\
         DTEND;TZID=America/Los_Angeles:{dtend}\r\n\
         SUMMARY:{title}\r\n\
         DESCRIPTION:{description}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR"
    );

    // PUT the event to the user's default calendar
    let event_url = format!(
        "https://caldav.icloud.com/{}/calendars/home/{}.ics",
        urlencoded_email(&email),
        uid
    );

    let resp = client
        .put(&event_url)
        .header("Content-Type", "text/calendar; charset=utf-8")
        .basic_auth(&email, Some(&password))
        .body(ical)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("iCloud CalDAV PUT: {}", e))?;

    if resp.status().is_success() || resp.status().as_u16() == 201 {
        info!(
            "[calendar:icloud] created event '{}' on {} at {}",
            title, date, start_time
        );
        Ok(RichToolResult::text(format!(
            "Added '{}' to iCloud Calendar on {} at {}.",
            title,
            date.format("%A %B %d"),
            start_time
        )))
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!(
            "iCloud CalDAV PUT failed: HTTP {} — {}",
            status,
            body.chars().take(200).collect::<String>()
        ))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Resolve "today", "tomorrow", or a YYYY-MM-DD string to a NaiveDate.
fn resolve_date(s: &str) -> Result<NaiveDate, String> {
    let s = s.trim().to_lowercase();
    let today = Utc::now().date_naive();
    match s.as_str() {
        "today" => Ok(today),
        "tomorrow" => Ok(today + Duration::days(1)),
        _ => NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|_| format!("bad date '{}', use YYYY-MM-DD or 'today'/'tomorrow'", s)),
    }
}

/// Minimal parser for iCalendar VEVENT data inside CalDAV XML responses.
/// Returns Vec<(summary, dtstart_formatted)>.
fn parse_ical_events(xml_body: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();

    // Split on VEVENT blocks
    for chunk in xml_body.split("BEGIN:VEVENT") {
        if !chunk.contains("END:VEVENT") {
            continue;
        }

        let summary = extract_ical_field(chunk, "SUMMARY")
            .unwrap_or_else(|| "(no title)".to_string());
        let dtstart_raw = extract_ical_field(chunk, "DTSTART");
        let dtstart = dtstart_raw
            .as_deref()
            .map(format_ical_datetime)
            .unwrap_or_else(|| "(unknown time)".to_string());

        events.push((summary, dtstart));
    }

    events
}

/// Extract a field value from an iCalendar block. Handles both
/// `FIELD:value` and `FIELD;params:value` formats.
fn extract_ical_field(block: &str, field: &str) -> Option<String> {
    for line in block.lines() {
        let line = line.trim();
        if line.starts_with(field) {
            // Handle FIELD:value and FIELD;PARAM=X:value
            if let Some(colon) = line.find(':') {
                let prefix = &line[..colon];
                if prefix == field || prefix.starts_with(&format!("{};", field)) {
                    return Some(line[colon + 1..].trim().to_string());
                }
            }
        }
    }
    None
}

/// Format an iCalendar datetime string (20260410T140000 or 20260410T140000Z)
/// into a human-readable form.
fn format_ical_datetime(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .replace("Z", "")
        .replace("z", "");
    if cleaned.len() >= 13 {
        // YYYYMMDDTHHmmSS
        let date_part = &cleaned[..8];
        let time_part = &cleaned[9..13];
        if let (Ok(d), Ok(t)) = (
            NaiveDate::parse_from_str(date_part, "%Y%m%d"),
            NaiveTime::parse_from_str(&format!("{}:{}:00", &time_part[..2], &time_part[2..4]), "%H:%M:%S"),
        ) {
            return NaiveDateTime::new(d, t)
                .format("%a %b %d %I:%M %p")
                .to_string();
        }
    } else if cleaned.len() == 8 {
        // YYYYMMDD (all-day event)
        if let Ok(d) = NaiveDate::parse_from_str(&cleaned, "%Y%m%d") {
            return d.format("%a %b %d (all day)").to_string();
        }
    }
    raw.to_string()
}

fn urlencoded_email(email: &str) -> String {
    email.replace('@', "%40")
}

// ══════════════════════════════════════════════════════════════════════
// Gmail connector (reuses Google OAuth infra above) + Microsoft 365
// ══════════════════════════════════════════════════════════════════════
// Scheduler module's intake pipelines call these from handlers in main.rs.
// All config-gated: if the relevant OAuth app/credentials aren't set,
// the call returns a descriptive error and the caller shows a connect CTA
// instead of exploding.

use crate::AppState;
use std::sync::Arc;

/// Scan the user's Gmail inbox for appointment-shaped emails and land any
/// hits as `pending_approvals` rows with kind='from_email'. Returns the
/// count of new proposals created. Requires the Google OAuth token already
/// granted for calendar — upgrades the scope list to include
/// `https://www.googleapis.com/auth/gmail.readonly`.
pub async fn gmail_scan_for_proposals(state: &Arc<AppState>, user_id: i64) -> Result<i64, String> {
    let token = google_get_token().await.map_err(|e| format!("gmail requires Google OAuth: {e}"))?;
    let client = state.client.clone();
    // Pull the 20 most recent messages in the primary inbox.
    let url = "https://gmail.googleapis.com/gmail/v1/users/me/messages?labelIds=INBOX&maxResults=20&q=newer_than:14d";
    let resp = client.get(url).bearer_auth(&token).send().await.map_err(|e| format!("gmail list: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("gmail list status: {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("gmail list json: {e}"))?;
    let msgs = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let mut created = 0i64;
    for m in msgs.iter().take(10) {
        let Some(id) = m.get("id").and_then(|v| v.as_str()) else { continue; };
        let full_url = format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format=full");
        let Ok(r) = client.get(&full_url).bearer_auth(&token).send().await else { continue; };
        if !r.status().is_success() { continue; }
        let Ok(msg): Result<serde_json::Value, _> = r.json().await else { continue; };
        let headers = msg["payload"]["headers"].as_array().cloned().unwrap_or_default();
        let get_header = |name: &str| -> String {
            headers.iter().find(|h| h["name"].as_str().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false))
                .and_then(|h| h["value"].as_str()).unwrap_or("").to_string()
        };
        let subject = get_header("Subject");
        let snippet = msg["snippet"].as_str().unwrap_or("").to_string();
        if !subject_looks_like_appointment(&subject) { continue; }
        // Quick LLM parse
        let chain = crate::llm::LlmChain::from_config(&state.config, "main", state.client.clone());
        let prompt = format!("Extract calendar event from email.\nSubject: {subject}\nBody: {snippet}\n\nReturn JSON only: {{title, start_time, end_time, location}}. ISO 8601 local. 1-hour default.");
        let messages = vec![
            crate::llm::ChatMessage::system("Extract a calendar event from an email subject + snippet. JSON only."),
            crate::llm::ChatMessage::user(&prompt),
        ];
        let reply = match chain.call(&messages).await { Ok(r) => r, Err(_) => continue };
        let (s, e) = match (reply.find('{'), reply.rfind('}')) {
            (Some(s), Some(e)) if e > s => (s, e + 1),
            _ => continue,
        };
        let Ok(parsed): Result<serde_json::Value, _> = serde_json::from_str(&reply[s..e]) else { continue };
        let title = parsed["title"].as_str().unwrap_or("(email event)").to_string();
        let start = parsed["start_time"].as_str().unwrap_or("").to_string();
        let summary = if start.len() >= 16 { format!("{title} · {}", &start[..16]) } else { title.clone() };
        let source = format!("gmail:{id}");
        // Dedup — skip if we already have a pending approval for this email id.
        let db = state.db_path.clone();
        let src = source.clone();
        let exists = tokio::task::spawn_blocking(move || {
            rusqlite::Connection::open(&db).ok()
                .and_then(|c| c.query_row(
                    "SELECT 1 FROM pending_approvals WHERE user_id = ? AND source = ? LIMIT 1",
                    rusqlite::params![user_id, src], |r| r.get::<_, i64>(0)
                ).ok()).is_some()
        }).await.unwrap_or(false);
        if exists { continue; }
        let db2 = state.db_path.clone();
        let payload = parsed.clone();
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();
        let sum = summary.clone();
        let src2 = source.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = rusqlite::Connection::open(&db2) {
                let _ = conn.execute(
                    "INSERT INTO pending_approvals (user_id, kind, source, summary, payload_json, created_at) VALUES (?, 'from_email', ?, ?, ?, ?)",
                    rusqlite::params![user_id, src2, sum, payload_str, chrono::Utc::now().timestamp()],
                );
            }
        }).await.ok();
        created += 1;
    }
    Ok(created)
}

fn subject_looks_like_appointment(subject: &str) -> bool {
    let s = subject.to_lowercase();
    let hints = [
        "appointment", "confirm", "reservation", "rsvp", "you're invited",
        "you are invited", "save the date", "reminder:", "meeting", "schedule",
        "booking", "consultation", "session", "registered", "your visit",
    ];
    hints.iter().any(|h| s.contains(h))
}

/// Send a reply to the email referenced by `pending_approvals.source='gmail:<id>'`.
pub async fn gmail_send_reply(state: &Arc<AppState>, user_id: i64, approval_id: i64, body_text: &str) -> Result<(), String> {
    let token = google_get_token().await.map_err(|e| format!("gmail send requires Google OAuth: {e}"))?;
    let db = state.db_path.clone();
    let row: Option<String> = tokio::task::spawn_blocking(move || {
        rusqlite::Connection::open(&db).ok().and_then(|c| c.query_row(
            "SELECT source FROM pending_approvals WHERE id = ? AND user_id = ?",
            rusqlite::params![approval_id, user_id], |r| r.get::<_, String>(0)
        ).ok())
    }).await.unwrap_or(None);
    let Some(source) = row else { return Err("approval not found".into()); };
    let gid = source.strip_prefix("gmail:").ok_or("not a gmail-sourced approval")?.to_string();
    // Fetch the original thread headers to build a proper reply.
    let client = state.client.clone();
    let url = format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{gid}?format=metadata&metadataHeaders=From&metadataHeaders=Subject&metadataHeaders=Message-ID&metadataHeaders=References");
    let r = client.get(&url).bearer_auth(&token).send().await.map_err(|e| format!("gmail meta: {e}"))?;
    if !r.status().is_success() { return Err(format!("gmail meta status: {}", r.status())); }
    let msg: serde_json::Value = r.json().await.map_err(|e| format!("gmail meta json: {e}"))?;
    let thread_id = msg["threadId"].as_str().unwrap_or("").to_string();
    let headers = msg["payload"]["headers"].as_array().cloned().unwrap_or_default();
    let get_header = |name: &str| -> String {
        headers.iter().find(|h| h["name"].as_str().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false))
            .and_then(|h| h["value"].as_str()).unwrap_or("").to_string()
    };
    let to = get_header("From");
    let subject_orig = get_header("Subject");
    let subject = if subject_orig.to_lowercase().starts_with("re:") { subject_orig.clone() } else { format!("Re: {subject_orig}") };
    let msg_id = get_header("Message-ID");
    let references = get_header("References");
    let new_refs = if references.is_empty() { msg_id.clone() } else { format!("{references} {msg_id}") };
    // Build RFC 822 body.
    let rfc = format!(
        "To: {to}\r\nSubject: {subject}\r\nIn-Reply-To: {msg_id}\r\nReferences: {new_refs}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body_text}",
    );
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE.encode(rfc.as_bytes());
    let send_url = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";
    let body = serde_json::json!({ "raw": raw, "threadId": thread_id });
    let s = client.post(send_url).bearer_auth(&token).json(&body).send().await.map_err(|e| format!("gmail send: {e}"))?;
    if !s.status().is_success() {
        let code = s.status();
        let text = s.text().await.unwrap_or_default();
        return Err(format!("gmail send status {code}: {}", text.chars().take(200).collect::<String>()));
    }
    Ok(())
}

// ── Microsoft 365 / Graph OAuth ──────────────────────────────────────
// These functions are config-gated. If M365_CLIENT_ID / M365_CLIENT_SECRET /
// M365_TENANT_ID aren't set in the gateway env or openclaw.json, the
// functions return a descriptive error so the UI can show "Connect
// Microsoft 365" with setup instructions.

const M365_REDIRECT_PATH: &str = "/api/scheduler/m365/callback";
const M365_SCOPES: &str = "offline_access openid profile User.Read Calendars.ReadWrite Schedule.Read.All";

fn m365_config() -> Result<(String, String, String, String), String> {
    let client_id = std::env::var("M365_CLIENT_ID").map_err(|_| "M365_CLIENT_ID not set".to_string())?;
    let client_secret = std::env::var("M365_CLIENT_SECRET").map_err(|_| "M365_CLIENT_SECRET not set".to_string())?;
    let tenant = std::env::var("M365_TENANT_ID").unwrap_or_else(|_| "common".to_string());
    let redirect = std::env::var("M365_REDIRECT_URI").unwrap_or_else(|_| format!("http://127.0.0.1:18789{M365_REDIRECT_PATH}"));
    Ok((client_id, client_secret, tenant, redirect))
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub async fn m365_auth_url(_state: &Arc<AppState>) -> Result<String, String> {
    let (client_id, _secret, tenant, redirect) = m365_config()?;
    let url = format!(
        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize?client_id={client_id}&response_type=code&redirect_uri={redirect}&response_mode=query&scope={scope}&state=sch",
        tenant = tenant, client_id = client_id,
        redirect = pct_encode(&redirect), scope = pct_encode(M365_SCOPES),
    );
    Ok(url)
}

pub async fn m365_exchange_code(state: &Arc<AppState>, code: &str) -> Result<(), String> {
    let (client_id, secret, tenant, redirect) = m365_config()?;
    let client = state.client.clone();
    let form = vec![
        ("client_id", client_id.as_str()),
        ("client_secret", secret.as_str()),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("grant_type", "authorization_code"),
        ("scope", M365_SCOPES),
    ];
    let url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token");
    let r = client.post(&url).form(&form).send().await.map_err(|e| format!("token: {e}"))?;
    if !r.status().is_success() { return Err(format!("token status: {}", r.status())); }
    let body: serde_json::Value = r.json().await.map_err(|e| format!("token json: {e}"))?;
    let access = body["access_token"].as_str().unwrap_or("").to_string();
    let refresh = body["refresh_token"].as_str().unwrap_or("").to_string();
    let expires = body["expires_in"].as_i64().unwrap_or(3600);
    // Cache on disk next to the Google token for simplicity.
    let payload = serde_json::json!({
        "access_token": access, "refresh_token": refresh,
        "expires_at": chrono::Utc::now().timestamp() + expires,
    });
    let _ = tokio::fs::write("/tmp/syntaur/m365_token.json", payload.to_string()).await;
    Ok(())
}

async fn m365_get_token(state: &Arc<AppState>) -> Result<String, String> {
    let raw = tokio::fs::read_to_string("/tmp/syntaur/m365_token.json").await
        .map_err(|_| "no M365 token on disk — run /api/scheduler/m365/connect_url first".to_string())?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("bad token file: {e}"))?;
    let now = chrono::Utc::now().timestamp();
    let expires_at = v["expires_at"].as_i64().unwrap_or(0);
    if now < expires_at - 60 {
        return Ok(v["access_token"].as_str().unwrap_or("").to_string());
    }
    // Refresh.
    let (client_id, secret, tenant, _redirect) = m365_config()?;
    let refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
    if refresh.is_empty() { return Err("M365 token expired and no refresh_token".into()); }
    let client = state.client.clone();
    let form = vec![
        ("client_id", client_id.as_str()),
        ("client_secret", secret.as_str()),
        ("refresh_token", refresh.as_str()),
        ("grant_type", "refresh_token"),
        ("scope", M365_SCOPES),
    ];
    let url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token");
    let r = client.post(&url).form(&form).send().await.map_err(|e| format!("refresh: {e}"))?;
    if !r.status().is_success() { return Err(format!("refresh status: {}", r.status())); }
    let body: serde_json::Value = r.json().await.map_err(|e| format!("refresh json: {e}"))?;
    let access = body["access_token"].as_str().unwrap_or("").to_string();
    let new_refresh = body["refresh_token"].as_str().unwrap_or(&refresh).to_string();
    let expires = body["expires_in"].as_i64().unwrap_or(3600);
    let payload = serde_json::json!({
        "access_token": access.clone(), "refresh_token": new_refresh,
        "expires_at": chrono::Utc::now().timestamp() + expires,
    });
    let _ = tokio::fs::write("/tmp/syntaur/m365_token.json", payload.to_string()).await;
    Ok(access)
}

pub async fn m365_list_calendars(state: &Arc<AppState>) -> Result<Vec<serde_json::Value>, String> {
    let token = m365_get_token(state).await?;
    let r = state.client.get("https://graph.microsoft.com/v1.0/me/calendars").bearer_auth(&token).send().await
        .map_err(|e| format!("graph: {e}"))?;
    if !r.status().is_success() { return Err(format!("graph status: {}", r.status())); }
    let body: serde_json::Value = r.json().await.map_err(|e| format!("graph json: {e}"))?;
    let list = body["value"].as_array().cloned().unwrap_or_default()
        .into_iter().map(|c| serde_json::json!({
            "calendar_id": c["id"].as_str().unwrap_or(""),
            "calendar_name": c["name"].as_str().unwrap_or(""),
            "color": c["hexColor"].as_str().unwrap_or("#6366f1"),
        })).collect();
    Ok(list)
}

pub async fn m365_sync_once(state: &Arc<AppState>, user_id: i64) -> Result<i64, String> {
    let token = m365_get_token(state).await?;
    // Figure out which Outlook calendars the user asked us to sync.
    let db = state.db_path.clone();
    let subs: Vec<(String, String)> = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(String, String)>> {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT calendar_id, color FROM user_calendar_subscriptions WHERE user_id = ? AND provider = 'outlook' AND enabled = 1",
        )?;
        let iter = stmt.query_map(rusqlite::params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(iter.filter_map(Result::ok).collect())
    }).await.map_err(|e| format!("join: {e}"))?.map_err(|e| format!("sql: {e}"))?;
    let mut synced = 0i64;
    let since = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%dT00:00:00Z").to_string();
    let until = (chrono::Utc::now() + chrono::Duration::days(90)).format("%Y-%m-%dT00:00:00Z").to_string();
    for (cid, color) in subs {
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/calendars/{cid}/calendarView?startDateTime={since}&endDateTime={until}&$top=200",
            cid = cid, since = since, until = until,
        );
        let r = state.client.get(&url).bearer_auth(&token).send().await.map_err(|e| format!("events: {e}"))?;
        if !r.status().is_success() { continue; }
        let body: serde_json::Value = match r.json().await { Ok(v) => v, Err(_) => continue };
        let items = body["value"].as_array().cloned().unwrap_or_default();
        for ev in items {
            let external_id = ev["id"].as_str().unwrap_or("").to_string();
            let title = ev["subject"].as_str().unwrap_or("(untitled)").to_string();
            let start = ev["start"]["dateTime"].as_str().unwrap_or("").to_string();
            let end = ev["end"]["dateTime"].as_str().unwrap_or("").to_string();
            if external_id.is_empty() || start.is_empty() { continue; }
            let db2 = state.db_path.clone();
            let t_color = color.clone();
            let t_cid = cid.clone();
            let t_ext = external_id.clone();
            let t_title = title.clone();
            let t_start = start.clone();
            let t_end = end.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(conn) = rusqlite::Connection::open(&db2) {
                    // Upsert by (user_id, source, external_id).
                    let existing: Option<i64> = conn.query_row(
                        "SELECT id FROM calendar_events WHERE user_id = ? AND source = 'outlook' AND external_id = ?",
                        rusqlite::params![user_id, t_ext], |r| r.get(0)).ok();
                    if let Some(id) = existing {
                        let _ = conn.execute(
                            "UPDATE calendar_events SET title = ?, start_time = ?, end_time = ?, source_calendar_id = ?, color = ? WHERE id = ?",
                            rusqlite::params![t_title, t_start, t_end, t_cid, t_color, id]);
                    } else {
                        let _ = conn.execute(
                            "INSERT INTO calendar_events (user_id, title, description, start_time, end_time, all_day, source, source_calendar_id, color, external_id) \
                             VALUES (?, ?, '', ?, ?, 0, 'outlook', ?, ?, ?)",
                            rusqlite::params![user_id, t_title, t_start, t_end, t_cid, t_color, t_ext]);
                    }
                }
            }).await;
            synced += 1;
        }
    }
    Ok(synced)
}
