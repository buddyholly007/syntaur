//! Tailscale Serve integration — Phase 4.1 TLS via Tailscale's Let's Encrypt
//! certs. Syntaur itself keeps its plain-HTTP listener on 18789; a sidecar
//! `tailscale` container in the compose reverse-proxies from the tailnet to
//! `host.docker.internal:18789`, giving every tailnet node a trusted HTTPS
//! URL (`https://<hostname>.<tailnet>.ts.net`) with zero cert management in
//! the gateway.
//!
//! ## Setup UX
//!
//! Two paths, both surface in the setup wizard at `/setup/remote-access`:
//!
//!   1. **Paste auth key** — user goes to Tailscale admin, clicks "Generate
//!      auth key", pastes it. Syntaur writes it to the sidecar's key file
//!      (0600); the sidecar's polling entrypoint notices and runs
//!      `tailscale up`. One-shot; the key expires per its TTL (max 90 days).
//!
//!   2. **Paste OAuth credentials** — user creates an OAuth client in their
//!      Tailscale admin with the `auth_keys` scope, pastes CLIENT_ID +
//!      CLIENT_SECRET. Syntaur stores them encrypted in the vault and uses
//!      the `client_credentials` grant to mint fresh auth keys on demand +
//!      auto-rotate before expiry. Permanent.
//!
//! Both land at the same runtime state: a key file the sidecar polls.
//! Option 2 lets us rotate autonomously; option 1 requires the user to come
//! back every 90 days. UX is otherwise identical.
//!
//! ## Sidecar contract
//!
//! The `tailscale` service in docker-compose-prod.yml expects:
//!   - `/state/tailscaled.sock` (volume: tailscale-state)
//!   - `/config/authkey` (the current auth key, may be empty)
//!   - `/config/serve.json` (TS_SERVE_CONFIG)
//!
//! The entrypoint polls `/config/authkey`; when non-empty and the node isn't
//! already logged in, it runs `tailscale up --authkey-file=/config/authkey`.
//! Key rotation is: Syntaur mints a new key, writes it to the file; within 5s
//! the sidecar logs out + re-logs-in with the new key, preserving the node
//! identity via the persistent state volume.

use std::path::Path;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

use crate::AppState;

/// Where the sidecar looks for the current auth key. Must line up with the
/// compose volume mount. Inside the gateway container the path is the same
/// thanks to a matching bind-mount.
const AUTHKEY_FILE: &str = "/config/tailscale/authkey";

/// Sidecar state directory. Presence of `tailscaled.state` inside means the
/// node has registered at least once — surface that as "connected" even if
/// the subprocess is temporarily restarting.
const STATE_DIR: &str = "/state/tailscale";

/// Serve.json that the sidecar's `tailscaled` reads at startup. Committed to
/// disk by the connect handler so the sidecar has a valid serve config from
/// the moment it comes up.
const SERVE_JSON: &str = "/config/tailscale/serve.json";

/// Vault key where OAuth credentials are stored.
const OAUTH_VAULT_CLIENT_ID: &str = "TAILSCALE_OAUTH_CLIENT_ID";
const OAUTH_VAULT_CLIENT_SECRET: &str = "TAILSCALE_OAUTH_CLIENT_SECRET";

// ── Request / response shapes ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConnectByAuthKeyRequest {
    pub token: String,
    /// Raw Tailscale auth key (`tskey-auth-...`).
    pub auth_key: String,
    /// Optional MagicDNS hostname. Defaults to `syntaur`.
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Deserialize)]
pub struct ConnectByOAuthRequest {
    pub token: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub enabled: bool,
    pub connected: bool,
    pub hostname: Option<String>,
    pub tailnet_url: Option<String>,
    pub auth_mode: &'static str,
    /// Human-readable explanation of why the sidecar isn't serving yet.
    /// Presence of `action_url` means this is a one-click-to-fix error.
    pub last_error: Option<String>,
    /// If the error is a one-click remediation (e.g. Tailscale's per-node
    /// "enable Serve" URL), the clickable URL the UI should render. `None`
    /// means the error is informational only.
    pub action_url: Option<String>,
    /// Machine-readable error kind for the UI to branch on.
    /// `"serve_not_enabled"` is the only one currently defined.
    pub error_kind: Option<String>,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// GET /api/setup/tailscale/status — used by the setup wizard + Settings
/// Connect card to render the current state.
pub async fn handle_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<StatusResponse>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal_scoped(&state, token, "admin").await?;
    Ok(Json(current_status(&state).await))
}

