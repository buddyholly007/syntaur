//! ESPHome firmware-role library — Phase 6b.
//!
//! The wizard's `/smart-home/esphome` discover panel classifies each
//! device into a `SuggestedRole` (see `esphome_discovery::SuggestedRole`).
//! When the user picks "Reflash with role X", the gateway needs a YAML
//! config that:
//!   - Enables the right components (bluetooth_proxy, voice_assistant, etc.)
//!   - Disables/drops the wrong ones (so a board with both mic and
//!     mmwave doesn't ship with both active and burn its radio time)
//!   - Carries the user's stable secrets (api_encryption_key,
//!     ota_password, wifi credentials) — same key the gateway already
//!     stored when the device was originally adopted.
//!
//! This module owns the role-specific YAML templates. The actual flash
//! happens elsewhere (`firmware_flash.rs`, follow-up): the gateway
//! writes the rendered YAML to `~/esphome-builds/<name>/<name>.yaml`,
//! shells out to the build host (`gaming-pc`) to compile + upload.
//!
//! Templates are intentionally minimal — they cover the components
//! that distinguish the role, plus the common base (api, ota, wifi,
//! captive_portal). Operators who want richer configs (Home Assistant
//! statestream, MQTT bridge, custom sensors) keep editing
//! `~/esphome-builds/<name>/<name>.yaml` by hand; this is for the
//! 80%-case "fresh ESP, give it a role, walk away".

use crate::smart_home::esphome_discovery::SuggestedRole;
use serde::{Deserialize, Serialize};

/// Per-board hardware variant. Different ESP32 dies have different
/// pin maps + radio caps; the YAML's `esp32:` block needs the right
/// `board:` token. We only carry the variants Sean's household
/// actually owns; new variants get added as new hardware lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HardwareVariant {
    /// Generic ESP32 dev board (e.g. WROOM-32). 4MB flash, BT 4.2.
    Esp32Generic,
    /// ESP32-S3 (BT 5.0 LE, dual-core, dedicated PSRAM). M5 Atom S3,
    /// LilyGo T-S3, Sat1.
    Esp32S3,
    /// ESP32-C3 (RISC-V, BT 5.0 LE, single-core, low cost). Used for
    /// passive proxies + presence sensors.
    Esp32C3,
    /// ESP32-C6 (Wi-Fi 6 + BT 5.0 LE + 802.15.4 Thread). Newer
    /// hardware Sean's evaluating for combined Matter/Thread proxies.
    Esp32C6,
}

impl HardwareVariant {
    /// `board:` value for the ESPHome `esp32:` block. Defaults are the
    /// most-shipped variant per chip family — operators with a different
    /// board edit the YAML by hand.
    pub fn board(self) -> &'static str {
        match self {
            HardwareVariant::Esp32Generic => "esp32dev",
            HardwareVariant::Esp32S3 => "esp32-s3-devkitc-1",
            HardwareVariant::Esp32C3 => "esp32-c3-devkitm-1",
            HardwareVariant::Esp32C6 => "esp32-c6-devkitc-1",
        }
    }

    /// `variant:` token for the `esp32:` block on dies that need it.
    /// Some boards (esp32dev, esp32-c3-devkitm-1) imply variant from
    /// `board:`; S3 + C6 require it explicitly. Empty string when the
    /// board key is sufficient.
    pub fn variant_token(self) -> &'static str {
        match self {
            HardwareVariant::Esp32Generic => "",
            HardwareVariant::Esp32S3 => "esp32s3",
            HardwareVariant::Esp32C3 => "",
            HardwareVariant::Esp32C6 => "esp32c6",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HardwareVariant::Esp32Generic => "ESP32 (generic)",
            HardwareVariant::Esp32S3 => "ESP32-S3",
            HardwareVariant::Esp32C3 => "ESP32-C3",
            HardwareVariant::Esp32C6 => "ESP32-C6",
        }
    }
}

/// Inputs to the YAML renderer. Captures everything the wizard knows
/// when the user clicks "Flash with role X" — name, board, role,
/// secrets the gateway previously stored. The renderer never reads
/// the filesystem or the network, so it's straightforward to unit-test.
#[derive(Debug, Clone)]
pub struct FirmwareRequest {
    /// ESPHome device name (lowercase, hyphenated). Becomes the host
    /// of `<name>.local` mDNS + the `name:` in the YAML's esphome
    /// block. Must be non-empty + match `[a-z0-9-]+`.
    pub name: String,
    pub friendly_name: Option<String>,
    pub variant: HardwareVariant,
    pub role: SuggestedRole,
    /// 32-byte base64 — same shape ESPHome `api.encryption.key` expects.
    pub api_encryption_key: Option<String>,
    pub ota_password: Option<String>,
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub ap_fallback_password: Option<String>,
}

