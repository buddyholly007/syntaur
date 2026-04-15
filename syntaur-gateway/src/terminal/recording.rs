//! Session recording — asciicast v2 format.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use log::info;

/// Handle to an active recording.
pub struct RecordingHandle {
    pub file: std::fs::File,
    pub started_at: Instant,
    pub path: PathBuf,
    pub bytes_written: usize,
}

impl RecordingHandle {
    /// Start a new recording.
    pub fn start(dir: &str, session_id: &str, cols: u16, rows: u16) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("create recording dir: {}", e))?;

        let path = PathBuf::from(dir).join(format!("{}.cast", session_id));
        let mut file = std::fs::File::create(&path)
            .map_err(|e| format!("create recording file: {}", e))?;

        // asciicast v2 header
        let header = serde_json::json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": chrono::Utc::now().timestamp(),
            "title": session_id,
            "env": { "TERM": "xterm-256color" },
        });
        writeln!(file, "{}", header).map_err(|e| format!("write header: {}", e))?;

        info!("[terminal:recording] started {}", path.display());

        Ok(Self {
            file,
            started_at: Instant::now(),
            path,
            bytes_written: 0,
        })
    }

    /// Record output data.
    pub fn record_output(&mut self, data: &[u8]) {
        let elapsed = self.started_at.elapsed().as_secs_f64();
        let escaped = serde_json::to_string(&String::from_utf8_lossy(data)).unwrap_or_default();
        let _ = writeln!(self.file, "[{:.6}, \"o\", {}]", elapsed, escaped);
        self.bytes_written += data.len();
    }

    /// Record input data.
    pub fn record_input(&mut self, data: &[u8]) {
        let elapsed = self.started_at.elapsed().as_secs_f64();
        let escaped = serde_json::to_string(&String::from_utf8_lossy(data)).unwrap_or_default();
        let _ = writeln!(self.file, "[{:.6}, \"i\", {}]", elapsed, escaped);
    }

    /// Finalize the recording.
    pub fn finish(self) -> (PathBuf, usize) {
        info!("[terminal:recording] finished {} ({} bytes)", self.path.display(), self.bytes_written);
        (self.path, self.bytes_written)
    }
}
