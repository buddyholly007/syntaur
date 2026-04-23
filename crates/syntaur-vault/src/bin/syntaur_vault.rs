//! `syntaur-vault` — CLI for Sean's encrypted secret store.
//!
//! Thin wrapper over [`syntaur_vault_core::agent`]. The crypto +
//! storage happens in the per-host agent daemon so the derived key
//! never leaves that process. The CLI binary is a client.
//!
//! Flows:
//!   syntaur-vault init              # first-time setup
//!   syntaur-vault unlock            # start agent + cache key
//!   syntaur-vault get openrouter    # print a secret
//!   syntaur-vault set NAME          # read value from stdin/prompt
//!   syntaur-vault list              # names + metadata
//!   syntaur-vault lock              # stop agent + zero key
//!   syntaur-vault status            # agent state

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};

use syntaur_vault_core::{
    agent::{self, AgentRequest, AgentResponse},
    default_pidfile_path, default_socket_path, default_vault_path,
};

#[derive(Parser, Debug)]
#[command(
    name = "syntaur-vault",
    version,
    about = "Encrypted secret store for personal API keys + tokens",
    long_about = None,
)]
struct Cli {
    /// Override vault file path (otherwise $SYNTAUR_VAULT_PATH or
    /// ~/vault/syntaur-vault.enc).
    #[arg(long, global = true)]
    vault_path: Option<PathBuf>,

    /// Override agent socket path (otherwise $SYNTAUR_VAULT_SOCKET or
    /// ~/.syntaur/vault.sock).
    #[arg(long, global = true)]
    socket_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new vault file at the vault path. Fails if one
    /// already exists there. Sets the passphrase and leaves the agent
    /// unlocked.
    Init {
        #[arg(long, default_value = "1800")]
        ttl_secs: u64,
    },
    /// Start the agent (if not already running) and unlock the vault
    /// with the passphrase.
    Unlock {
        #[arg(long, default_value = "1800")]
        ttl_secs: u64,
    },
    /// Tell the agent to zero the in-memory key and exit.
    Lock,
    /// Print agent state + entry count.
    Status,
    /// Fetch a secret's value. Prints on stdout with no trailing
    /// newline so command substitution (`$(syntaur-vault get X)`)
    /// works cleanly.
    Get {
        name: String,
    },
    /// Insert or overwrite a secret. Value comes from stdin if piped;
    /// otherwise prompts interactively (hidden input).
    Set {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Comma-separated tags.
        #[arg(long, default_value = "")]
        tags: String,
    },
    /// Remove a secret. Silent no-op if it doesn't exist.
    Rm {
        name: String,
    },
    /// List all entry metadata (no values).
    List {
        /// Output as JSON (default: human table).
        #[arg(long)]
        json: bool,
    },
    /// Run the agent daemon in the foreground. Used internally by
    /// `unlock`/`init` which spawn a detached child of this. Users
    /// usually don't call this directly.
    Agent {
        /// Daemonize via double-fork before serving. Set by the
        /// spawn path; no daemonize when running in the foreground
        /// for debugging.
        #[arg(long, hide = true)]
        detach: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let vault_path = cli.vault_path.unwrap_or_else(default_vault_path);
    let socket_path = cli.socket_path.unwrap_or_else(default_socket_path);

    let result = match cli.command {
        Commands::Agent { detach } => run_agent(vault_path, socket_path, detach),
        Commands::Init { ttl_secs } => cmd_init(&socket_path, &vault_path, ttl_secs),
        Commands::Unlock { ttl_secs } => cmd_unlock(&socket_path, &vault_path, ttl_secs),
        Commands::Lock => cmd_lock(&socket_path),
        Commands::Status => cmd_status(&socket_path),
        Commands::Get { name } => cmd_get(&socket_path, &name),
        Commands::Set {
            name,
            description,
            notes,
            tags,
        } => cmd_set(&socket_path, &name, &description, &notes, &tags),
        Commands::Rm { name } => cmd_rm(&socket_path, &name),
        Commands::List { json } => cmd_list(&socket_path, json),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("syntaur-vault: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run_agent(vault_path: PathBuf, socket_path: PathBuf, detach: bool) -> Result<()> {
    if detach {
        let pidfile = default_pidfile_path();
        if let Some(parent) = pidfile.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        daemonize::Daemonize::new()
            .pid_file(&pidfile)
            .chown_pid_file(false)
            .working_directory("/")
            .umask(0o077)
            .start()
            .context("daemonize failed")?;
    }
    agent::serve(vault_path, socket_path, Duration::from_secs(1800))
}

fn ensure_agent_running(socket_path: &std::path::Path) -> Result<()> {
    if socket_path.exists() {
        return Ok(());
    }
    let me = std::env::current_exe().context("resolving own path")?;
    // Detached child: closes stdio, double-forks inside `run_agent`.
    Command::new(&me)
        .arg("agent")
        .arg("--detach")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawning agent subprocess")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "agent didn't bind {} within 3s — check `{}` stderr by running `syntaur-vault agent` in the foreground",
        socket_path.display(),
        std::env::current_exe().unwrap_or_default().display()
    )
}

fn prompt_passphrase(prompt: &str) -> Result<String> {
    let pw = if io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).context("reading passphrase")?
    } else {
        // Non-TTY stdin: read one line plain. Enables scripted flows
        // like `printf "$PASS\n" | syntaur-vault unlock`.
        let mut buf = String::new();
        io::stdin()
            .read_line(&mut buf)
            .context("reading passphrase from stdin")?;
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        buf
    };
    if pw.is_empty() {
        bail!("empty passphrase");
    }
    Ok(pw)
}

