//! On-disk vault file format.
//!
//! Layout (little-endian / network-order irrelevant because all fields
//! are byte arrays — no multi-byte integers in the header):
//!
//! ```text
//! [4 bytes magic = b"SVLT"]
//! [1 byte  format version]
//! [32 bytes argon2 salt]
//! [12 bytes chacha20poly1305 nonce]
//! [N bytes AEAD ciphertext + 16-byte Poly1305 tag]
//! ```
//!
//! The whole file is rewritten on every `set`/`rm`. Atomic-replace via
//! write-to-temp + rename so NFS + a crash during write can't produce
//! a half-written file.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::crypto::{NONCE_LEN, SALT_LEN};
use crate::FORMAT_VERSION;

pub const MAGIC: &[u8; 4] = b"SVLT";
pub const HEADER_LEN: usize = 4 + 1 + SALT_LEN + NONCE_LEN;

pub struct VaultFile {
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl VaultFile {
    /// Read + parse the header. Returns an error with a clear message
    /// if the file isn't a SVLT vault.
    pub fn read(path: &Path) -> Result<Self> {
        let mut f = fs::File::open(path)
            .with_context(|| format!("opening vault file {}", path.display()))?;

        let mut header = [0u8; HEADER_LEN];
        f.read_exact(&mut header)
            .context("vault file too short — is it really a vault file?")?;

        if &header[0..4] != MAGIC {
            bail!(
                "vault file {} doesn't start with SVLT magic — wrong file?",
                path.display()
            );
        }
        let version = header[4];
        if version != FORMAT_VERSION {
            bail!(
                "vault file is format v{}, this tool supports v{}",
                version,
                FORMAT_VERSION
            );
        }

        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&header[5..5 + SALT_LEN]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&header[5 + SALT_LEN..HEADER_LEN]);

        let mut ciphertext = Vec::new();
        f.read_to_end(&mut ciphertext).context("reading vault ciphertext")?;

        Ok(Self {
            salt,
            nonce,
            ciphertext,
        })
    }

    /// Atomically write the vault file: temp-write + rename. Produces
    /// mode 0600 so only the owner can read. Parent directory is
    /// created if missing.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir -p {}", parent.display()))?;
        }

        let tmp = temp_path(path);
        {
            let mut f = open_0600(&tmp)?;
            f.write_all(MAGIC)?;
            f.write_all(&[FORMAT_VERSION])?;
            f.write_all(&self.salt)?;
            f.write_all(&self.nonce)?;
            f.write_all(&self.ciphertext)?;
            f.sync_all().context("fsync vault tempfile")?;
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(unix)]
fn open_0600(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| anyhow!("opening {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn open_0600(path: &Path) -> Result<fs::File> {
    fs::File::create(path).map_err(|e| anyhow!("opening {}: {e}", path.display()))
}

