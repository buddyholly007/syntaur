//! MACE — Maurice Amazing Coder Extraordinaire.
//!
//! A Syntaur-native coding CLI. Auto-launched inside a `/coders` terminal tab
//! when the user hands off from a main agent (e.g. Peter → Maurice). Reads
//! authentication + conversation context from environment variables set by
//! the web-terminal launcher, talks to the Syntaur gateway, runs tools on
//! the local host, streams replies with a live "thinking…" timer.

mod tools;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

struct Config {
    token: String,
    conv_id: Option<String>,
    return_conv: Option<String>,
    return_agent: String,
    syntaur_url: String,
}

fn load_config() -> Result<Config> {
    let token = env::var("MAURICE_TOKEN")
        .or_else(|_| env::var("SYNTAUR_TOKEN"))
        .context(
            "No MAURICE_TOKEN in the environment. MACE is meant to be launched \
             from the Syntaur /coders module so the token is injected for you — \
             if you're seeing this at a plain shell, open Syntaur and hand off \
             from your main agent instead of running `mace` directly.",
        )?;
    let syntaur_url = env::var("MAURICE_SYNTAUR_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18789".into());
    let conv_id = env::var("MAURICE_CONV_ID").ok().filter(|s| !s.is_empty());
    let return_conv = env::var("MAURICE_RETURN_CONV").ok().filter(|s| !s.is_empty());
    let return_agent = env::var("MAURICE_RETURN_AGENT").unwrap_or_else(|_| "your main agent".into());
    Ok(Config { token, syntaur_url, conv_id, return_conv, return_agent })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl ChatMessage {
    fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into(), tool_calls: None, tool_call_id: None }
    }
}

/// Queue of chat-panel interjections detected by the background poller.
type Interjections = Arc<Mutex<Vec<String>>>;

struct Session {
    cfg: Config,
    client: reqwest::Client,
    messages: Vec<ChatMessage>,
    tool_specs: Vec<Value>,
    /// Count of user-role messages that are MACE's own (or were already on the
    /// conversation when we started). The background poller increments this
    /// atomically as it observes / accepts new user messages, so MACE never
    /// treats its own REPL input or already-seen chat-panel messages as fresh
    /// interjections.
    our_user_count: Arc<std::sync::atomic::AtomicUsize>,
    interjections: Interjections,
    exit_requested: bool,
}

impl Session {
    fn new(cfg: Config, client: reqwest::Client) -> Self {
        Self {
            cfg,
            client,
            messages: Vec::new(),
            tool_specs: tools::specs(),
            our_user_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            interjections: Arc::new(Mutex::new(Vec::new())),
            exit_requested: false,
        }
    }