/// POST /api/setup/tailscale/connect — auth-key path.
pub async fn handle_connect_authkey(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectByAuthKeyRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Admin-only: remote-access config is operator territory, not per-user.
    let _principal = crate::resolve_principal_scoped(&state, &req.token, "admin").await?;
    if req.auth_key.trim().is_empty() || !req.auth_key.starts_with("tskey-") {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": "That doesn't look like a Tailscale auth key — they start with 'tskey-'. Generate one at https://login.tailscale.com/admin/settings/keys"
        })));
    }
    let hostname = sanitize_hostname(req.hostname.as_deref().unwrap_or("syntaur"));
    match write_key_and_serve(&req.auth_key, &hostname).await {
        Ok(()) => {
            crate::security::audit_log(
                &state,
                None,
                "tailscale.connect.authkey",
                None,
                serde_json::json!({"hostname": hostname}),
                None,
                None,
            ).await;
            Ok(Json(serde_json::json!({
                "ok": true,
                "hostname": hostname,
                "note": "Sidecar picks up the new key within 5 seconds. Watch the status endpoint.",
            })))
        }
        Err(e) => Ok(Json(serde_json::json!({"ok": false, "error": e}))),
    }
}

/// POST /api/setup/tailscale/connect_oauth — OAuth credentials path.
///
/// 1. Verifies the credentials by requesting a `client_credentials` grant.
/// 2. Stores them encrypted in the vault.
/// 3. Mints a reusable auth key via the Tailscale API.
/// 4. Writes the key to the sidecar's file.
///
/// Future auto-rotation: a background task re-mints every 30 days using the
/// stored credentials. Key expiry caps the blast radius if the vault is
/// ever compromised.
pub async fn handle_connect_oauth(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectByOAuthRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _principal = crate::resolve_principal_scoped(&state, &req.token, "admin").await?;
    if req.client_id.trim().is_empty() || req.client_secret.trim().is_empty() {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": "Both Client ID and Client Secret are required. Get them at https://login.tailscale.com/admin/settings/oauth"
        })));
    }
    let hostname = sanitize_hostname(req.hostname.as_deref().unwrap_or("syntaur"));

    // Try the client_credentials exchange + key mint before we persist
    // anything. If it fails, surface the error directly to the user.
    let mint = match mint_key_via_oauth(&req.client_id, &req.client_secret, &hostname).await {
        Ok(key) => key,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "ok": false,
                "error": format!("Tailscale rejected those credentials: {e}. Open https://login.tailscale.com/admin/settings/oauth, delete any existing Syntaur client, and create a new one with BOTH scopes checked: `auth_keys` (Write) and `acl` (Write). Paste the new Client ID + Secret here."),
            })));
        }
    };

    // Persist credentials to the vault for future rotations. Vault is
    // AES-256-GCM encrypted at rest per Phase 3.1.
    if let Err(e) = vault_set(&state, OAUTH_VAULT_CLIENT_ID, &req.client_id).await {
        return Ok(Json(serde_json::json!({"ok": false, "error": format!("Couldn't save credentials: {e}")})));
    }
    if let Err(e) = vault_set(&state, OAUTH_VAULT_CLIENT_SECRET, &req.client_secret).await {
        return Ok(Json(serde_json::json!({"ok": false, "error": format!("Couldn't save credentials: {e}")})));
    }

    if let Err(e) = write_key_and_serve(&mint, &hostname).await {
        return Ok(Json(serde_json::json!({"ok": false, "error": e})));
    }

    crate::security::audit_log(
        &state,
        None,
        "tailscale.connect.oauth",
        None,
        serde_json::json!({"hostname": hostname, "rotation": "auto"}),
        None,
        None,
    ).await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "hostname": hostname,
        "rotation": "auto",
        "note": "Credentials saved. Syntaur will rotate the auth key every 30 days automatically."
    })))
}