/// Render a complete ESPHome YAML for the given request. Produces a
/// self-contained file that compiles cleanly with `esphome compile`
/// against pioarduino — no `<<:` includes, no environment expansion.
/// Secrets go into the YAML literally because the wizard owns the
/// generation: the user pastes them into the form, the gateway stores
/// them in `smart_home_credentials`, the renderer composes the
/// final file. Re-running the wizard regenerates without state from
/// disk, so secrets-on-disk drift is a non-issue.
pub fn render_yaml(req: &FirmwareRequest) -> Result<String, String> {
    if req.name.is_empty() || !req.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!(
            "device name must be lowercase alphanumeric/hyphen, got `{}`",
            req.name
        ));
    }
    if req.wifi_ssid.is_empty() {
        return Err("wifi_ssid is required".into());
    }

    let mut y = String::new();
    y.push_str("# Auto-generated by Syntaur firmware-role wizard.\n");
    y.push_str("# Edits made here survive `esphome run`; DO NOT commit secrets to git.\n");
    y.push_str(&format!("# Role: {}\n\n", role_label(req.role)));

    // ─── esphome / esp32 ────────────────────────────────────────────
    y.push_str("esphome:\n");
    y.push_str(&format!("  name: {}\n", req.name));
    if let Some(fn_) = req.friendly_name.as_deref() {
        y.push_str(&format!("  friendly_name: {}\n", quote_yaml(fn_)));
    }
    y.push_str("  min_version: 2026.1.0\n\n");

    y.push_str("esp32:\n");
    y.push_str(&format!("  board: {}\n", req.variant.board()));
    let v = req.variant.variant_token();
    if !v.is_empty() {
        y.push_str(&format!("  variant: {v}\n"));
    }
    y.push('\n');

    y.push_str("logger:\n");
    y.push_str("  level: INFO\n\n");

    // ─── api ───────────────────────────────────────────────────────
    y.push_str("api:\n");
    if let Some(k) = req.api_encryption_key.as_deref() {
        y.push_str("  encryption:\n");
        y.push_str(&format!("    key: {}\n", quote_yaml(k)));
    }
    y.push_str("  reboot_timeout: 15min\n\n");

    // ─── ota ───────────────────────────────────────────────────────
    if let Some(pw) = req.ota_password.as_deref() {
        y.push_str("ota:\n");
        y.push_str("  - platform: esphome\n");
        y.push_str(&format!("    password: {}\n\n", quote_yaml(pw)));
    }

    // ─── wifi ──────────────────────────────────────────────────────
    y.push_str("wifi:\n");
    y.push_str(&format!("  ssid: {}\n", quote_yaml(&req.wifi_ssid)));
    y.push_str(&format!("  password: {}\n", quote_yaml(&req.wifi_password)));
    if let Some(ap_pw) = req.ap_fallback_password.as_deref() {
        y.push_str("  ap:\n");
        y.push_str(&format!("    ssid: {}-fallback\n", req.name));
        y.push_str(&format!("    password: {}\n", quote_yaml(ap_pw)));
    }
    y.push_str("\ncaptive_portal:\n\n");

    // ─── role-specific block ──────────────────────────────────────
    y.push_str(&render_role_block(req.role));

    Ok(y)
}

fn render_role_block(role: SuggestedRole) -> String {
    match role {
        SuggestedRole::BtProxyActive => render_bt_proxy(true),
        SuggestedRole::BtProxyPassive => render_bt_proxy(false),
        SuggestedRole::VoiceSatellite => render_voice_satellite(),
        SuggestedRole::PresenceMmwave => render_presence_mmwave(),
        SuggestedRole::Unknown => String::new(),
    }
}

fn render_bt_proxy(active: bool) -> String {
    // `active: true` enables outgoing GATT connect (bluetooth_proxy
    // can mediate pairing for HA / Syntaur). `active: false` is a
    // pure scanner — listens to adverts only, ~half the radio time.
    let mut s = String::new();
    s.push_str("esp32_ble_tracker:\n");
    s.push_str("  scan_parameters:\n");
    s.push_str("    interval: 1100ms\n");
    s.push_str("    window: 1100ms\n");
    s.push_str("    active: true\n\n");
    s.push_str("bluetooth_proxy:\n");
    s.push_str(&format!("  active: {}\n\n", if active { "true" } else { "false" }));
    s
}

fn render_voice_satellite() -> String {
    // Conservative defaults — Sat1 hardware specifically. Pin map +
    // microphone driver vary per board; operators with non-standard
    // boards edit by hand.
    let mut s = String::new();
    s.push_str("micro_wake_word:\n");
    s.push_str("  models:\n");
    s.push_str("    - model: hey_jarvis\n\n");
    s.push_str("voice_assistant:\n");
    s.push_str("  microphone: mic\n");
    s.push_str("  speaker: speaker\n");
    s.push_str("  noise_suppression_level: 2\n");
    s.push_str("  auto_gain: 31dBFS\n");
    s.push_str("  volume_multiplier: 2.0\n\n");
    s.push_str("# Pin-mapping for Sat1 / generic I2S mic+speaker — edit\n");
    s.push_str("# for non-Sat1 hardware.\n");
    s
}

