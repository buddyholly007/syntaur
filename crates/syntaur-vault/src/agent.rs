//! Per-host agent daemon: owns the derived master key, serves
//! unlock/lock/get/set/list/rm/status over a 0600 Unix socket.
//!
//! The agent intentionally does ALL crypto itself — clients never see
//! the derived key. That keeps the attack surface tiny (just the
//! socket perms) and means `syntaur-vault get X` is the whole trust
//! boundary: if you can talk to the socket, you can read secrets; if
//! the socket doesn't exist, the vault is locked.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, MasterKey, NONCE_LEN, SALT_LEN};
use crate::file::VaultFile;
use crate::vault::{Entry, EntryMeta, Vault};

/// How the client addresses the agent. All fields are plain JSON —
/// the socket is 0600 so we don't need transport encryption.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentRequest {
    /// Start a new vault file (fails if one already exists at the
    /// vault path). Derives a master key from the passphrase + a fresh
    /// random salt, writes an empty encrypted payload, and keeps the
    /// agent unlocked with that key for `ttl_secs`.
    Init { passphrase: String, ttl_secs: u64 },
    /// Read the vault file, derive the master key from the passphrase
    /// + the salt embedded in the header, verify by decrypting, hold
    /// the key for `ttl_secs`.
    Unlock { passphrase: String, ttl_secs: u64 },
    /// Zero the in-memory key + exit the daemon.
    Lock,
    /// Agent state — unlocked?, ttl remaining, entry count.
    Status,
    /// Fetch a single entry's value.
    Get { name: String },
    /// Insert or overwrite an entry.
    Set {
        name: String,
        value: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        tags: Vec<String>,
    },
    /// Remove an entry. No-op if it didn't exist.
    Rm { name: String },
    /// List all entry metadata (no values).
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AgentResponse {
    Ok,
    /// `Get` response.
    Value { value: String },
    /// `Status` response. Struct variant (not tuple) because serde's
    /// internally-tagged enums only support struct + empty variants.
    Status { status: Status },
    /// `List` response.
    Entries { entries: Vec<EntryMeta> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub unlocked: bool,
    pub ttl_remaining_secs: u64,
    pub entry_count: usize,
    pub vault_path: String,
    pub socket_path: String,
}

/// Agent state held in memory. Guarded by a mutex shared across all
/// client connections.
struct AgentState {
    vault_path: PathBuf,
    socket_path: PathBuf,
    key: Option<MasterKey>,
    unlocked_until: Option<Instant>,
    /// Cache of the salt pulled from the vault header on unlock so
    /// subsequent `set` operations can re-derive… wait, we don't
    /// re-derive on set. We re-encrypt with the cached key + a fresh
    /// nonce. Salt stays the same for the life of this vault file
    /// (changing it would require a passphrase re-entry).
    salt: Option<[u8; SALT_LEN]>,
}

impl AgentState {
    fn is_unlocked(&self) -> bool {
        match (&self.key, self.unlocked_until) {
            (Some(_), Some(deadline)) => Instant::now() < deadline,
            _ => false,
        }
    }

    fn zeroize(&mut self) {
        // MasterKey Drop impl zeroes on drop.
        self.key = None;
        self.unlocked_until = None;
        self.salt = None;
    }

    fn ttl_remaining(&self) -> u64 {
        match self.unlocked_until {
            Some(deadline) => deadline
                .saturating_duration_since(Instant::now())
                .as_secs(),
            None => 0,
        }
    }

    /// Read + decrypt the vault file with the cached key. Returns
    /// Vault on success; errors if locked or ciphertext won't verify.
    fn read_vault(&self) -> Result<Vault> {
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| anyhow!("agent is locked"))?;
        let vf = VaultFile::read(&self.vault_path)?;
        let plaintext = crypto::decrypt(key.as_bytes(), &vf.nonce, &vf.ciphertext)?;
        let vault: Vault = serde_json::from_slice(&plaintext)
            .context("vault payload isn't valid JSON — format drift?")?;
        Ok(vault)
    }

    /// Serialize + encrypt + atomically write the vault. Uses a fresh
    /// nonce every time (nonce reuse with the same key breaks
    /// ChaCha20-Poly1305).
    fn write_vault(&self, vault: &Vault) -> Result<()> {
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| anyhow!("agent is locked"))?;
        let salt = self
            .salt
            .ok_or_else(|| anyhow!("agent has no salt cached"))?;
        let plaintext = serde_json::to_vec(vault)?;
        let nonce = crypto::new_nonce();
        let ciphertext = crypto::encrypt(key.as_bytes(), &nonce, &plaintext)?;
        VaultFile {
            salt,
            nonce,
            ciphertext,
        }
        .write(&self.vault_path)
    }
}