/// POST /api/tailscale/disconnect — clears the auth key + OAuth creds + state
/// dir. Next sidecar poll discovers an empty key file and stays logged out.
pub async fn handle_disconnect(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let _principal = crate::resolve_principal_scoped(&state, token, "admin").await?;

    let _ = tokio::fs::write(AUTHKEY_FILE, b"").await;
    let _ = vault_delete(&state, OAUTH_VAULT_CLIENT_ID).await;
    let _ = vault_delete(&state, OAUTH_VAULT_CLIENT_SECRET).await;

    crate::security::audit_log(
        &state,
        None,
        "tailscale.disconnect",
        None,
        serde_json::json!({}),
        None,
        None,
    ).await;
    Ok(Json(serde_json::json!({"ok": true})))
}

// ── Core helpers ────────────────────────────────────────────────────────

async fn current_status(state: &Arc<AppState>) -> StatusResponse {
    let state_present = Path::new(STATE_DIR).join("tailscaled.state").exists();
    let key_present = matches!(tokio::fs::read_to_string(AUTHKEY_FILE).await, Ok(s) if !s.trim().is_empty());
    let enabled = state_present || key_present;

    // Prefer talking to the sidecar's local tailscaled socket if reachable.
    // If we can't reach it, fall back to "enabled but status unknown".
    let (connected, hostname, url) = match query_sidecar_status().await {
        Ok(s) => (s.connected, s.hostname, s.url),
        Err(_) => (state_present, None, None),
    };

    let auth_mode = if vault_has(state, OAUTH_VAULT_CLIENT_ID).await {
        "oauth"
    } else if key_present {
        "authkey"
    } else {
        "none"
    };

    // The sidecar writes /state/tailscale/sidecar-error.txt when it can't
    // apply the serve config — most commonly because the tailnet owner
    // hasn't enabled Tailscale Serve yet. The file is `kind\turl\n`.
    // Surface the URL to the wizard UI so the user can click it directly
    // rather than hunting through Tailscale's admin console.
    let (last_error, action_url, error_kind) =
        match tokio::fs::read_to_string("/state/tailscale/sidecar-error.txt").await {
            Ok(s) => {
                let line = s.trim();
                let mut parts = line.splitn(2, '\t');
                let kind = parts.next().unwrap_or("").to_string();
                let action = parts.next().map(|s| s.to_string());
                let msg = match kind.as_str() {
                    "serve_not_enabled" | "serve-not-enabled" => Some(
                        "Tailscale Serve isn't enabled on your tailnet yet. One click in your Tailscale admin console finishes the setup — we've pre-filled the exact URL.".to_string(),
                    ),
                    _ if !kind.is_empty() => Some(format!("Sidecar error: {kind}")),
                    _ => None,
                };
                (msg, action, Some(kind).filter(|s| !s.is_empty()))
            }
            Err(_) => (None, None, None),
        };

    StatusResponse {
        enabled,
        connected,
        hostname,
        tailnet_url: url,
        auth_mode,
        last_error,
        action_url,
        error_kind,
    }
}

struct SidecarStatus {
    connected: bool,
    hostname: Option<String>,
    url: Option<String>,
}

/// Query the sidecar's local tailscaled over its LocalAPI socket. The
/// socket is bind-mounted from the sidecar's state volume into the gateway
/// container at `/state/tailscale/tailscaled.sock` (read-only).
async fn query_sidecar_status() -> Result<SidecarStatus, String> {
    let sock = "/state/tailscale/tailscaled.sock";
    if !Path::new(sock).exists() {
        return Ok(SidecarStatus {
            connected: false,
            hostname: None,
            url: None,
        });
    }
    // Unix-socket HTTP client — one-shot, short timeout.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    // The LocalAPI exposes /localapi/v0/status over HTTP-on-Unix-socket.
    // reqwest can't hit Unix sockets directly; we shell out instead.
    let out = tokio::process::Command::new("curl")
        .args([
            "-s", "--max-time", "2",
            "--unix-socket", sock,
            "http://local-tailscaled.sock/localapi/v0/status",
        ])
        .output()
        .await
        .map_err(|e| format!("localapi: {e}"))?;
    if !out.status.success() {
        return Err(format!("localapi status {}", out.status));
    }
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let self_node = &v["Self"];
    let dns = self_node["DNSName"].as_str().map(|s| s.trim_end_matches('.').to_string());
    let online = self_node["Online"].as_bool().unwrap_or(false);
    let hostname = dns.as_ref().and_then(|d| d.split('.').next()).map(|s| s.to_string());
    let url = dns.clone().map(|d| format!("https://{d}"));
    let _ = client; // suppress unused
    Ok(SidecarStatus {
        connected: online,
        hostname,
        url,
    })
}