    async fn sync_user_count(&self) {
        let Some(cid) = self.cfg.conv_id.as_ref() else { return; };
        let url = format!("{}/api/conversations/{}?token={}", self.cfg.syntaur_url, cid, self.cfg.token);
        let Ok(resp) = self.client.get(&url).send().await else { return; };
        if !resp.status().is_success() { return; }
        let Ok(body) = resp.json::<Value>().await else { return; };
        if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
            let count = arr
                .iter()
                .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("user"))
                .count();
            self.our_user_count.store(count, Ordering::SeqCst);
        }
    }

    /// Spawn a background task that polls the conversation every 2s and
    /// pushes any new user messages into the interjection queue. Drained by
    /// `drain_interjections` at safe points in the tool loop and REPL.
    fn spawn_interjection_poller(&self, stop: Arc<AtomicBool>) {
        let client = self.client.clone();
        let url = self
            .cfg
            .conv_id
            .as_ref()
            .map(|cid| format!("{}/api/conversations/{}?token={}", self.cfg.syntaur_url, cid, self.cfg.token));
        let Some(url) = url else { return; };
        let counter = self.our_user_count.clone();
        let queue = self.interjections.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                if stop.load(Ordering::SeqCst) { break; }
                let Ok(resp) = client.get(&url).send().await else { continue; };
                if !resp.status().is_success() { continue; }
                let Ok(body) = resp.json::<Value>().await else { continue; };
                let Some(arr) = body.get("messages").and_then(|v| v.as_array()) else { continue; };
                let user_msgs: Vec<String> = arr
                    .iter()
                    .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("user"))
                    .filter_map(|m| m.get("content").and_then(|v| v.as_str()).map(String::from))
                    .collect();
                let seen = counter.load(Ordering::SeqCst);
                if user_msgs.len() > seen {
                    let new_ones = user_msgs[seen..].to_vec();
                    counter.store(user_msgs.len(), Ordering::SeqCst);
                    let mut q = queue.lock().await;
                    q.extend(new_ones);
                }
            }
        });
    }

    async fn drain_interjections(&mut self) -> Vec<String> {
        let mut q = self.interjections.lock().await;
        std::mem::take(&mut *q)
    }

    async fn call_llm_streaming(&self) -> Result<LlmReply> {
        let url = format!("{}/api/llm/complete/stream", self.cfg.syntaur_url);
        let body = json!({
            "token": self.cfg.token,
            "agent": "maurice",
            "messages": self.messages,
            "tools": self.tool_specs,
        });
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("gateway {}: {}", status, text.chars().take(400).collect::<String>());
        }

        let start = Instant::now();
        let mut byte_stream = resp.bytes_stream();
        let mut buffer = Vec::new();
        let mut done: Option<LlmReply> = None;
        let mut err_msg: Option<String> = None;

        loop {
            tokio::select! {
                chunk = byte_stream.next() => {
                    let Some(chunk) = chunk else { break; };
                    let chunk = chunk?;
                    buffer.extend_from_slice(&chunk);
                    // SSE events end with \n\n.
                    while let Some(boundary) = find_subslice(&buffer, b"\n\n") {
                        let raw = buffer.drain(..boundary + 2).collect::<Vec<u8>>();
                        let text = String::from_utf8_lossy(&raw);
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "keepalive" { continue; }
                                let Ok(v) = serde_json::from_str::<Value>(data) else { continue; };
                                match v.get("type").and_then(|t| t.as_str()) {
                                    Some("thinking") => {
                                        let ms = v.get("elapsed_ms").and_then(|m| m.as_u64()).unwrap_or(0);
                                        redraw_status(&format!("thinking · {}s", ms / 1000));
                                    }
                                    Some("done") => {
                                        done = Some(LlmReply {
                                            content: v.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                                            tool_calls: v.get("tool_calls").and_then(|c| c.as_array()).cloned().unwrap_or_default(),
                                        });
                                    }
                                    Some("error") => {
                                        err_msg = Some(v.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error").to_string());
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if done.is_some() || err_msg.is_some() { break; }
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    // Tick the status line even between chunks so it keeps feeling alive.
                    redraw_status(&format!("thinking · {}s", start.elapsed().as_secs()));
                }
            }
        }
        clear_status();

        if let Some(e) = err_msg { anyhow::bail!("{e}"); }
        done.ok_or_else(|| anyhow::anyhow!("stream closed before 'done' event"))
    }

    async fn persist_turn(&self, role: &str, content: &str) {
        let Some(cid) = self.cfg.conv_id.as_ref() else { return; };
        let url = format!("{}/api/conversations/{}/append", self.cfg.syntaur_url, cid);
        let body = json!({ "token": self.cfg.token, "role": role, "content": content });
        let _ = self.client.post(&url).json(&body).send().await;
    }

    async fn post_return(&self, summary: &str) -> bool {
        // Mark the session as closed in the specialist conv so /coders can
        // auto-navigate back with the summary pre-filled.
        if let Some(cid) = self.cfg.conv_id.as_ref() {
            let url = format!("{}/api/conversations/{}/append", self.cfg.syntaur_url, cid);
            let body = json!({
                "token": self.cfg.token,
                "role": "system",
                "content": format!("[MACE_SESSION_CLOSED]\n{summary}"),
            });
            let _ = self.client.post(&url).json(&body).send().await;
        }
        // Post the user-visible outcome report into the origin conversation.
        let Some(rcid) = self.cfg.return_conv.as_ref() else { return false; };
        let url = format!("{}/api/conversations/{}/append", self.cfg.syntaur_url, rcid);
        let body = json!({
            "token": self.cfg.token,
            "role": "user",
            "content": format!("[maurice → {}]\n\n{}\n\nsean is heading back to you.", self.cfg.return_agent, summary),
        });
        self.client.post(&url).json(&body).send().await.is_ok()
    }

    async fn user_turn(&mut self, text: &str) -> Result<String> {
        let pending = self.drain_interjections().await;
        let combined = if pending.is_empty() {
            text.to_string()
        } else {
            let mut s = String::new();
            for p in &pending {
                s.push_str(&format!("[from chat panel] {p}\n\n"));
                println!("{}", dim(&format!("  ← chat: {}", summarize(p))));
            }
            s.push_str(text);
            s
        };
        self.messages.push(ChatMessage::user(&combined));
        self.persist_turn("user", &combined).await;
        self.our_user_count.fetch_add(1, Ordering::SeqCst);

        let mut rounds = 0usize;
        let max_rounds = 12;
        loop {
            rounds += 1;
            if rounds > max_rounds {
                anyhow::bail!("tool loop exceeded {max_rounds} rounds — try a smaller ask");
            }
            if rounds > 1 {
                let mid = self.drain_interjections().await;
                for m in mid {
                    println!("{}", dim(&format!("  ← chat (mid-task): {}", summarize(&m))));
                    self.messages.push(ChatMessage::user(format!("[from chat panel, mid-task] {m}")));
                    self.our_user_count.fetch_add(1, Ordering::SeqCst);
                }
            }

            let reply = tokio::select! {
                r = self.call_llm_streaming() => r?,
                _ = tokio::signal::ctrl_c() => {
                    clear_status();
                    println!("{}", dim("(LLM call cancelled — back at the prompt)"));
                    return Ok(String::new());
                }
            };

            if reply.tool_calls.is_empty() {
                if !reply.content.is_empty() {
                    println!("{}", reply.content);
                }
                self.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: reply.content.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                });
                self.persist_turn("assistant", &reply.content).await;
                return Ok(reply.content);
            }

            if !reply.content.is_empty() {
                println!("{}", dim(&reply.content));
            }
            self.messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.content,
                tool_calls: Some(reply.tool_calls.clone()),
                tool_call_id: None,
            });

            for tc in reply.tool_calls {
                let (id, name, args) = parse_tool_call(&tc);
                println!("{}", tool_header(&name, &args));
                let outcome = tools::run(&name, &args).await;
                let (display, tool_content, exit_summary) = match outcome {
                    Ok(tools::ToolOutput::Normal(out)) => (summarize(&out), out, None),
                    Ok(tools::ToolOutput::RequestExit { summary }) => {
                        let ack = format!("returning to {} with summary: {}", self.cfg.return_agent, summary);
                        (summarize(&ack), ack, Some(summary))
                    }
                    Err(e) => {
                        let msg = format!("error: {e}");
                        (msg.clone(), msg, None)
                    }
                };
                println!("{}", dim(&format!("  {display}")));
                self.messages.push(ChatMessage {
                    role: "tool".into(),
                    content: tool_content,
                    tool_calls: None,
                    tool_call_id: Some(id),
                });
                if let Some(summary) = exit_summary {
                    if self.post_return(&summary).await {
                        println!("{}", dim(&format!("  posted summary to {}'s conversation.", self.cfg.return_agent)));
                    }
                    self.exit_requested = true;
                    return Ok(summary);
                }
            }
        }
    }
}

