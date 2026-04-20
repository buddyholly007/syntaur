# Security Policy

Syntaur is a privileged local assistant with access to files, browser automation,
smart home controls, messaging, office documents, and finance-related features.
Security reports are welcome — please read this page before filing.

## Supported versions

Only the current release line receives security fixes:

| Version | Supported |
| ------- | --------- |
| 0.4.x   | ✅ fixes land here |
| < 0.4   | ❌ unsupported |

When we cut 0.5, 0.4 will move to a 60-day maintenance window before being dropped.

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

Release artifacts will be signed with Sigstore cosign (rolling out over the
next release cycle). Until signatures are live, verify by SHA-256 against the
`checksums.txt` file attached to each GitHub Release.

Install paths that skip verification (e.g. `curl … | sh` without `--verify`)
are clearly labelled as developer-convenience only. The documented default is
the verified path.

## Operator resources

For anyone running Syntaur in production:

- **[Threat model](docs/security/threat-model.md)** — assets, threat actors, attack surfaces + current defenses, tracked gaps with fix plans.
- **[Operator hardening checklist](docs/security/operator-hardening.md)** — pre-first-user + weekly + post-upgrade + on-compromise-suspicion playbooks.
- **[Isolation harness](syntaur-isolation-tests/)** — `syntaur-isolation-tests` CLI. Proves cross-user data isolation holds after every deploy. Ships in the release bundle; invoke with `SYNTAUR_URL=… SYNTAUR_ADMIN_TOKEN=… syntaur-isolation-tests`.

## Known gaps (public)

We track our own security posture honestly. At time of writing, the following
are known and being worked on:

- **Body-token handlers still exist.** A small set of handlers read the session
  token from the JSON request body (`body["token"]`). Middleware lifts
  `Authorization: Bearer` into the body for them so clients only ever send
  Authorization, but the server-side handler code still reads the old position.
  Every such lift logs a `DEPRECATED body-token lift` warning. **Scheduled
  removal: v0.5.0** — the middleware + every handler move to a single
  Authorization-header reader at that release.
- **Long-lived tokens in SSE / WebSocket / media-element URLs.** Browser
  streaming APIs can't attach an `Authorization: Bearer` header, so those
  endpoints currently accept `?token=<long-lived>` in the URL. That's the
  session token — if it leaks into browser history, proxy access logs, or
  a referrer header, an attacker gets up to 48 hours of session access.
  **Mitigation in progress (v0.5.0):** a new `POST /api/auth/stream-token`
  endpoint mints 60-second URL-scoped tokens, and handlers can opt in via
  `resolve_principal_for_stream`. The message-stream SSE endpoint is the
  first converted reference. Remaining ~39 SSE/WS/media handlers migrate
  in subsequent v0.5.x point releases; until then each emits a
  `DEPRECATED: long-lived ?token= on stream endpoint` warning.
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