pub fn serve(vault_path: PathBuf, socket_path: PathBuf, idle_timeout: Duration) -> Result<()> {
    // Socket lives in ~/.syntaur/ at 0600. Remove any stale socket
    // from a previous run (must have been a crash; fresh listener
    // can't bind otherwise).
    if socket_path.exists() {
        fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding socket {}", socket_path.display()))?;
    set_socket_0600(&socket_path)?;

    let state = Arc::new(Mutex::new(AgentState {
        vault_path,
        socket_path: socket_path.clone(),
        key: None,
        unlocked_until: None,
        salt: None,
    }));

    // Idle-timeout thread: once a second, checks whether the unlock
    // deadline has passed and if so kills the listener. Simpler than
    // SO_RCVTIMEO dance on accept().
    let state_timeout = Arc::clone(&state);
    let socket_for_cleanup = socket_path.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut s = state_timeout.lock().unwrap();
        if s.key.is_some() && !s.is_unlocked() {
            log("idle-timeout reached — zeroing key + shutting down");
            s.zeroize();
            drop(s);
            fs::remove_file(&socket_for_cleanup).ok();
            // Listener's accept() returns an error once the socket
            // file is gone; main loop exits.
            std::process::exit(0);
        }
    });
    let _ = idle_timeout; // used indirectly via unlock request ttl_secs

    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                log(&format!("accept failed: {e}"));
                continue;
            }
        };
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            if let Err(e) = handle_client(stream, &state) {
                log(&format!("client handler error: {e}"));
            }
        });
    }
    Ok(())
}

fn handle_client(stream: UnixStream, state: &Arc<Mutex<AgentState>>) -> Result<()> {
    let stream_write = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut writer = stream_write;
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }

    let req: AgentRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            write_response(&mut writer, &AgentResponse::Error {
                message: format!("bad request: {e}"),
            })?;
            return Ok(());
        }
    };

    let resp = match req {
        AgentRequest::Init {
            passphrase,
            ttl_secs,
        } => op_init(state, &passphrase, ttl_secs),
        AgentRequest::Unlock {
            passphrase,
            ttl_secs,
        } => op_unlock(state, &passphrase, ttl_secs),
        AgentRequest::Lock => {
            let mut s = state.lock().unwrap();
            s.zeroize();
            let socket = s.socket_path.clone();
            drop(s);
            fs::remove_file(&socket).ok();
            write_response(&mut writer, &AgentResponse::Ok)?;
            // Give the response time to flush, then exit. The client
            // will see EOF on its read.
            std::thread::sleep(Duration::from_millis(50));
            std::process::exit(0);
        }
        AgentRequest::Status => op_status(state),
        AgentRequest::Get { name } => op_get(state, &name),
        AgentRequest::Set {
            name,
            value,
            description,
            notes,
            tags,
        } => op_set(state, &name, value, description, notes, tags),
        AgentRequest::Rm { name } => op_rm(state, &name),
        AgentRequest::List => op_list(state),
    };

    write_response(&mut writer, &resp)?;
    Ok(())
}

fn write_response(writer: &mut UnixStream, resp: &AgentResponse) -> Result<()> {
    let line = serde_json::to_string(resp)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn op_init(state: &Arc<Mutex<AgentState>>, passphrase: &str, ttl_secs: u64) -> AgentResponse {
    let mut s = state.lock().unwrap();
    if s.vault_path.exists() {
        return AgentResponse::Error {
            message: format!(
                "vault already exists at {} — use `unlock`, or move the old file aside to re-init",
                s.vault_path.display()
            ),
        };
    }
    let salt = crypto::new_salt();
    let key = match MasterKey::derive_from_passphrase(passphrase.as_bytes(), &salt) {
        Ok(k) => k,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("kdf failed: {e}"),
            }
        }
    };
    let vault = Vault::new();
    let plaintext = match serde_json::to_vec(&vault) {
        Ok(p) => p,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("serialize empty vault: {e}"),
            }
        }
    };
    let nonce = crypto::new_nonce();
    let ciphertext = match crypto::encrypt(key.as_bytes(), &nonce, &plaintext) {
        Ok(c) => c,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("encrypt: {e}"),
            }
        }
    };
    let vf = VaultFile {
        salt,
        nonce,
        ciphertext,
    };
    if let Err(e) = vf.write(&s.vault_path) {
        return AgentResponse::Error {
            message: format!("write vault: {e}"),
        };
    }
    s.salt = Some(salt);
    s.key = Some(key);
    s.unlocked_until = Some(Instant::now() + Duration::from_secs(ttl_secs));
    AgentResponse::Ok
}

fn op_unlock(state: &Arc<Mutex<AgentState>>, passphrase: &str, ttl_secs: u64) -> AgentResponse {
    let mut s = state.lock().unwrap();
    let vf = match VaultFile::read(&s.vault_path) {
        Ok(v) => v,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("read vault: {e}"),
            }
        }
    };
    let key = match MasterKey::derive_from_passphrase(passphrase.as_bytes(), &vf.salt) {
        Ok(k) => k,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("kdf failed: {e}"),
            }
        }
    };
    // Verify by actually decrypting. Wrong passphrase → tag mismatch.
    if let Err(e) = crypto::decrypt(key.as_bytes(), &vf.nonce, &vf.ciphertext) {
        return AgentResponse::Error {
            message: format!("unlock failed: {e}"),
        };
    }
    s.salt = Some(vf.salt);
    s.key = Some(key);
    s.unlocked_until = Some(Instant::now() + Duration::from_secs(ttl_secs));
    AgentResponse::Ok
}

