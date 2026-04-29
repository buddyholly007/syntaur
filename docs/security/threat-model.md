# Syntaur threat model

Last updated 2026-04-29. Applies to v0.5.x.
<!-- syntaur-doc-claim applies_to_version || 0.5 -->
<!-- syntaur-doc-claim code_grep || syntaur-gateway/src/security.rs || if !host_is_private(&req_host) -->
<!-- syntaur-doc-claim code_grep || syntaur-gateway/src/mcp_sandbox.rs || SYNTAUR_ALLOW_UNSANDBOXED_MCP -->
<!-- syntaur-doc-claim code_no_match || syntaur-gateway/src/mcp_sandbox.rs || SYNTAUR_STRICT_MCP_SANDBOX=1 flips fail-open to fail-closed -->

> Tagged-claim audit. The HTML comments above are read by `syntaur-ship`'s
> `doc_audit` stage, which asserts each claim against the live workspace
> before every build. The audit also runs through the example syntax
> below; claims inside fenced code blocks (``` ... ```) are skipped as
> documentation, not assertions. To add a new claim:
>
> ```text
> <!-- syntaur-doc-claim applies_to_version || PREFIX -->
> <!-- syntaur-doc-claim code_grep        || FILE || NEEDLE -->
> <!-- syntaur-doc-claim code_no_match    || FILE || NEEDLE -->
> ```
>
> Keep `NEEDLE` small + unique. If the surrounding code is refactored
> such that the substring no longer appears, the audit fails — that's
> correct: revalidate the doc and the code together.

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
- **Gateway on :18789 (HTTP)** — LAN-only. CSP (script-src `'self' 'unsafe-inline'` for HTML, `'self'` for JSON), CSRF origin check on POST/PUT/PATCH/DELETE, 300 req/min rate limit per token+IP, bearer auth required on every `/api/*` route. `cache-control: no-store, private` on every HTML + `/api/*` + `/v1/*` response.
- **Tailscale Serve on :443 (HTTPS)** — tailnet-only. Real LE cert. Every device on the tailnet can reach it; tailnet membership gates the first perimeter. **HSTS now emitted** on every response whose Host header is not a LAN/loopback/link-local address. The earlier `X-Forwarded-Proto: https` gate was silently bypassed by Tailscale Serve's `--tls-terminated-tcp=443` L4 forwarder, which doesn't propagate that header.
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
- MCP servers wrapped with bubblewrap — ro-bind / /, tmpfs /tmp, new user + PID namespace, die-with-parent, new-session.
- **Fail-closed by default on Linux** as of v0.5.x: missing bubblewrap returns `/bin/false` (the MCP server cannot run) instead of spawning unsandboxed. Operators who genuinely need fail-open opt out with `SYNTAUR_ALLOW_UNSANDBOXED_MCP=1` (or legacy `SYNTAUR_STRICT_MCP_SANDBOX=0`). Non-Linux platforms keep the old fail-open default since there's no bubblewrap backend on macOS/Windows.
- No network-namespace isolation on MCP children by default (they need HTTP); per-server `unshare_net` config is available.
- Gateway itself is containerized; TrueNAS app lifecycle. Container UID 568 (TrueNAS `apps`, non-privileged).

### Audit trail
- `audit_log` table records: login success/fail, token refresh/revoke/mint, admin user ops, password changes, scheduler approval decisions, voice ingest, tailscale connect/disconnect/key-rotate.
- Per-user + time-range indexed. Retention TBD (operator choice).

## Known gaps (2026-04-29)

| Gap | Mitigation in place | Fix plan |
|---|---|---|
| Stream-token migration partial. Server side: `POST /api/auth/stream-token` mints 60s URL-scoped tokens; `resolve_principal_for_stream` is wired into `/api/message/{id}/stream` and `/api/research/{id}/stream`. Client side: `window.sdStreamQuery` helper now mints + falls back to `?token=` if the mint endpoint is unreachable; `pages/chat.rs` (×2), `pages/scheduler.rs`, `pages/knowledge.rs` migrated. | Server still accepts long-lived `?token=` on every stream endpoint with a `DEPRECATED:` warning log per request — no immediate compatibility loss while clients catch up. | Audit remaining stream endpoints (`<audio>` TTS srcs, `/ws/stt` WebSocket which currently has NO auth at all, music streaming). Once every client mints, flip the middleware to reject long-lived query tokens on stream paths. |
| `/ws/stt` WebSocket has no authentication | Bound to LAN; reachable only via Tailscale or local network. No state mutation possible — STT is read-only synthesis. | Add `Query<HashMap>` extraction + `state.stream_tokens.resolve` validation; couple with a client mint-then-connect dance in voice-mode JS. Tracked as the highest-priority remaining stream-token item. |
| Isolation harness covers 15+ endpoints but direct write-verb probes exist only on journal + calendar | List-visibility is the primary leakage path and it's covered everywhere; cross-user DELETE / PUT is now probed on journal-moments + calendar-events | Extend direct write-verb probes to music folders, social drafts, scheduler-lists. |
| No automated secret-rotation for non-Tailscale integrations | Tailscale auto-rotates via OAuth client-credentials | Add per-integration rotation configs (OpenRouter, Gmail, etc.) where the upstream API supports it. |
| Password policy is intentionally light: no minimum-length increase, no complexity rules, no forced rotation. See `feedback/security_no_user_friction.md` | Setup form now shows a strength meter + optional HIBP k-anonymity check (SHA-1 prefix only, password never leaves the device). Pure advisory — no rejection. | Stays as advisory by design. If breach data shows real-world abuse against household-deployed Syntaur instances, revisit. |

### Closed gaps (was previously in this list)

- **HSTS now emits in production (2026-04-29).** The original gate
  required `X-Forwarded-Proto: https`, which Tailscale Serve's
  `--tls-terminated-tcp=443` L4 forwarder does not set. Replaced with
  a host-based gate: emit HSTS whenever the Host header is not a
  LAN/loopback/link-local address. Public hostnames (e.g. the
  Tailscale-Serve `*.ts.net` URL) get HSTS; direct-IP access on the
  LAN never does. Verified via `curl -sI` against
  `https://syntaur.tail75e2be.ts.net/health`.
- **MCP sandbox fail-closed by default (2026-04-29).** v0.4.x relied on
  the prod Dockerfile setting `SYNTAUR_STRICT_MCP_SANDBOX=1` to flip
  fail-open to fail-closed; non-prod runs (and any operator using a
  custom build) silently fell through to unsandboxed spawns. Code
  default is now fail-closed on Linux when bwrap is missing — returns
  `/bin/false`. Operators who genuinely need the old behavior set
  `SYNTAUR_ALLOW_UNSANDBOXED_MCP=1` (or legacy
  `SYNTAUR_STRICT_MCP_SANDBOX=0`).
- **`bubblewrap` in the production Dockerfile** — v0.4.0 bakes bubblewrap
  into the runtime image, v0.4.1 adds bubblewrap to install.sh's
  apt/dnf/pacman/zypper package list for host installs.
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
