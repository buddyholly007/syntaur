# Security Policy

Syntaur is a privileged local assistant with access to files, browser automation,
smart home controls, messaging, office documents, and finance-related features.
Security reports are welcome — please read this page before filing.

## Supported versions

Only the current release line receives security fixes:

<!-- syntaur-doc-claim applies_to_version || 0.6 -->
| Version | Supported |
| ------- | --------- |
| 0.6.x   | ✅ fixes land here |
| 0.5.x   | ⚠️ 60-day maintenance window from 0.6.0 release date; critical fixes only |
| < 0.5   | ❌ unsupported |

<!-- syntaur-doc-claim applies_to_version || 0.6 -->
**v0.6.2 highlights** (security patch):
- `/ws/stt` WebSocket now requires a stream-token. Pre-v0.6.2 the STT WebSocket accepted any upgrade and forwarded audio to the Wyoming pipeline with no auth — the threat model called this out as the highest-priority remaining stream-token item. v0.6.2 wires the same `stream_token → Bearer → cookie → 401` resolution as `/ws/terminal`. All three browser call sites (chat voice button, voice-mode, music DJ STT) now mint via `sdStreamQuery('/ws/stt')` before opening.
- `sdStreamQuery` + `sdPrefixStreamQuery` JS helpers dropped their dead long-lived `?token=` fallback. Pre-v0.6.2 the helpers fell back to `?token=` if the mint endpoint was unreachable; that fallback silently produced 401-bound URLs once the v0.6.1 default-flip rejected legacy `?token=` on stream endpoints. Now they return `''` with a `console.warn`, so the 401-and-retry path on the calling stream is the single failure mode operators have to debug.
- Threat-model + SECURITY.md now agree on stream-token state. The threat-model "Stream-token migration partial" + "/ws/stt has NO authentication" rows moved from "Known gaps" to "Closed gaps" with `(v0.6.0 + v0.6.1)` and `(v0.6.2)` markers respectively.

**v0.6.1 highlights** (security patch):
- Long-lived `?token=` on stream endpoints is now **REJECTED by default**. The flip moved up from v0.7.0 because pre-flip prod telemetry showed zero `[auth/stream] DEPRECATED` log hits in the 7-day window — every UI surface had already migrated. Operators with custom integrations that still pass `?token=` can opt back in for one release with `SYNTAUR_ALLOW_LEGACY_STREAM_TOKEN=1` (kept for emergency only; both env names removed in v0.8.0 along with the entire `?token=` code path). The legacy `SYNTAUR_REJECT_LEGACY_STREAM_TOKEN=1` env is honored as a no-op so operators who pre-flipped don't see a behavior change.
- Non-Linux MCP startup banner — macOS / Windows now print a multi-line `warn!` block at gateway startup AND write a `gateway.start.mcp_sandbox_unavailable` `audit_log` row. Mirrors the visibility of the Linux opt-out (`SYNTAUR_ALLOW_UNSANDBOXED_MCP=1`) banner so non-Linux operators see the limitation immediately instead of waiting for the first MCP spawn warn line.
- Landing page primary install command switched from `curl … | sh` to two-step `wget` then `sh install.sh`, with copy explaining why (read the script before running it). Three new `syntaur-doc-claim` markers pin the policy: a `code_no_match` for `install.sh | sh` and `code_grep` for the wget command + `sh install.sh`.

**v0.6.0 highlights:**
- Stream-token migration **complete** for every browser-side stream surface — music file/art streaming, `/api/music/local_events` SSE, `/ws/terminal` WebSocket, the chat / knowledge / scheduler SSE flows. Every UI path mints a 60-second URL-scoped token via `POST /api/auth/stream-token` (`window.sdStreamQuery` for single resources, `window.sdPrefixStreamQuery` for directory-scoped lists). Long-lived `?token=` deprecation pipeline shipped here; default flipped to reject in v0.6.1 (see above).
- `SYNTAUR_ALLOW_UNSANDBOXED_MCP=1` (and legacy `SYNTAUR_STRICT_MCP_SANDBOX=0`) now emits a multi-line startup `error!` banner AND writes a `gateway.start.unsandboxed_mcp` row to `audit_log` so the dangerous mode is visible in `/api/audit` and incident review.
- Pages site is now a derived artifact of `landing/` — every push to main that touches the landing page redeploys via Actions, with a hard-fail check that `<!-- VERSION-BADGE -->` matches `/VERSION` before publish. Eliminates the stale-`gh-pages`-branch failure mode that pinned the public site at v0.4.
- `operator-hardening.md` now agrees with `mcp_sandbox.rs`: Linux fail-closes when bubblewrap is missing (`/bin/false` MCP child), pinned with two `<!-- syntaur-doc-claim -->` markers so future code/doc drift fails the ship pipeline.

