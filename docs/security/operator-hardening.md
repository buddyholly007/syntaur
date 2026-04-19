# Operator hardening checklist

For admins running Syntaur in production. Run through this after first install + after every major version bump. Most items auto-apply — this list exists so you can verify nothing regressed.

## Before the first user logs in

- [ ] **Gateway bound behind Tailscale.** Don't expose the HTTP port to the internet. Connect your tailnet at `/setup/tailscale` and give out the `https://<host>.<tailnet>.ts.net` URL instead of the LAN IP.
- [ ] **Admin password set.** Set it via the `/setup` wizard. Pick something you'll remember — Syntaur doesn't force complexity and doesn't auto-rotate. Store it in your password manager.
- [ ] **Backup strategy in place.** The entire Syntaur state lives in `<data-dir>` (default `~/.syntaur/` on host installs; `/mnt/cherry_family_nas/syntaur/data` on the TrueNAS app). Snapshot the directory. Back up off-box too.

## Every week

- [ ] **Glance at `/api/audit`.** Look for `auth.login.fail` bursts, `admin.*` actions you didn't perform, `tailscale.*` events you didn't trigger. Anything surprising, investigate.
- [ ] **Check the integrations page** at `/settings/connect`. A broken integration is a service you're silently not getting. A newly-connected integration you didn't add is a red flag.
- [ ] **Run the isolation harness** against your live gateway:

  ```bash
  SYNTAUR_URL=https://<your-tailnet>.ts.net \
  SYNTAUR_ADMIN_TOKEN=<admin-session-token> \
  syntaur-isolation-tests
  ```

  All probes should pass in under a minute. Any failure means user boundary enforcement regressed — stop and investigate before continuing to use the gateway for shared data.

## After every upgrade

- [ ] **Re-run the isolation harness.** (Above.)
- [ ] **Confirm the audit-log endpoint still returns.** `GET /api/audit?limit=1` with your admin token should return a recent row.
- [ ] **Hit `/api/setup/tailscale/status`** to confirm the tailnet sidecar is still connected + the cert is valid. A `connected: false` means the sidecar fell off the tailnet — check `docker logs syntaur-tailscale`.

## On a compromise suspicion

1. **Revoke all sessions.** In the admin UI (or via `POST /api/admin/users/{id}/tokens` with `revoked_at`), burn every active session. Users re-login on their next action.
2. **Rotate the gateway password.** `/settings/account/password` — the new password takes effect immediately; the running session stays live until its token expires.
3. **Inspect the audit log.** `GET /api/audit?user_id=<id>&limit=500` — look for the first entry that doesn't match expected activity.
4. **Rotate vault secrets.** For every integration credential in `vault list`, regenerate it at the upstream (OpenRouter, Cerebras, Groq, Telegram bot tokens, Gmail OAuth, Tailscale OAuth) and import the new value with `vault set`. Syntaur picks up the new value on next restart.
5. **If `master.key` is suspected exposed**, generate a new key + re-encrypt: `vault rotate`. This re-encrypts every entry with a fresh key.

## Environment expectations

- **`<data-dir>` ownership.** Must be owned by the process user + 0700 on the directory, 0600 on `master.key` and `vault.json`. Gateway refuses to start otherwise.
- **`bubblewrap` installed.** Required for MCP process sandboxing (Phase 4.6). `apt-get install bubblewrap` on Debian/Ubuntu bases; Arch-based containers have it by default. Gateway falls back to unsandboxed MCP spawns with a loud warning if not found.
- **`/dev/net/tun` accessible in the Tailscale sidecar container.** Required by the sidecar's compose definition (`devices: - /dev/net/tun:/dev/net/tun`). TrueNAS Electric Eel allows this by default.

## Never do

- Don't skip the `/setup/tailscale` step and just expose `http://<lan-ip>:18789` to the internet. There's no TLS on that port; bearer tokens would fly in plaintext from a phone on coffee-shop wifi.
- Don't run the gateway as root. The container's default user is `syntaur`.
- Don't commit `vault.json` or `master.key` to git. A `.gitignore` catches this but double-check on any manual backup.
- Don't set `sharing_mode = shared` unless you actually want every user on this gateway to see every other user's data. That mode is for single-household installs where all accounts are family members who trust each other.
