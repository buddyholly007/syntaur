//! File operation primitives.
//!
//! These are blocking, synchronous functions wrapped in `spawn_blocking` by
//! the tool dispatcher. The reference Node implementation is async-on-libuv;
//! we use std::fs from a blocking pool because that's the natural shape for
//! tokio + small file operations.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rand::RngCore;
use similar::{ChangeTag, TextDiff};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not find exact match for edit:\n{0}")]
    EditNotFound(String),
}

// ---- read ----

pub fn read_text(path: &Path) -> Result<String, OpError> {
    Ok(std::fs::read_to_string(path)?)
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>, OpError> {
    Ok(std::fs::read(path)?)
}

/// Read the first `n` lines of a text file. Reads only as much as needed.
pub fn head_file(path: &Path, n: usize) -> Result<String, OpError> {
    let f = File::open(path)?;
    let r = BufReader::new(f);
    let mut out = Vec::with_capacity(n);
    for line in r.lines().take(n) {
        out.push(line?);
    }
    Ok(out.join("\n"))
}

/// Read the last `n` lines of a text file. Streams from the end of the file
/// in 8KB chunks so we never load the whole file into memory.
pub fn tail_file(path: &Path, n: usize) -> Result<String, OpError> {
    const CHUNK: usize = 8192;

    let mut f = File::open(path)?;
    let total = f.seek(SeekFrom::End(0))?;
    if total == 0 || n == 0 {
        return Ok(String::new());
    }

    let mut buf = Vec::new();
    let mut position = total;
    let mut newlines = 0usize;

    while position > 0 && newlines <= n {
        let read = std::cmp::min(CHUNK as u64, position) as usize;
        position -= read as u64;
        f.seek(SeekFrom::Start(position))?;

        let mut chunk = vec![0u8; read];
        f.read_exact(&mut chunk)?;

        // Count newlines in this chunk and prepend it to buf.
        newlines += chunk.iter().filter(|b| **b == b'\n').count();
        chunk.append(&mut buf);
        buf = chunk;
    }

    // Convert to string lossily so we don't choke on partial UTF-8 at the
    // chunk boundary; the join below will recover.
    let text = String::from_utf8_lossy(&buf);
    let normalized = normalize_line_endings(&text);
    let lines: Vec<&str> = normalized.lines().collect();
    let take = lines.len().saturating_sub(0).min(n);
    let start = lines.len() - take;
    Ok(lines[start..].join("\n"))
}

// ---- write ----

/// Write `content` to `path`. Atomic on collision: if the target already
/// exists, write to a sibling temp file then rename. Mirrors the Node
/// reference's `wx`-then-rename pattern, which prevents writes from
/// following pre-existing symlinks.
pub fn write_file_content(path: &Path, content: &str) -> Result<(), OpError> {
    // Try exclusive create first.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(content.as_bytes())?;
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }

    // Already exists: write to temp + rename.
    let tmp = temp_sibling(path);
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.into()
    })
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", suffix));
    path.with_file_name(name)
}

// ---- edits ----

#[derive(Debug, Clone)]
pub struct EditOperation {
    pub old_text: String,
    pub new_text: String,
}

