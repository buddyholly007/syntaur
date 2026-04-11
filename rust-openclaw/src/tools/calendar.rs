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

// ── Google Calendar (via OAuth2 token from openclaw) ────────────────────────

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

/// Read iCloud CalDAV credentials from openclaw config.
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