fn op_status(state: &Arc<Mutex<AgentState>>) -> AgentResponse {
    let s = state.lock().unwrap();
    let unlocked = s.is_unlocked();
    let ttl = s.ttl_remaining();
    let count = if unlocked {
        s.read_vault().map(|v| v.entries.len()).unwrap_or(0)
    } else {
        0
    };
    AgentResponse::Status {
        status: Status {
            unlocked,
            ttl_remaining_secs: ttl,
            entry_count: count,
            vault_path: s.vault_path.display().to_string(),
            socket_path: s.socket_path.display().to_string(),
        },
    }
}

fn op_get(state: &Arc<Mutex<AgentState>>, name: &str) -> AgentResponse {
    let s = state.lock().unwrap();
    if !s.is_unlocked() {
        return AgentResponse::Error {
            message: "agent is locked — run `syntaur-vault unlock` first".into(),
        };
    }
    let vault = match s.read_vault() {
        Ok(v) => v,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("read vault: {e}"),
            }
        }
    };
    match vault.get(name) {
        Some(e) => AgentResponse::Value {
            value: e.value.clone(),
        },
        None => AgentResponse::Error {
            message: format!("no entry {name:?} — run `syntaur-vault list`"),
        },
    }
}

fn op_set(
    state: &Arc<Mutex<AgentState>>,
    name: &str,
    value: String,
    description: String,
    notes: String,
    tags: Vec<String>,
) -> AgentResponse {
    let s = state.lock().unwrap();
    if !s.is_unlocked() {
        return AgentResponse::Error {
            message: "agent is locked".into(),
        };
    }
    let mut vault = match s.read_vault() {
        Ok(v) => v,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("read vault: {e}"),
            }
        }
    };
    let now = chrono::Utc::now();
    let entry = match vault.entries.get(name) {
        Some(existing) => Entry {
            value,
            description: if description.is_empty() {
                existing.description.clone()
            } else {
                description
            },
            notes: if notes.is_empty() {
                existing.notes.clone()
            } else {
                notes
            },
            tags: if tags.is_empty() {
                existing.tags.clone()
            } else {
                tags
            },
            created_at: existing.created_at,
            updated_at: now,
        },
        None => Entry {
            value,
            description,
            notes,
            tags,
            created_at: now,
            updated_at: now,
        },
    };
    vault.insert(name, entry);
    if let Err(e) = s.write_vault(&vault) {
        return AgentResponse::Error {
            message: format!("write: {e}"),
        };
    }
    AgentResponse::Ok
}

fn op_rm(state: &Arc<Mutex<AgentState>>, name: &str) -> AgentResponse {
    let s = state.lock().unwrap();
    if !s.is_unlocked() {
        return AgentResponse::Error {
            message: "agent is locked".into(),
        };
    }
    let mut vault = match s.read_vault() {
        Ok(v) => v,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("read vault: {e}"),
            }
        }
    };
    vault.remove(name);
    if let Err(e) = s.write_vault(&vault) {
        return AgentResponse::Error {
            message: format!("write: {e}"),
        };
    }
    AgentResponse::Ok
}

fn op_list(state: &Arc<Mutex<AgentState>>) -> AgentResponse {
    let s = state.lock().unwrap();
    if !s.is_unlocked() {
        return AgentResponse::Error {
            message: "agent is locked".into(),
        };
    }
    let vault = match s.read_vault() {
        Ok(v) => v,
        Err(e) => {
            return AgentResponse::Error {
                message: format!("read vault: {e}"),
            }
        }
    };
    let entries = vault
        .entries
        .iter()
        .map(|(name, e)| EntryMeta::from_entry(name, e))
        .collect();
    AgentResponse::Entries { entries }
}

#[cfg(unix)]
fn set_socket_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, perms)
        .with_context(|| format!("chmod 0600 {}", path.display()))
}

#[cfg(not(unix))]
fn set_socket_0600(_: &Path) -> Result<()> {
    Ok(())
}

fn log(msg: &str) {
    eprintln!("[syntaur-vault agent] {msg}");
}

/// Client-side helper: connect to the agent, send one request, read
/// one response. All CLI commands go through this.
pub fn request(socket_path: &Path, req: &AgentRequest) -> Result<AgentResponse> {
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "connecting to vault agent at {} (try `syntaur-vault unlock`)",
            socket_path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let line = serde_json::to_string(req)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    let resp: AgentResponse = serde_json::from_str(buf.trim())
        .with_context(|| format!("parsing agent response: {buf:?}"))?;
    Ok(resp)
}