**v0.5.0 highlights** (still load-bearing):
- `LegacyAdmin` principal + every `gateway.auth.token` / `gateway.auth.password` config-file login path deleted. Every login now hits a real user row.
- Handler-layer defense-in-depth for the first-run bootstrap endpoints — non-loopback peers are rejected at the handler *and* the middleware, not just the middleware.
- Bearer-header migration completed: all JSON handlers read the token from `Authorization: Bearer …` via `security::bearer_from_headers`.

When 0.7 ships, 0.6 moves to the same 60-day maintenance window.

## Reporting a vulnerability

**Please do not open a public issue for security reports.**

Two options, in order of preference:

1. **GitHub private vulnerability reporting.** Go to
   [this repo → Security → Report a vulnerability](../../security/advisories/new).
   This creates a private draft advisory that only the maintainer can see.

2. **Email:** `security@syntaur.dev` (reaches the maintainer directly).
   PGP key fingerprint will be published here once the keypair is rotated;
   until then plain mail is fine — do not include the exploit in the first
   message, only the summary and your preferred reply address.

When reporting, please include (as much as you can):

- Affected version(s)
- Affected component (e.g. `syntaur-gateway /api/scheduler/…`, `install.sh`)
- Reproduction steps
- Observed impact (what the attacker can do)
- Whether the issue is already public anywhere

## What happens next

| Stage | Target |
| --- | --- |
| Acknowledgement | within 72 hours of the report |
| Triage + severity | within 7 days |
| Fix landed in a release | within 90 days for High/Critical; best-effort for Medium/Low |
| Public advisory | at release, credited to the reporter unless anonymity is requested |

If a report is borderline (doesn't clearly affect a supported configuration, or
describes intended behavior), we'll say so in the acknowledgement reply and
close the thread politely.

## Scope

**In scope** — we want to hear about these:

- Authentication + session handling (`syntaur-gateway`, OAuth flows)
- Authorization / multi-user isolation bugs
- Installer or release-artifact tampering risks
- Sandbox escapes in the MCP / browser / shell-exec tool surface
- Prompt-injection paths that can reach privileged tools without the consent gate
- Secret leakage (tokens, API keys, OAuth credentials)
- Supply-chain issues in bundled dependencies

**Out of scope** — please don't file these as security issues:

- Denial-of-service against a single local instance (local-first assumption)
- Physical access attacks against the host
- Social engineering against the operator
- Reports based purely on theoretical weaknesses without a concrete attack path
- Issues in third-party services (OpenRouter, Telegram, HA) that don't translate
  into a Syntaur-side exposure

## Safe harbor

Good-faith security research is welcome. We will not pursue legal action against
researchers who:

- Give us a reasonable chance to fix the issue before going public
- Do not access or retain more data than needed to demonstrate the issue
- Do not pivot into third-party systems or other users' data
- Do not degrade the availability of production systems they are testing against

## Release-integrity verification

<!-- Pin against future-tense drift. The pre-v0.6.0 version of this section -->
<!-- promised signing was "rolling out over the next release cycle" — kept -->
<!-- saying that for two minor versions after signing went live. Forbid the -->
<!-- giveaway phrases so the audit fails before the doc ships stale again. -->
<!-- syntaur-doc-claim code_no_match || SECURITY.md || rolling out -->
<!-- syntaur-doc-claim code_no_match || SECURITY.md || next release cycle -->
<!-- syntaur-doc-claim code_no_match || SECURITY.md || Until signatures are live -->
<!-- syntaur-doc-claim code_grep || .github/workflows/release-sign.yml || cosign-installer -->
<!-- Landing page must NOT recommend a pipe-to-shell installer. Two-step  -->
<!-- (wget then sh install.sh) lets the operator read the script before  -->
<!-- running it. A 2026-04-29 reviewer noted the prior `curl ... | sh`   -->
<!-- recommendation was a regression — pin against its return.           -->
<!-- syntaur-doc-claim code_no_match || landing/index.html || install.sh | sh -->
<!-- syntaur-doc-claim code_grep    || landing/index.html || wget https://github.com/buddyholly007/syntaur/releases/latest/download/install.sh -->
<!-- syntaur-doc-claim code_grep    || landing/index.html || sh install.sh -->

Every GitHub Release is signed with Sigstore cosign via the
[`release-sign.yml`](.github/workflows/release-sign.yml) workflow under the
GitHub Actions OIDC identity. Each release ships:

- All platform binaries (gateway / viewer / isolation-tests, ×3 for Linux-x86_64
  / macOS-arm64 / Windows-x86_64).
- A `.cosign.bundle` for every binary.
- A single `checksums.txt` listing SHA-256 of every binary, plus its own
  `.cosign.bundle`.

To verify a downloaded binary:

```bash
# 1. Verify the cosign signature is from the release-sign workflow on this repo.
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/buddyholly007/syntaur/" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --bundle syntaur-gateway-linux-x86_64.cosign.bundle \
  syntaur-gateway-linux-x86_64

# 2. Cross-check against checksums.txt (also cosign-signed).
sha256sum -c <(grep syntaur-gateway-linux-x86_64 checksums.txt)
```

`install.sh` does the cosign + checksum verification automatically and aborts
on mismatch. Install paths that skip verification (e.g. `curl … | sh` without
`--verify`) are clearly labelled as developer-convenience only.

## Operator resources

For anyone running Syntaur in production:

- **[Threat model](docs/security/threat-model.md)** — assets, threat actors, attack surfaces + current defenses, tracked gaps with fix plans.
- **[Operator hardening checklist](docs/security/operator-hardening.md)** — pre-first-user + weekly + post-upgrade + on-compromise-suspicion playbooks.
- **[Isolation harness](syntaur-isolation-tests/)** — `syntaur-isolation-tests` CLI. Proves cross-user data isolation holds after every deploy. Ships in the release bundle; invoke with `SYNTAUR_URL=… SYNTAUR_ADMIN_TOKEN=… syntaur-isolation-tests`.

## Known gaps (public)

We track our own security posture honestly. At time of writing, the following
are known and being worked on:

- **Long-lived tokens in SSE / WebSocket / media-element URLs.** Browser
  streaming APIs can't attach an `Authorization: Bearer` header, so those
  endpoints historically accepted `?token=<long-lived>` in the URL. That's
  the session token — if it leaks into browser history, proxy access logs,
  or a referrer header, an attacker gets up to 48 hours of session access.
  **Mitigation complete (v0.6.0) + default flipped (v0.6.1):** every browser-side
  stream endpoint — music file/art streaming, /api/music/local_events SSE,
  /ws/terminal WebSocket, the chat / knowledge / scheduler SSE flows —
  uses `POST /api/auth/stream-token` to mint a 60-second URL-scoped token
  via `window.sdStreamQuery` (single resource) or `window.sdPrefixStreamQuery`
  (a directory of art tiles). The `resolve_principal_for_stream` helper
  is the single auth gate: stream_token → Authorization header → legacy
  `?token=` (now REJECTED by default with 401). Operators with custom
  integrations that need the old accept-with-warn behavior for one
  release can set `SYNTAUR_ALLOW_LEGACY_STREAM_TOKEN=1` — both env names
  (allow + the legacy `SYNTAUR_REJECT_LEGACY_STREAM_TOKEN=1`) are
  removed in v0.8.0 along with the entire `?token=` code path.
- The `/v1/chat/completions` voice-relay endpoint requires an explicit
  `voice_secret` in config; unsetting it is allowed only when the gateway is
  bound to loopback.
- Release installers are **not code-signed** on macOS or Windows. Gatekeeper
  and SmartScreen will warn on first launch. Sigstore + cosign signatures and
  SHA-256 checksums are published alongside every release artifact; verify
  with `install.sh`'s `--skip-verify=0` path (default).

**Removed in v0.5.0** (previously in this list):
- ~~Legacy gateway.auth.token / gateway.auth.password fallback.~~ The
  `LegacyAdmin` principal + every config-file-secret login path were
  deleted. Every login now hits a real user row. Fresh installs bootstrap
  through `/setup/register` and the syntaur.json config file no longer
  carries a usable admin credential.

**Removed in v0.4.3** (previously in this list):
- ~~Body-token handlers + `lift_bearer_to_body_and_query` middleware.~~
  Every JSON POST/PUT/PATCH handler that used to read `body["token"]` or
  `params.get("token")` now reads the bearer directly from the
  `Authorization` header via `security::bearer_from_headers`. The
  middleware that copied header→body is deleted. Three handlers
  (`handle_auth_refresh`, `handle_auth_stream_token`, the legacy
  `?token=` fallback in `resolve_principal_for_stream`) keep a
  body/query reader as a transition fallback for older clients;
  each prefers the header and the legacy paths emit deprecation warnings.

**Removed in v0.4.0** (previously in this list):
- ~~URL query-string token injection.~~ The middleware no longer lifts
  `Authorization: Bearer` into the URL query. Every shipped client sends
  tokens via header only, and no query-string token lives on disk (logs,
  browser history, referrer).
- ~~Release artifacts are not yet signed.~~ `release-sign.yml` signs every
  shipped binary (gateway + viewer + isolation-tests) with Sigstore cosign
  (keyless OIDC) and publishes SHA-256 checksums + SLSA v1 provenance
  attestation.

Keep us honest — tell us when this list is out of date.

## Credits

Reporters are credited in published advisories (with their permission). Thank
you for making Syntaur safer.
