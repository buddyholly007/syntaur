# Security Policy

Syntaur is a privileged local assistant with access to files, browser automation,
smart home controls, messaging, office documents, and finance-related features.
Security reports are welcome — please read this page before filing.

## Supported versions

Only the current release line receives security fixes:

| Version | Supported |
| ------- | --------- |
| 0.1.x   | ✅ fixes land here |
| < 0.1   | ❌ unsupported |

When we cut 0.2, 0.1 will move to a 60-day maintenance window before being dropped.

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

- Some authenticated endpoints accept the session token in a URL query string
  as well as the `Authorization` header. The query-string path is deprecated
  and will be removed after a 30-day deprecation window.
- Release artifacts are not yet signed; Sigstore provenance lands next release.
- The `/v1/chat/completions` voice-relay endpoint requires an explicit
  `voice_secret` in config; unsetting it is allowed only when the gateway is
  bound to loopback.

Keep us honest — tell us when this list is out of date.

## Credits

Reporters are credited in published advisories (with their permission). Thank
you for making Syntaur safer.
