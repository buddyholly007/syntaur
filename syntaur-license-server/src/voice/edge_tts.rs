//! Built-in Edge TTS — Microsoft's free neural text-to-speech.
//!
//! Uses the `edge-tts` Python CLI (pip install edge-tts) which handles
//! Microsoft's DRM/fingerprinting. Falls back to a pure-Rust WebSocket
//! implementation if the CLI is unavailable.
//!
//! Default voice: en-US-AriaNeural. Full list: `edge-tts --list-voices`

use log::{debug, info, warn};
use std::process::Stdio;

/// Available built-in voices (short name → Edge voice ID).
pub const VOICES: &[(&str, &str)] = &[
    ("aria", "en-US-AriaNeural"),
    ("guy", "en-US-GuyNeural"),
    ("jenny", "en-US-JennyNeural"),
    ("davis", "en-US-DavisNeural"),
    ("ana", "en-US-AnaNeural"),
    ("andrew", "en-US-AndrewNeural"),
    ("emma", "en-US-EmmaNeural"),
    ("brian", "en-US-BrianNeural"),
    // OpenAI voice name aliases
    ("alloy", "en-US-AriaNeural"),
    ("nova", "en-US-JennyNeural"),
    ("echo", "en-US-GuyNeural"),
    ("onyx", "en-US-DavisNeural"),
    ("shimmer", "en-US-EmmaNeural"),
];

const DEFAULT_VOICE: &str = "en-US-AriaNeural";

/// Resolve a voice name to an Edge TTS voice ID.
pub fn resolve_voice(name: &str) -> &str {
    let lower = name.to_lowercase();
    VOICES
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, id)| *id)
        .unwrap_or_else(|| {
            if name.contains('-') && name.contains("Neural") {
                name
            } else {
                DEFAULT_VOICE
            }
        })
}

/// Check if edge-tts CLI is available.
pub fn is_available() -> bool {
    std::process::Command::new("edge-tts")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Generate speech audio using the edge-tts CLI.
/// Returns MP3 audio bytes.
pub async fn synthesize(text: &str, voice: &str) -> Result<Vec<u8>, String> {
    let voice_id = resolve_voice(voice);
    let tmp_path = format!("/tmp/syntaur_tts_{}.mp3", uuid::Uuid::new_v4());

    debug!(
        "[edge-tts] synthesizing voice={} text_len={} → {}",
        voice_id,
        text.len(),
        tmp_path
    );

    let output = tokio::process::Command::new("edge-tts")
        .arg("--voice")
        .arg(voice_id)
        .arg("--text")
        .arg(text)
        .arg("--write-media")
        .arg(&tmp_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "edge-tts not installed (pip install edge-tts)".to_string()
            } else {
                format!("edge-tts exec: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up temp file on error
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!("edge-tts failed: {}", stderr.trim()));
    }

    // Read the generated audio
    let audio = tokio::fs::read(&tmp_path)
        .await
        .map_err(|e| format!("read tts output: {}", e))?;

    // Clean up temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    if audio.is_empty() {
        return Err("edge-tts produced empty audio".into());
    }

    info!(
        "[edge-tts] synthesized {} bytes (voice={}, text_len={})",
        audio.len(),
        voice_id,
        text.len()
    );

    Ok(audio)
}