/// Ensure the tailnet's ACL has `tag:syntaur` declared in `tagOwners`.
///
/// Tailnet-owned auth keys (the only kind OAuth clients can mint) must
/// carry a tag, and the tag must be declared. This function reads the
/// current ACL, idempotently adds the tag line, and writes it back if a
/// change was needed.
///
/// Idempotent: a second call is a no-op if the tag is already present.
/// Parse path is hujson-tolerant — the Tailscale ACL is JSON-with-comments,
/// and we preserve comments/formatting as much as we can by doing a
/// targeted text insertion rather than a full re-serialize.
async fn ensure_syntaur_tag_owner(access_token: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    // Fetch the current ACL as hujson (preserves comments).
    let resp = client
        .get("https://api.tailscale.com/api/v2/tailnet/-/acl")
        .bearer_auth(access_token)
        .header("Accept", "application/hujson")
        .send()
        .await
        .map_err(|e| format!("ACL fetch: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        if body.contains("scope") || body.contains("permission") {
            return Err(format!(
                "Tailscale rejected the ACL read — your OAuth client is missing the `acl` scope. Delete it at https://login.tailscale.com/admin/settings/oauth and re-create it with both `auth_keys` and `acl` scopes checked. Original error: {body}"
            ));
        }
        return Err(format!("ACL fetch returned non-2xx: {body}"));
    }
    let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let current = resp.text().await.map_err(|e| format!("ACL read body: {e}"))?;

    // Already present? No-op.
    if current.contains("\"tag:syntaur\"") {
        return Ok(());
    }

    // Insert `"tag:syntaur": ["autogroup:admin"],` into tagOwners.
    // Three cases the default/common ACLs fall into:
    //   1. `tagOwners` block exists uncommented: add one line inside.
    //   2. `tagOwners` block exists but fully commented out: uncomment and
    //      swap in our entry.
    //   3. No `tagOwners` block at all: insert a fresh block at the top
    //      level after the opening `{`.
    let updated = if let Some(pos) = current.find("\"tagOwners\":") {
        // Find the opening `{` after the key.
        let after_key = &current[pos..];
        if let Some(brace_rel) = after_key.find('{') {
            let insert_at = pos + brace_rel + 1;
            let mut out = String::with_capacity(current.len() + 64);
            out.push_str(&current[..insert_at]);
            out.push_str("\n\t\t\"tag:syntaur\": [\"autogroup:admin\"],");
            out.push_str(&current[insert_at..]);
            out
        } else {
            return Err("malformed tagOwners: no `{` after key".to_string());
        }
    } else if let Some(pos) = current.find("// \"tagOwners\":") {
        // Uncomment the line + surrounding block.
        let prefix = &current[..pos];
        let rest = &current[pos..];
        let end_of_block = rest.find("// },").map(|i| i + "// },".len());
        match end_of_block {
            Some(end) => {
                let mut out = String::with_capacity(current.len() + 64);
                out.push_str(prefix);
                out.push_str("\"tagOwners\": {\n\t\t\"tag:syntaur\": [\"autogroup:admin\"],\n\t},");
                out.push_str(&rest[end..]);
                out
            }
            None => {
                // Fallback: just inject a fresh block at the top.
                inject_tag_owners_at_top(&current)
            }
        }
    } else {
        inject_tag_owners_at_top(&current)
    };

    // Push the update back.
    let mut req = client
        .post("https://api.tailscale.com/api/v2/tailnet/-/acl")
        .bearer_auth(access_token)
        .header("Content-Type", "application/hujson")
        .body(updated);
    if let Some(tag) = etag {
        req = req.header("If-Match", tag);
    }
    let put = req.send().await.map_err(|e| format!("ACL push: {e}"))?;
    if !put.status().is_success() {
        let body = put.text().await.unwrap_or_default();
        return Err(format!("ACL push returned non-2xx: {body}"));
    }
    log::info!("[tailscale] added `tag:syntaur` to tagOwners");
    Ok(())
}

