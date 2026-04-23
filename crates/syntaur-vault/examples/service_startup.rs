//! Example: how a Syntaur service reads its secrets from the vault at
//! startup. Meant to be referenced (not built as part of the workspace
//! binary graph) when wiring `syntaur-gateway` + `rust-social-manager`
//! + trading bots to read from the shared vault.
//!
//! The pattern:
//!
//!   1. Service process starts.
//!   2. It connects to the per-host vault agent via the default Unix
//!      socket.
//!   3. If the agent isn't running or the vault is locked, service
//!      exits WITH A CLEAR MESSAGE telling the operator exactly what
//!      to run. No silent fallback to plaintext — refusing to start
//!      is the safe default.
//!   4. For each named secret the service needs, call `get_secret`.
//!   5. Service constructs its runtime config + proceeds.
//!
//! Production considerations (not shown here; see the project doc
//! `projects/syntaur_vault.md` for the phase-2 cutover plan):
//!
//!   - The agent needs to be unlocked BEFORE the service starts. For
//!     systemd services, a common pattern is a one-shot
//!     `syntaur-vault-unlock.service` that prompts the operator at
//!     boot and caches the passphrase via the OS keyring. The main
//!     service's `[Unit] After=` and `Requires=` that unit.
//!   - For headless / CI: use systemd credentials
//!     (`LoadCredentialEncrypted=`) to feed the passphrase in, OR
//!     `SYNTAUR_VAULT_PASSPHRASE_FILE=` pointing at a root-owned file.
//!   - Never pass the passphrase through a CLI arg — it ends up in
//!     `ps` output. Always file, env, or stdin.

use anyhow::{anyhow, Context, Result};
use syntaur_vault_core::{
    agent::{self, AgentRequest, AgentResponse},
    default_socket_path,
};

fn main() -> Result<()> {
    let socket = default_socket_path();

    // Secrets this service needs. In real code this list lives in the
    // service's config schema; the vault doesn't care how you pick
    // names.
    let openrouter_key = get_secret(&socket, "openrouter")?;
    let telegram_token = get_secret(&socket, "telegram.claude_bot")?;

    eprintln!("[service-example] loaded {} secrets from vault", 2);
    eprintln!("[service-example] openrouter: {} chars", openrouter_key.len());
    eprintln!("[service-example] telegram:   {} chars", telegram_token.len());

    // ...proceed to build the service config + start serving...

    Ok(())
}

/// Fetch one secret by name. Production-quality error messages:
/// telling the operator exactly what to do is more important than
/// hiding the fact that the vault is locked.
fn get_secret(socket: &std::path::Path, name: &str) -> Result<String> {
    if !socket.exists() {
        return Err(anyhow!(
            "vault agent not running at {} — run `syntaur-vault unlock` first",
            socket.display()
        ));
    }

    let resp = agent::request(
        socket,
        &AgentRequest::Get {
            name: name.to_string(),
        },
    )
    .with_context(|| format!("asking vault agent for {name}"))?;

    match resp {
        AgentResponse::Value { value } => Ok(value),
        AgentResponse::Error { message } => Err(anyhow!(
            "vault refused `{name}`: {message} — check `syntaur-vault list` or `syntaur-vault status`"
        )),
        other => Err(anyhow!("unexpected response for {name}: {other:?}")),
    }
}