fn cmd_init(socket: &std::path::Path, vault: &std::path::Path, ttl: u64) -> Result<()> {
    if vault.exists() {
        bail!(
            "vault already exists at {} — use `unlock`, or move the old file aside to re-init",
            vault.display()
        );
    }
    let pw1 = prompt_passphrase("new vault passphrase: ")?;
    let pw2 = prompt_passphrase("confirm passphrase: ")?;
    if pw1 != pw2 {
        bail!("passphrases didn't match");
    }
    ensure_agent_running(socket)?;
    match agent::request(
        socket,
        &AgentRequest::Init {
            passphrase: pw1,
            ttl_secs: ttl,
        },
    )? {
        AgentResponse::Ok => {
            println!("✓ vault initialized at {}", vault.display());
            println!("  unlocked for {ttl}s; run `syntaur-vault set NAME` to add secrets.");
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_unlock(socket: &std::path::Path, vault: &std::path::Path, ttl: u64) -> Result<()> {
    if !vault.exists() {
        bail!(
            "no vault at {} — run `syntaur-vault init` first",
            vault.display()
        );
    }
    let pw = prompt_passphrase("vault passphrase: ")?;
    ensure_agent_running(socket)?;
    match agent::request(
        socket,
        &AgentRequest::Unlock {
            passphrase: pw,
            ttl_secs: ttl,
        },
    )? {
        AgentResponse::Ok => {
            println!("✓ unlocked for {ttl}s");
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_lock(socket: &std::path::Path) -> Result<()> {
    if !socket.exists() {
        println!("agent not running; nothing to lock.");
        return Ok(());
    }
    match agent::request(socket, &AgentRequest::Lock) {
        Ok(AgentResponse::Ok) => {
            println!("✓ locked; agent exited");
            Ok(())
        }
        // Lock forces the agent to exit, so reading the response may
        // race — ignore EOF errors.
        Err(_) | Ok(_) => {
            println!("✓ locked");
            Ok(())
        }
    }
}

fn cmd_status(socket: &std::path::Path) -> Result<()> {
    if !socket.exists() {
        println!("status: locked (agent not running)");
        return Ok(());
    }
    match agent::request(socket, &AgentRequest::Status)? {
        AgentResponse::Status { status: s } => {
            let st = if s.unlocked { "unlocked" } else { "locked" };
            println!("status:       {st}");
            println!("vault path:   {}", s.vault_path);
            println!("socket:       {}", s.socket_path);
            println!("entries:      {}", s.entry_count);
            if s.unlocked {
                let m = s.ttl_remaining_secs / 60;
                let ss = s.ttl_remaining_secs % 60;
                println!("unlock ttl:   {m}m {ss}s remaining");
            }
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_get(socket: &std::path::Path, name: &str) -> Result<()> {
    match agent::request(socket, &AgentRequest::Get { name: name.into() })? {
        AgentResponse::Value { value } => {
            // Deliberately no trailing newline — command substitution
            // with backticks/`$(…)` would eat trailing newlines
            // anyway, but this keeps pipelines exact.
            print!("{value}");
            if io::stdout().is_terminal() {
                println!();
            }
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_set(
    socket: &std::path::Path,
    name: &str,
    description: &str,
    notes: &str,
    tags: &str,
) -> Result<()> {
    let value = if io::stdin().is_terminal() {
        prompt_passphrase(&format!("value for {name:?}: "))?
    } else {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).context("reading stdin")?;
        // Trim ONE trailing newline so `echo sk-... | vault set` works.
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        buf
    };
    if value.is_empty() {
        bail!("empty value — refusing to store");
    }
    let tags_vec: Vec<String> = if tags.is_empty() {
        Vec::new()
    } else {
        tags.split(',').map(|s| s.trim().to_string()).collect()
    };
    match agent::request(
        socket,
        &AgentRequest::Set {
            name: name.into(),
            value,
            description: description.into(),
            notes: notes.into(),
            tags: tags_vec,
        },
    )? {
        AgentResponse::Ok => {
            println!("✓ set {name}");
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_rm(socket: &std::path::Path, name: &str) -> Result<()> {
    match agent::request(socket, &AgentRequest::Rm { name: name.into() })? {
        AgentResponse::Ok => {
            println!("✓ rm {name}");
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}

fn cmd_list(socket: &std::path::Path, json: bool) -> Result<()> {
    match agent::request(socket, &AgentRequest::List)? {
        AgentResponse::Entries { entries } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
                return Ok(());
            }
            if entries.is_empty() {
                println!("(no entries — `syntaur-vault set NAME` to add one)");
                return Ok(());
            }
            println!(
                "{:<24}  {:<3}  {:<20}  {}",
                "NAME", "LEN", "TAGS", "DESCRIPTION"
            );
            for e in &entries {
                let tags = e.tags.join(",");
                println!(
                    "{:<24}  {:>3}  {:<20}  {}",
                    e.name, e.value_len, tags, e.description
                );
            }
            Ok(())
        }
        AgentResponse::Error { message } => Err(anyhow!("{message}")),
        other => Err(anyhow!("unexpected response: {other:?}")),
    }
}
