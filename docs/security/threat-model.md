# Syntaur threat model

Last updated 2026-04-19. Applies to v0.3.x.

## Assets (what an attacker wants)

| Asset | Where it lives | Blast radius if lost |
|---|---|---|
| `master.key` | `<data-dir>/master.key`, 0600 | Full vault decrypt → every integration credential |
| Vault contents | `<data-dir>/vault.json`, AES-256-GCM | OpenRouter / Cerebras / Groq / Telegram / Gmail / OAuth / Tailscale credentials |
| Session tokens | `user_api_tokens` table, hashed | Per-user account takeover for up to 48h (rotatable via `/api/auth/refresh`) |
| Audit log | `audit_log` table | Forensic visibility; attacker may try to truncate |
| Per-user data | Conversations, approvals, memories, journal, tax docs | Privacy breach, cross-user snooping |
| Host filesystem | The container's rootfs + bind-mounts | Lateral movement, cred theft from other apps |
| LAN services | Home Assistant, Frigate, trading bots, NAS admin | SSRF pivot, data exfil, control of physical devices |

## Threat actors

1. **Internet attacker with no credentials.** Default posture. Can probe the public Tailscale-Serve endpoint (LE cert, no password); limited by bearer-auth on every route + CSRF origin check + bootstrap-loopback lock on `/setup` when the users table is empty.
2. **Household member with a valid session token.** Trusted to the extent of their assigned role + sharing mode. Admin-scoped actions still require `principal.is_admin()`; scoped tokens cap what even the primary account can reach.
3. **Compromised integration (OpenRouter downtime swapped for a malicious model host, a scanned PDF with OCR that carries an injection).** Reaches Syntaur via normal LLM I/O. Mitigated by `security::wrap_untrusted_input` markers (Phase 4.3), consent-gated writes (scheduler approval queue), per-tool allowlists, and MCP process sandboxing (Phase 4.6).
4. **Compromised MCP server binary.** Would run inside the bubblewrap sandbox — read-only rootfs view, no PID visibility into the gateway, no TTY access. Cannot read `master.key` (not bind-mounted into the sandbox).
5. **Disgruntled local admin on the TrueNAS host.** Out of scope — they have root on the machine, no app-level control prevents that. Physical security + backups protect against this one.

## Attack surfaces + current defenses

### Public HTTP(S) surface
- **Gateway on :18789 (HTTP)** — LAN-only. CSP, CSRF check, 300 req/min rate limit, bearer auth on every `/api/*` route.
- **Tailscale Serve on :443 (HTTPS)** — tailnet-only. Real LE cert. Every device on the tailnet can reach it; tailnet membership gates the first perimeter. HSTS planned.
- **MDNS + Matter** — host-networked for the voice pipeline. No public exposure; LAN only.

### Credential storage
- Secrets at rest in `vault.json` are AES-256-GCM encrypted, keyed by `master.key`. Both files are 0600 — `security::assert_startup_permissions` refuses to boot if either is wider than owner-only.
- `openclaw.json` / `syntaur.json` config references secrets by `{{vault.X}}` — no plaintext keys on disk outside the vault.
- Gateway password is Argon2id-hashed in `users` table.

### LLM-directed paths
- Scheduler voice/photo/email intake wraps user content in `<<<UNTRUSTED_INPUT_BEGIN>>>` markers + system directive banning embedded-instruction execution.
- Consent-gated writes — every scheduler mutation lands in `pending_approvals` and requires explicit user tap before touching the calendar.
- `tools::web::web_fetch` rejects loopback/RFC1918/link-local/ULA addresses + non-http(s) schemes (Phase 4.5 SSRF guard).

### Process isolation
- MCP servers wrapped with bubblewrap — ro-bind / /, tmpfs /tmp, new user + PID namespace, die-with-parent.
- No tailscale-serve / net-isolation on MCP children by default (they need HTTP).
- Gateway itself is containerized; TrueNAS app lifecycle.

### Audit trail
- `audit_log` table records: login success/fail, token refresh/revoke/mint, admin user ops, password changes, scheduler approval decisions, voice ingest, tailscale connect/disconnect/key-rotate.
- Per-user + time-range indexed. Retention TBD (operator choice).

## Known gaps (2026-04-19)

| Gap | Mitigation in place | Fix plan |
|---|---|---|
| No HSTS header yet (we emit TLS via Tailscale Serve, which controls the outer cert) | Referrer-Policy strict-origin-when-cross-origin limits token leakage to other origins | Add HSTS on the 443 branch once we have a way to detect TLS-terminated requests in the gateway |
| `/api/*` routes still accept query-string tokens (`?token=...`) for back-compat with SSE/EventSource/media elements | `cache-control: no-store` on `/api/*` keeps them out of disk cache; strict-origin referrer policy; documented deprecation warning in logs | Cookie-based session auth + CSRF double-submit token on SSE paths. Multi-month work. |
| `bubblewrap` not in the production Dockerfile yet | Installed by hand on Sean's TrueNAS container for now; fail-open fallback in code | Add `RUN apt-get install -y bubblewrap` to the Dockerfile on next image rebuild. |
| Isolation harness covers only conversations/approvals/memories/admin/me | Proves the high-signal endpoints isolate correctly | Add probes for tax/journal/knowledge/music on next pass. |
| No automated secret-rotation for non-Tailscale integrations | Tailscale auto-rotates via OAuth client-credentials | Add per-integration rotation configs (OpenRouter, Gmail, etc.) where the upstream API supports it. |

## Out of scope

- Physical security of the TrueNAS box.
- Backup integrity (handled by TrueNAS snapshots + off-site replication).
- Weather-event kinds of compromise where the whole box is taken.
- End-user device security (if Sean's wife's phone is compromised, Syntaur's tailnet session goes with it — same as any VPN app).