/// Normalize CRLF to LF. Mirrors the Node reference's `normalizeLineEndings`.
pub fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Apply a sequence of `oldText -> newText` edits to a file. Each edit:
///   1. Tries an exact substring replacement first.
///   2. Falls back to whitespace-flexible line-by-line matching, preserving
///      the original indentation of the first matched line.
///
/// Returns a fenced unified diff. Writes the file in place unless `dry_run`
/// is true, in which case the file is left untouched.
pub fn apply_file_edits(
    path: &Path,
    edits: &[EditOperation],
    dry_run: bool,
) -> Result<String, OpError> {
    let original = normalize_line_endings(&std::fs::read_to_string(path)?);
    let mut modified = original.clone();

    for edit in edits {
        let old_norm = normalize_line_endings(&edit.old_text);
        let new_norm = normalize_line_endings(&edit.new_text);

        if modified.contains(&old_norm) {
            modified = modified.replacen(&old_norm, &new_norm, 1);
            continue;
        }

        // Whitespace-flexible match.
        let old_lines: Vec<&str> = old_norm.split('\n').collect();
        let mut content_lines: Vec<String> =
            modified.split('\n').map(|s| s.to_string()).collect();

        let mut matched_at: Option<usize> = None;
        if old_lines.len() <= content_lines.len() {
            for i in 0..=content_lines.len() - old_lines.len() {
                let window = &content_lines[i..i + old_lines.len()];
                let is_match = old_lines
                    .iter()
                    .zip(window.iter())
                    .all(|(a, b)| a.trim() == b.trim());
                if is_match {
                    matched_at = Some(i);
                    break;
                }
            }
        }

        let i = matched_at.ok_or_else(|| OpError::EditNotFound(edit.old_text.clone()))?;

        let original_indent: String = content_lines[i]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();

        let new_lines: Vec<String> = new_norm
            .split('\n')
            .enumerate()
            .map(|(j, line)| {
                if j == 0 {
                    format!("{}{}", original_indent, line.trim_start())
                } else {
                    let old_indent: String = old_lines
                        .get(j)
                        .map(|s| s.chars().take_while(|c| c.is_whitespace()).collect())
                        .unwrap_or_default();
                    let new_indent: String =
                        line.chars().take_while(|c| c.is_whitespace()).collect();
                    if !old_indent.is_empty() && !new_indent.is_empty() {
                        let relative = new_indent.len().saturating_sub(old_indent.len());
                        format!(
                            "{}{}{}",
                            original_indent,
                            " ".repeat(relative),
                            line.trim_start()
                        )
                    } else {
                        line.to_string()
                    }
                }
            })
            .collect();

        content_lines.splice(i..i + old_lines.len(), new_lines);
        modified = content_lines.join("\n");
    }

    let diff = create_unified_diff(&original, &modified, &path.display().to_string());
    let mut backticks = 3;
    while diff.contains(&"`".repeat(backticks)) {
        backticks += 1;
    }
    let fence = "`".repeat(backticks);
    let formatted = format!("{}diff\n{}{}\n\n", fence, diff, fence);

    if !dry_run {
        let tmp = temp_sibling(path);
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(modified.as_bytes())?;
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
    }

    Ok(formatted)
}

/// Build a unified diff in the same shape as `diff.createTwoFilesPatch`.
/// We use the `similar` crate but manually format because the default
/// `similar::udiff` puts headers in a slightly different style.
pub fn create_unified_diff(original: &str, modified: &str, filepath: &str) -> String {
    let original_n = normalize_line_endings(original);
    let modified_n = normalize_line_endings(modified);

    let diff = TextDiff::from_lines(&original_n, &modified_n);
    let mut out = String::new();
    out.push_str(&format!("Index: {}\n", filepath));
    out.push_str(&format!(
        "{}\n",
        "===================================================================",
    ));
    out.push_str(&format!("--- {}\toriginal\n", filepath));
    out.push_str(&format!("+++ {}\tmodified\n", filepath));

    for group in diff.grouped_ops(3) {
        if group.is_empty() {
            continue;
        }
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        let old_start = first.old_range().start + 1;
        let old_len = last.old_range().end - first.old_range().start;
        let new_start = first.new_range().start + 1;
        let new_len = last.new_range().end - first.new_range().start;
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_len, new_start, new_len
        ));
        for op in group {
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                let mut line = change.to_string();
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                out.push_str(sign);
                out.push_str(&line);
            }
        }
    }
    out
}

// ---- stat ----

#[derive(Debug, Clone)]
pub struct FileStatInfo {
    pub size: u64,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub is_directory: bool,
    pub is_file: bool,
    pub permissions: String,
}

pub fn get_file_stats(path: &Path) -> Result<FileStatInfo, OpError> {
    let m = std::fs::metadata(path)?;
    let perms_octal = unix_perms_octal(&m);
    Ok(FileStatInfo {
        size: m.len(),
        created: m.created().ok(),
        modified: m.modified().ok(),
        accessed: m.accessed().ok(),
        is_directory: m.is_dir(),
        is_file: m.is_file(),
        permissions: perms_octal,
    })
}

#[cfg(unix)]
fn unix_perms_octal(m: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = m.permissions().mode();
    format!("{:o}", mode & 0o777)
}

#[cfg(not(unix))]
fn unix_perms_octal(_m: &std::fs::Metadata) -> String {
    "644".to_string()
}

/// Format a byte count using the same units as the Node reference:
/// `0 B`, `123 B`, `1.23 KB`, etc.
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, UNITS[idx])
    } else {
        format!("{:.2} {}", value, UNITS[idx])
    }
}

/// Format a SystemTime in the same shape as JS `Date` toString — but since
/// the Node reference just calls `String(stat.mtime)` we use ISO-ish UTC.
pub fn format_time(t: Option<SystemTime>) -> String {
    let Some(t) = t else {
        return "?".to_string();
    };
    let dur = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    chrono_like_iso(secs)
}

/// Tiny ISO-8601 UTC formatter without pulling in chrono.
fn chrono_like_iso(secs: i64) -> String {
    // Days since epoch + time components.
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;
    let (y, m, d) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hh, mm, ss
    )
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's "date" library.
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
