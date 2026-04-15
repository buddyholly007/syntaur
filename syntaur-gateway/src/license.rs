//! Licensing for Syntaur — Free / Pro tier split.
//!
//! - **Free tier**: Core chat, web search, file management, shell — unlimited, forever
//! - **Pro tier**: Voice, smart home, social media, finance, browser automation
//! - A license key (Ed25519-signed JSON) unlocks Pro features
//! - License verification is fully offline — no phone-home
//! - No demo timer, no conversation limits on free tier

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Ed25519 public key from the Syntaur license server.
/// Used for offline license verification — no phone-home required.
const LICENSE_PUBLIC_KEY_HEX: &str = "5397b444a47e84ef88c932beb0c4adf50c055d7988056c7267cf99510f7a4cd4";

/// Modules included in the free tier (always available).
pub const FREE_MODULES: &[&str] = &[
    "core-files", "core-shell", "core-web", "core-telegram",
];

/// Modules that require Pro license.
pub const PRO_MODULES: &[&str] = &[
    "mod-comms", "mod-captcha", "mod-office", "mod-accounts", "mod-browser",
    "social-manager", "office", "filesystem", "search",
    // Future premium: home, camera, finance
    "mod-voice-journal",
    "mod-coders",
];

/// License status for the current installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseStatus {
    pub mode: LicenseMode,
    pub license_holder: Option<String>,
    pub license_tier: Option<String>,
    pub free_modules: Vec<String>,
    pub pro_modules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LicenseMode {
    /// Pro license active — all modules available.
    Pro,
    /// Free tier — core modules only, no time limit.
    Free,
}

impl std::fmt::Display for LicenseMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pro => write!(f, "Pro"),
            Self::Free => write!(f, "Free"),
        }
    }
}

/// Persisted license state on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LicenseState {
    license_key: Option<String>,
    #[serde(default)]
    installed_at: u64,
}

/// A signed license key (JSON blob signed with Ed25519).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseKey {
    /// Email of the license holder.
    pub email: String,
    /// License tier: "standard", "professional", "enterprise".
    pub tier: String,
    /// Expiry timestamp (0 = perpetual).
    pub expires_at: u64,
    /// Enabled premium modules (empty = all).
    pub modules: Vec<String>,
    /// Timestamp when the license was issued.
    #[serde(default)]
    pub issued_at: u64,
    /// Unique purchase identifier.
    #[serde(default)]
    pub purchase_id: String,
    /// Ed25519 signature of the above fields (hex-encoded).
    pub signature: String,
}

/// Check the current license status.
pub fn check_license(data_dir: &Path) -> LicenseStatus {
    let state = load_state(data_dir);
    let now = now_secs();

    let free = FREE_MODULES.iter().map(|s| s.to_string()).collect();
    let pro = PRO_MODULES.iter().map(|s| s.to_string()).collect();

    // Check for valid license key
    if let Some(key_str) = &state.license_key {
        if let Some(key) = parse_license_key(key_str) {
            if key.expires_at == 0 || key.expires_at > now {
                return LicenseStatus {
                    mode: LicenseMode::Pro,
                    license_holder: Some(key.email),
                    license_tier: Some(key.tier),
                    free_modules: free,
                    pro_modules: pro,
                };
            }
        }
    }

    // Free tier — no time limit, core modules always work
    LicenseStatus {
        mode: LicenseMode::Free,
        license_holder: None,
        license_tier: None,
        free_modules: free,
        pro_modules: pro,
    }
}

/// Check if a new conversation is allowed. Always true — free tier has no limits.
pub fn can_start_conversation(_data_dir: &Path) -> (bool, String) {
    (true, String::new())
}

/// Check if a specific module is available (free tier or Pro licensed).
pub fn is_module_available(data_dir: &Path, module_id: &str) -> bool {
    if FREE_MODULES.contains(&module_id) {
        return true; // Always available
    }
    let status = check_license(data_dir);
    status.mode == LicenseMode::Pro
}

/// Apply a license key. Returns Ok(email) on success.
pub fn apply_license_key(data_dir: &Path, key_str: &str) -> Result<String, String> {
    let key = parse_license_key(key_str)
        .ok_or_else(|| "Invalid license key format".to_string())?;

    // Ed25519 signature verification — fail closed
    if let Err(e) = verify_signature(&key) {
        return Err(format!("License signature verification failed: {}", e));
    }

    let now = now_secs();
    if key.expires_at != 0 && key.expires_at < now {
        return Err("License key has expired".to_string());
    }

    let mut state = load_state(data_dir);
    state.license_key = Some(key_str.to_string());
    save_state(data_dir, &state);

    log::info!("[license] Activated: {} (tier: {})", key.email, key.tier);
    Ok(key.email)
}

/// Verify the Ed25519 signature on a license key.
fn verify_signature(key: &LicenseKey) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let pub_bytes = hex::decode(LICENSE_PUBLIC_KEY_HEX)
        .map_err(|_| "Invalid embedded public key".to_string())?;

    let pub_key_bytes: [u8; 32] = pub_bytes.try_into()
        .map_err(|_| "Public key wrong length".to_string())?;

    let verifying_key = VerifyingKey::from_bytes(&pub_key_bytes)
        .map_err(|_| "Invalid public key".to_string())?;

    // Reconstruct the payload that was signed (without the signature field)
    let payload = serde_json::json!({
        "email": key.email,
        "tier": key.tier,
        "expires_at": key.expires_at,
        "modules": key.modules,
        "issued_at": key.issued_at,
        "purchase_id": key.purchase_id,
    });
    let payload_str = serde_json::to_string(&payload)
        .map_err(|_| "Serialization error".to_string())?;

    let sig_bytes = hex::decode(&key.signature)
        .map_err(|_| "Invalid signature encoding".to_string())?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|_| "Invalid signature".to_string())?;

    verifying_key.verify_strict(payload_str.as_bytes(), &signature)
        .map_err(|_| "Signature verification failed".to_string())
}

// ── Internal helpers ────────────────────────────────────────────────────

fn load_state(data_dir: &Path) -> LicenseState {
    let path = state_path(data_dir);
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(state) = serde_json::from_str(&content) {
            return state;
        }
    }
    // First run
    let state = LicenseState {
        license_key: None,
        installed_at: now_secs(),
    };
    save_state(data_dir, &state);
    state
}

fn save_state(data_dir: &Path, state: &LicenseState) {
    let path = state_path(data_dir);
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(&path, json);
    }
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("license.json")
}

fn parse_license_key(s: &str) -> Option<LicenseKey> {
    // Try JSON directly
    if let Ok(key) = serde_json::from_str::<LicenseKey>(s) {
        return Some(key);
    }
    // Try base64-encoded JSON
    if let Ok(decoded) = base64_decode(s) {
        if let Ok(key) = serde_json::from_str::<LicenseKey>(&decoded) {
            return Some(key);
        }
    }
    None
}

fn base64_decode(s: &str) -> Result<String, ()> {
    // Simple base64 decode without pulling in a crate
    let s = s.trim();
    let bytes: Vec<u8> = s.bytes().collect();
    // Use the standard library's approach
    if let Ok(decoded) = String::from_utf8(
        bytes.iter().copied()
            .filter(|b| *b != b'\n' && *b != b'\r')
            .collect()
    ) {
        // Just return the raw string if it looks like JSON
        if decoded.starts_with('{') {
            return Ok(decoded);
        }
    }
    Err(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}