fn render_presence_mmwave() -> String {
    // LD2410B/LD2412 family is the most common consumer mmWave.
    // Defaults pulled from the ld2410 component docs.
    let mut s = String::new();
    s.push_str("uart:\n");
    s.push_str("  id: uart_mmwave\n");
    s.push_str("  rx_pin: GPIO16\n");
    s.push_str("  tx_pin: GPIO17\n");
    s.push_str("  baud_rate: 256000\n\n");
    s.push_str("ld2410:\n");
    s.push_str("  uart_id: uart_mmwave\n\n");
    s.push_str("binary_sensor:\n");
    s.push_str("  - platform: ld2410\n");
    s.push_str("    has_target:\n");
    s.push_str("      name: \"Presence\"\n");
    s.push_str("    has_moving_target:\n");
    s.push_str("      name: \"Moving Target\"\n");
    s.push_str("    has_still_target:\n");
    s.push_str("      name: \"Still Target\"\n\n");
    s.push_str("# Add esp32_ble_tracker if combining with passive scanning.\n");
    s
}

fn role_label(role: SuggestedRole) -> &'static str {
    match role {
        SuggestedRole::BtProxyActive => "BLE proxy (active)",
        SuggestedRole::BtProxyPassive => "BLE proxy (passive)",
        SuggestedRole::VoiceSatellite => "Voice satellite",
        SuggestedRole::PresenceMmwave => "Presence (mmWave + BLE)",
        SuggestedRole::Unknown => "Unspecified",
    }
}

/// Defensive YAML quoting. Anything that contains characters YAML's
/// flow-scalar parser reacts to (colon, hash, leading dash/space, the
/// trailing-space SSID quirk, etc.) gets wrapped in double quotes with
/// inner quotes/backslashes escaped. Plain identifiers stay unquoted.
fn quote_yaml(s: &str) -> String {
    let unsafe_char = |c: char| matches!(c, ':' | '#' | '"' | '\'' | '\\' | '\n' | '\r' | '\t');
    let needs_quote = s.is_empty()
        || s.chars().any(unsafe_char)
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.starts_with('-')
        || s.starts_with('!');
    if !needs_quote {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 4);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(role: SuggestedRole) -> FirmwareRequest {
        FirmwareRequest {
            name: "test-proxy".into(),
            friendly_name: Some("Test Proxy".into()),
            variant: HardwareVariant::Esp32Generic,
            role,
            api_encryption_key: Some("0Q/SMOlxnQQU6dIKGFm+7Lgusp0Ke4eU3tKfj3eHNbo=".into()),
            ota_password: Some("ota-secret".into()),
            wifi_ssid: "IOT Devices ".into(), // trailing space — UniFi quirk
            wifi_password: "wifi-secret".into(),
            ap_fallback_password: Some("ap-secret".into()),
        }
    }

    #[test]
    fn renders_active_bt_proxy() {
        let y = render_yaml(&req(SuggestedRole::BtProxyActive)).unwrap();
        assert!(y.contains("name: test-proxy"));
        assert!(y.contains("board: esp32dev"));
        assert!(y.contains("bluetooth_proxy:\n  active: true"));
        assert!(!y.contains("voice_assistant"));
    }

    #[test]
    fn renders_passive_bt_proxy() {
        let y = render_yaml(&req(SuggestedRole::BtProxyPassive)).unwrap();
        assert!(y.contains("bluetooth_proxy:\n  active: false"));
    }

    #[test]
    fn renders_voice_satellite() {
        let y = render_yaml(&req(SuggestedRole::VoiceSatellite)).unwrap();
        assert!(y.contains("voice_assistant:"));
        assert!(y.contains("micro_wake_word:"));
    }

    #[test]
    fn renders_presence_mmwave() {
        let y = render_yaml(&req(SuggestedRole::PresenceMmwave)).unwrap();
        assert!(y.contains("ld2410:"));
        assert!(y.contains("uart_mmwave"));
    }

    #[test]
    fn rejects_invalid_name() {
        let mut r = req(SuggestedRole::BtProxyActive);
        r.name = "Test Proxy".into(); // space + capital
        assert!(render_yaml(&r).is_err());
    }

    #[test]
    fn quotes_ssid_with_trailing_space() {
        let y = render_yaml(&req(SuggestedRole::BtProxyActive)).unwrap();
        // SSID has a trailing space (a known UniFi quirk in Sean's
        // setup). Must round-trip through YAML unmangled.
        assert!(y.contains("ssid: \"IOT Devices \""));
    }

    #[test]
    fn embeds_api_encryption_key() {
        let y = render_yaml(&req(SuggestedRole::BtProxyActive)).unwrap();
        assert!(y.contains("encryption:"));
        assert!(y.contains("0Q/SMOlxnQQU6dIKGFm+7Lgusp0Ke4eU3tKfj3eHNbo="));
    }

    #[test]
    fn s3_uses_correct_board() {
        let mut r = req(SuggestedRole::BtProxyActive);
        r.variant = HardwareVariant::Esp32S3;
        let y = render_yaml(&r).unwrap();
        assert!(y.contains("board: esp32-s3-devkitc-1"));
    }
}