struct LlmReply {
    content: String,
    tool_calls: Vec<Value>,
}

fn parse_tool_call(tc: &Value) -> (String, String, Value) {
    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = tc
        .get("function")
        .and_then(|f| f.get("name"))
        .or_else(|| tc.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let args_str = tc
        .get("function")
        .and_then(|f| f.get("arguments"))
        .or_else(|| tc.get("arguments"))
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    let args = serde_json::from_str::<Value>(args_str).unwrap_or(Value::Object(Default::default()));
    (id, name, args)
}

fn color_enabled() -> bool {
    env::var("NO_COLOR").is_err() && std::io::stdout().is_terminal()
}

fn print_banner() {
    let (d, a, r) = if color_enabled() {
        ("\x1b[38;5;244m", "\x1b[38;5;215m", "\x1b[0m")
    } else { ("", "", "") };
    println!();
    println!("{d}┌{r}");
    println!("{d}│ {a}MACE{r}  —  Maurice Amazing Coder Extraordinaire");
    println!("{d}│ {r}a Syntaur coding session");
    println!("{d}└{r}");
    println!();
}

async fn fetch_topic(client: &reqwest::Client, cfg: &Config, conv_id: &str) -> Option<String> {
    let url = format!("{}/api/conversations/{}?token={}", cfg.syntaur_url, conv_id, cfg.token);
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let body: Value = resp.json().await.ok()?;
    body.get("topic").and_then(|v| v.as_str())
        .or_else(|| body.get("title").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

fn dim(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[38;5;244m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn tool_header(name: &str, args: &Value) -> String {
    let preview = match name {
        "run_shell" => args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        "read_file" | "write_file" | "list_dir" | "edit_file" => {
            args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "ask_user" => args.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        _ => serde_json::to_string(args).unwrap_or_default(),
    };
    let short = if preview.len() > 80 { format!("{}…", &preview[..79]) } else { preview };
    if color_enabled() {
        format!("\x1b[38;5;215m▸\x1b[0m {name} \x1b[38;5;244m{short}\x1b[0m")
    } else {
        format!("▸ {name} {short}")
    }
}

fn summarize(s: &str) -> String {
    let lines = s.lines().count();
    let first = s.lines().next().unwrap_or("").chars().take(120).collect::<String>();
    if lines <= 1 { first } else { format!("{first}  … {lines} lines") }
}

/// Redraw an in-place status line. Only emits cursor-control escapes when
/// stdout is a TTY — scripted / piped output gets a plain single-line trace
/// instead of "\r\x1b[K" artifacts.
fn redraw_status(msg: &str) {
    if color_enabled() {
        print!("\r\x1b[K{}", dim(msg));
    } else if std::io::stdout().is_terminal() {
        print!("\r{}", msg);
    } else {
        // non-tty: silent so tests and logs stay clean
        return;
    }
    std::io::stdout().flush().ok();
}

fn clear_status() {
    if std::io::stdout().is_terminal() {
        print!("\r\x1b[K");
        std::io::stdout().flush().ok();
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() { return None; }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn history_path() -> Option<std::path::PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".mace_history"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cfg = load_config()?;
    print_banner();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    if let Some(cid) = cfg.conv_id.as_deref() {
        if let Some(topic) = fetch_topic(&client, &cfg, cid).await {
            println!("  context: {topic}\n");
        }
    }

    println!("  type your request. `/return` when you're done.");
    println!();

    let mut session = Session::new(cfg, client);
    let return_agent = session.cfg.return_agent.clone();
    session.sync_user_count().await;

    let stop_flag = Arc::new(AtomicBool::new(false));
    session.spawn_interjection_poller(stop_flag.clone());

    let mut rl = rustyline::DefaultEditor::new()?;
    if let Some(hp) = history_path() {
        let _ = rl.load_history(&hp);
    }

    loop {
        if session.exit_requested { break; }
        let line = match rl.readline("» ") {
            Ok(l) => l,
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("(interrupt — type /return or /exit to hand back)");
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(e) => { eprintln!("input error: {e}"); break; }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        rl.add_history_entry(trimmed).ok();

        if let Some(rest) = trimmed.strip_prefix("/return") {
            let arg = rest.trim();
            let summary = if arg.is_empty() {
                println!("{}", dim("  composing a short summary for the main agent…"));
                session.messages.push(ChatMessage::user(
                    "Compose a 2-4 sentence outcome summary for the main agent. Do not call any tools — just prose. State what was asked, what you did, and the current state.",
                ));
                session.persist_turn("user", "(internal: compose-summary request)").await;
                session.our_user_count.fetch_add(1, Ordering::SeqCst);
                match session.call_llm_streaming().await {
                    Ok(r) if !r.content.is_empty() => r.content,
                    _ => "Maurice wrapped up without generating a summary.".to_string(),
                }
            } else {
                arg.to_string()
            };
            if session.post_return(&summary).await {
                println!("\n{}", dim(&format!("posted summary to {}'s conversation.", return_agent)));
            }
            println!("handing back to {return_agent}.\n");
            break;
        }

        if matches!(trimmed, "/exit" | "/quit" | "/bye" | "exit" | "quit") {
            println!("\nhanding back to {return_agent} (no summary).");
            break;
        }

        match session.user_turn(trimmed).await {
            Ok(_) => println!(),
            Err(e) => eprintln!("{}\n", dim(&format!("[mace] {e}"))),
        }
    }

    stop_flag.store(true, Ordering::SeqCst);
    if let Some(hp) = history_path() {
        let _ = rl.save_history(&hp);
    }
    Ok(())
}