/// Fallback ACL-surgery path when neither an existing `tagOwners` nor a
/// commented example is present. Inserts a fresh `tagOwners` block on a
/// new line after the opening `{`.
fn inject_tag_owners_at_top(acl: &str) -> String {
    if let Some(pos) = acl.find('{') {
        let mut out = String::with_capacity(acl.len() + 80);
        out.push_str(&acl[..=pos]);
        out.push_str("\n\t\"tagOwners\": {\n\t\t\"tag:syntaur\": [\"autogroup:admin\"],\n\t},\n");
        out.push_str(&acl[pos + 1..]);
        out
    } else {
        // ACL without an opening brace? Unlikely — return unchanged so the
        // caller's idempotency check surfaces a real error from Tailscale.
        acl.to_string()
    }
}

/// Mint a reusable auth key via the Tailscale HTTP API using the
/// `client_credentials` OAuth grant.
///
/// Three HTTP calls: token exchange, tagOwners update (idempotent),
/// then key creation. Returns the raw auth key (prefixed `tskey-auth-...`)
/// on success.
async fn mint_key_via_oauth(
    client_id: &str,
    client_secret: &str,
    hostname: &str,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    // Step 1: client_credentials grant. Scope parameter is omitted — Tailscale
    // derives the token's effective scope from the OAuth client's server-side
    // registration, so asking for `auth_keys` explicitly when the client
    // wasn't created with that scope produces a confusing `cannot grant scopes`
    // error. If the client was registered without `auth_keys` write, the mint
    // call in step 2 surfaces a clear error that tells the user to fix it.
    let token_resp = client
        .post("https://api.tailscale.com/api/v2/oauth/token")
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(format!("token endpoint returned {body}"));
    }
    let token_json: serde_json::Value = token_resp.json().await.map_err(|e| e.to_string())?;
    let access_token = token_json["access_token"]
        .as_str()
        .ok_or_else(|| "no access_token in token response".to_string())?
        .to_string();

    // Step 2: make sure `tag:syntaur` is declared in the tailnet's ACL.
    // OAuth-minted keys are tailnet-owned and Tailscale requires them to
    // carry a declared tag. Syntaur manages this automatically so the user
    // never has to hand-edit an ACL.
    ensure_syntaur_tag_owner(&access_token).await?;

    // Step 3: mint the auth key with `tag:syntaur` so the node registers
    // pre-approved on first boot. 60-day TTL with 30-day auto-rotation
    // gives a healthy overlap window where either key can still register.
    let key_req = serde_json::json!({
        "capabilities": {
            "devices": {
                "create": {
                    "reusable": true,
                    "ephemeral": false,
                    "preauthorized": true,
                    "tags": ["tag:syntaur"],
                }
            }
        },
        "expirySeconds": 60 * 60 * 24 * 60, // 60 days (rotate every 30)
        "description": format!("syntaur {hostname} auto rotated"),
    });
    let key_resp = client
        .post("https://api.tailscale.com/api/v2/tailnet/-/keys")
        .bearer_auth(&access_token)
        .json(&key_req)
        .send()
        .await
        .map_err(|e| format!("key mint request failed: {e}"))?;
    if !key_resp.status().is_success() {
        let body = key_resp.text().await.unwrap_or_default();
        // Common failure: tag:syntaur not in the tailnet's ACL tagOwners.
        // Surface the exact guidance the user needs.
        if body.contains("tag:syntaur") || body.contains("tagOwners") {
            return Err(format!(
                "Tailscale rejected the key request because `tag:syntaur` isn't declared in your ACL. Add a `tagOwners` entry for it at https://login.tailscale.com/admin/acls — e.g. `\"tag:syntaur\": [\"autogroup:admin\"]`. Original error: {body}"
            ));
        }
        return Err(format!("key endpoint returned {body}"));
    }
    let key_json: serde_json::Value = key_resp.json().await.map_err(|e| e.to_string())?;
    key_json["key"]
        .as_str()
        .ok_or_else(|| "no key in response".to_string())
        .map(|s| s.to_string())
}

