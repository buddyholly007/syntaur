# Syntaur threat model

Last updated 2026-04-20. Applies to v0.4.x.

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

## Known gaps (2026-04-20)

| Gap | Mitigation in place | Fix plan |
|---|---|---|
| HSTS is emitted only on requests whose `X-Forwarded-Proto` is `https` (Tailscale Serve sidecar path) | Plain-HTTP LAN access deliberately omits HSTS to avoid wedging WebKitGTK sessions after a one-off cert bounce; public HTTPS path gets the header | Formalize the proxy contract: once every documented public path terminates TLS before hitting the gateway, emit HSTS unconditionally. |
| ~39 SSE / WebSocket / media-element handlers still accept long-lived `?token=...` | Stream-token infrastructure shipped (`POST /api/auth/stream-token`, 60s URL-scoped); `handle_message_stream` is the first converted reference; every long-lived use logs a `DEPRECATED: long-lived ?token= on stream endpoint` warning | Migrate remaining handlers through v0.5.x point releases; flip the middleware to reject query-string tokens entirely once the count reaches 0. |
| Isolation harness covers 15+ endpoints (conversations, approvals, memories, admin, me, tax, journal, knowledge, music, social, calendar, scheduler-lists, research) but direct write-verb probes exist only on journal + calendar | List-visibility is the primary leakage path and it's covered everywhere; cross-user DELETE / PUT is now probed on journal-moments + calendar-events | Extend direct write-verb probes to music folders, social drafts, scheduler-lists. |
| No automated secret-rotation for non-Tailscale integrations | Tailscale auto-rotates via OAuth client-credentials | Add per-integration rotation configs (OpenRouter, Gmail, etc.) where the upstream API supports it. |

### Closed gaps (was previously in this list)

- **`bubblewrap` in the production Dockerfile** — v0.4.0 bakes bubblewrap
  into the runtime image, v0.4.1 adds bubblewrap to install.sh's
  apt/dnf/pacman/zypper package list for host installs, and
  `SYNTAUR_STRICT_MCP_SANDBOX=1` in the prod Dockerfile flips fail-open
  to fail-closed.
- **Legacy `gateway.auth.token` / `gateway.auth.password` fallback** —
  removed entirely in v0.4.1. Every login now hits a real `users` row;
  config-file-secret login paths are gone. The `LegacyAdmin` principal
  and legacy-token extractor fallback were deleted.
- **Audit log retention** — v0.4.1 spawns a daily task that trims
  pre-hash-chain rows older than 90 days while preserving every chain-
  verified row so `/api/audit/verify` can walk the complete chain.
- **Prompt-injection boundaries on tax / journal / image-gen** — vendor
  descriptions, journal STT transcripts, and vision-model captions of
  user-uploaded images are now wrapped with `UNTRUSTED_INPUT_*` markers
  and accompanied by `UNTRUSTED_INPUT_SYSTEM_DIRECTIVE`, on the same
  pattern scheduler voice/photo/email use.
- **Browser SSRF parity with `web_fetch`** — `browser_open` +
  `browser_open_and_fill` now call `check_url_safe` before navigating,
  so prompt-inject can no longer pivot headless Chromium through the
  cloud-metadata / loopback / RFC1918 paths that `web_fetch` already
  blocks.
- **rustls-webpki CVEs (RUSTSEC-2026-0049/0098/0099)** — rumqttc 0.24 →
  0.25 with default features off drops rustls-webpki 0.102.8 from the
  tree entirely; 0.103.11 bumped to 0.103.12. cargo-audit CI carries a
  documented `--ignore` allowlist for upstream-only unmaintained-crate
  warnings.

## Out of scope

- Physical security of the TrueNAS box.
- Backup integrity (handled by TrueNAS snapshots + off-site replication).
- Weather-event kinds of compromise where the whole box is taken.
- End-user device security (if Sean's wife's phone is compromised, Syntaur's tailnet session goes with it — same as any VPN app).
