//! Coders terminal module — web-based SSH/terminal client.
//!
//! Manages local PTY sessions and remote SSH connections, bridging
//! terminal I/O to browser WebSocket connections. Sessions survive
//! page reloads via a scrollback ring buffer.

pub mod hosts;
pub mod pty;
pub mod recording;
pub mod session;
pub mod sftp;
pub mod ssh;
pub mod ws;
pub mod forwarding;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aes_gcm::{Aes256Gcm, Key};
use bytes::Bytes;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};

/// Central manager for all terminal sessions.
pub struct TerminalManager {
    pub sessions: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<LiveSession>>>>>,
    pub db_path: PathBuf,
    pub master_key: Key<Aes256Gcm>,
    pub config: TerminalConfig,
}

impl TerminalManager {
    pub fn new(
        db_path: PathBuf,
        master_key: Key<Aes256Gcm>,
        config: TerminalConfig,
    ) -> Self {
        info!(
            "[terminal] manager ready (max_sessions={}, scrollback={}KB)",
            config.max_sessions,
            config.scrollback_bytes / 1024
        );
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            db_path,
            master_key,
            config,
        }
    }
}

/// Per-session state held in memory.
pub struct LiveSession {
    pub id: String,
    pub host_id: i64,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: RingBuffer,
    pub output_tx: broadcast::Sender<Bytes>,
    pub input_tx: mpsc::Sender<Bytes>,
    pub created_at: std::time::Instant,
    pub last_active: std::time::Instant,
    pub backend: SessionBackend,
    pub recording: Option<recording::RecordingHandle>,
}

pub enum SessionBackend {
    LocalPty {
        master_fd: std::os::unix::io::RawFd,
        child_pid: u32,
    },
    Ssh {
        client: Arc<ssh::SshClient>,
    },
}

impl SessionBackend {
    pub fn master_fd(&self) -> Option<std::os::unix::io::RawFd> {
        match self {
            Self::LocalPty { master_fd, .. } => Some(*master_fd),
            Self::Ssh { .. } => None,
        }
    }
}

/// Simple ring buffer for scrollback.
pub struct RingBuffer {
    buf: Vec<u8>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        for byte in data {
            if self.buf.len() >= self.capacity {
                self.buf.remove(0);
            }
            self.buf.push(*byte);
        }
    }

    /// Bulk push — more efficient for large chunks.
    pub fn extend(&mut self, data: &[u8]) {
        if data.len() >= self.capacity {
            // Data larger than buffer — just keep the tail
            let start = data.len() - self.capacity;
            self.buf.clear();
            self.buf.extend_from_slice(&data[start..]);
            return;
        }
        let new_len = self.buf.len() + data.len();
        if new_len > self.capacity {
            let remove = new_len - self.capacity;
            self.buf.drain(..remove);
        }
        self.buf.extend_from_slice(data);
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Module config from syntaur.json modules.entries["mod-coders"].config
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalConfig {
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_scrollback_bytes")]
    pub scrollback_bytes: usize,
    #[serde(default = "default_session_timeout_minutes")]
    pub session_timeout_minutes: u64,
    #[serde(default)]
    pub recording_enabled: bool,
    #[serde(default = "default_recording_dir")]
    pub recording_dir: String,
    #[serde(default = "default_sftp_max_upload_mb")]
    pub sftp_max_upload_mb: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            max_sessions: default_max_sessions(),
            scrollback_bytes: default_scrollback_bytes(),
            session_timeout_minutes: default_session_timeout_minutes(),
            recording_enabled: false,
            recording_dir: default_recording_dir(),
            sftp_max_upload_mb: default_sftp_max_upload_mb(),
        }
    }
}

fn default_max_sessions() -> usize { 20 }
fn default_scrollback_bytes() -> usize { 65536 }
fn default_session_timeout_minutes() -> u64 { 1440 }
fn default_recording_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
    format!("{}/.syntaur/recordings", home)
}
fn default_sftp_max_upload_mb() -> usize { 100 }

/// Host definition from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalHost {
    pub id: i64,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub private_key: Option<String>,
    pub password: Option<String>,
    pub jump_host_id: Option<i64>,
    pub default_shell: String,
    pub group_name: String,
    pub tags: String,
    pub color: String,
    pub sort_order: i32,
    pub is_local: bool,
    pub favorite: bool,
}

/// Snippet definition from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSnippet {
    pub id: i64,
    pub name: String,
    pub command: String,
    pub variables: String,
    pub tags: String,
    pub folder: String,
}

/// Port forward definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortForward {
    pub id: i64,
    pub host_id: i64,
    pub direction: String,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub auto_start: bool,
}