/// Write the auth key + serve config to the paths the sidecar polls. Paths
/// are 0600 + 0700 on their parent dir so the key never leaks to group/
/// world even on a misconfigured bind-mount.
async fn write_key_and_serve(auth_key: &str, hostname: &str) -> Result<(), String> {
    let key_path = Path::new(AUTHKEY_FILE);
    let serve_path = Path::new(SERVE_JSON);

    // Ensure parent directory exists with 0700 perms.
    if let Some(parent) = key_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    tokio::fs::write(key_path, auth_key.as_bytes())
        .await
        .map_err(|e| format!("write {}: {e}", key_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
    }

    // Serve config: terminate HTTPS on 443 on the tailnet IP, forward to the
    // TrueNAS host's port 18789 where Syntaur is listening. `host.docker.
    // internal` resolves to the host from inside the tailscale sidecar.
    let serve = serde_json::json!({
        "TCP": {
            "443": { "HTTPS": true }
        },
        "Web": {
            format!("{hostname}:443"): {
                "Handlers": {
                    "/": { "Proxy": "http://host.docker.internal:18789" }
                }
            }
        },
        "AllowFunnel": {}
    });
    tokio::fs::write(serve_path, serde_json::to_string_pretty(&serve).unwrap().as_bytes())
        .await
        .map_err(|e| format!("write {}: {e}", serve_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(serve_path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn sanitize_hostname(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(32).collect();
    if trimmed.is_empty() {
        "syntaur".to_string()
    } else {
        trimmed
    }
}

// ── Vault helpers (thin wrappers so handlers stay readable) ──────────────

async fn vault_set(state: &Arc<AppState>, name: &str, value: &str) -> Result<(), String> {
    let data_dir = crate::resolve_data_dir();
    let name_owned = name.to_string();
    let value_owned = value.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut v = crate::vault::Vault::open(&data_dir)
            .map_err(|e| format!("vault open: {e}"))?;
        v.set(&name_owned, &value_owned)
            .map_err(|e| format!("vault set: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    // Inform caller; reload not needed since the vault re-reads on next access.
    let _ = state;
    Ok(())
}

async fn vault_delete(state: &Arc<AppState>, name: &str) -> Result<(), String> {
    let data_dir = crate::resolve_data_dir();
    let name_owned = name.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut v = crate::vault::Vault::open(&data_dir)
            .map_err(|e| format!("vault open: {e}"))?;
        let _ = v.delete(&name_owned);
        Ok(())
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    let _ = state;
    Ok(())
}

async fn vault_has(state: &Arc<AppState>, name: &str) -> bool {
    let data_dir = crate::resolve_data_dir();
    let name_owned = name.to_string();
    let res: Result<bool, _> = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let v = crate::vault::Vault::open(&data_dir)
            .map_err(|e| format!("vault open: {e}"))?;
        Ok(v.list_keys().iter().any(|k| k == &name_owned))
    })
    .await
    .unwrap_or_else(|e| Err(format!("join: {e}")));
    let _ = state;
    res.unwrap_or(false)
}

// ── Auto-rotation background task ────────────────────────────────────────

/// Spawned once at startup. Every 12h, if OAuth credentials are in the
/// vault, mint a fresh auth key and overwrite the sidecar's key file.
/// The sidecar's polling loop re-registers within 5s, preserving node
/// identity via the persistent state volume.
pub fn spawn_rotation_task(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Wait a minute after startup so the sidecar has settled.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            if let Err(e) = maybe_rotate(&state).await {
                log::warn!("[tailscale] rotation check: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
        }
    });
}

/// Mint a Tailscale pre-auth key for a single device (typical use:
/// personalized per-invite installer credentials). Requires OAuth creds
/// already stored in the vault. Unlike the auto-rotation key, these are
/// single-use (`reusable: false`) and expire after 7 days so an
/// unredeemed invite lapses on its own.
///
/// Returns the raw auth key (prefixed `tskey-auth-...`); caller is
/// responsible for delivery (bake into installer command, email, etc.)
/// and must audit-log the mint event with whatever context it has.
pub async fn mint_invite_authkey(state: &Arc<AppState>, label: &str) -> Result<String, String> {
    let (client_id, client_secret) = read_oauth_creds(state).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token_resp = client
        .post("https://api.tailscale.com/api/v2/oauth/token")
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    if !token_resp.status().is_success() {
        return Err(format!("token endpoint: {}", token_resp.text().await.unwrap_or_default()));
    }
    let token_json: serde_json::Value = token_resp.json().await.map_err(|e| e.to_string())?;
    let access_token = token_json["access_token"]
        .as_str()
        .ok_or_else(|| "no access_token".to_string())?
        .to_string();

    ensure_syntaur_tag_owner(&access_token).await?;

    // Single-use, 7-day invite key. `reusable: false` means the key burns
    // on first successful registration — safe to paste into a command
    // the operator sends via an insecure channel.
    let key_req = serde_json::json!({
        "capabilities": {
            "devices": {
                "create": {
                    "reusable": false,
                    "ephemeral": false,
                    "preauthorized": true,
                    "tags": ["tag:syntaur"],
                }
            }
        },
        "expirySeconds": 60 * 60 * 24 * 7, // 7 days
        "description": format!("syntaur invite {label}"),
    });
    let key_resp = client
        .post("https://api.tailscale.com/api/v2/tailnet/-/keys")
        .bearer_auth(&access_token)
        .json(&key_req)
        .send()
        .await
        .map_err(|e| format!("key mint: {e}"))?;
    if !key_resp.status().is_success() {
        return Err(format!("key endpoint: {}", key_resp.text().await.unwrap_or_default()));
    }
    let key_json: serde_json::Value = key_resp.json().await.map_err(|e| e.to_string())?;
    let _ = state; // suppress unused warning
    key_json["key"].as_str().map(|s| s.to_string()).ok_or_else(|| "no key in response".to_string())
}

async fn maybe_rotate(state: &Arc<AppState>) -> Result<(), String> {
    if !vault_has(state, OAUTH_VAULT_CLIENT_ID).await {
        return Ok(());
    }
    // Check the current key's age by inspecting the file mtime. Rotate if
    // older than 30 days.
    let meta = match tokio::fs::metadata(AUTHKEY_FILE).await {
        Ok(m) => m,
        Err(_) => return Ok(()), // key file absent, nothing to rotate
    };
    let age = std::time::SystemTime::now()
        .duration_since(meta.modified().map_err(|e| e.to_string())?)
        .unwrap_or_default();
    if age.as_secs() < 30 * 24 * 3600 {
        return Ok(());
    }

    // Fetch creds from vault.
    let (client_id, client_secret) = read_oauth_creds(state).await?;
    let hostname = detect_hostname().await.unwrap_or_else(|| "syntaur".to_string());
    let new_key = mint_key_via_oauth(&client_id, &client_secret, &hostname).await?;
    let _ = &new_key;
    write_key_and_serve(&new_key, &hostname).await?;
    crate::security::audit_log(
        state,
        None,
        "tailscale.key.rotated",
        None,
        serde_json::json!({"hostname": hostname}),
        None,
        None,
    ).await;
    log::info!("[tailscale] rotated auth key (age was {}d)", age.as_secs() / 86400);
    Ok(())
}

async fn read_oauth_creds(state: &Arc<AppState>) -> Result<(String, String), String> {
    let data_dir = crate::resolve_data_dir();
    let res: Result<(String, String), String> = tokio::task::spawn_blocking(move || {
        let v = crate::vault::Vault::open(&data_dir)
            .map_err(|e| format!("vault open: {e}"))?;
        let cid = v
            .get(OAUTH_VAULT_CLIENT_ID)
            .map_err(|e| format!("vault get client_id: {e}"))?
            .ok_or_else(|| "client_id missing from vault".to_string())?;
        let cs = v
            .get(OAUTH_VAULT_CLIENT_SECRET)
            .map_err(|e| format!("vault get client_secret: {e}"))?
            .ok_or_else(|| "client_secret missing from vault".to_string())?;
        Ok((cid, cs))
    })
    .await
    .map_err(|e| format!("join: {e}"))?;
    let _ = state;
    res
}

async fn detect_hostname() -> Option<String> {
    // Parse the current serve.json if present — it carries the hostname we
    // last wrote, which matches what the sidecar advertised.
    let serve = tokio::fs::read_to_string(SERVE_JSON).await.ok()?;
    let v: serde_json::Value = serde_json::from_str(&serve).ok()?;
    let web = v.get("Web")?.as_object()?;
    let key = web.keys().next()?.clone();
    let host = key.split(':').next()?.to_string();
    Some(host)
}
