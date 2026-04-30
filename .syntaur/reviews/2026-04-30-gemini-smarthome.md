# Gemini reviews — 2026-04-30 (smart-home auth-header fix)

Findings surfaced while fixing Sean's prod bug "Smart Home module
loads but no devices visible, can't navigate within". Root cause:
`shFetch` and three energy fetch helpers in `pages/smart_home.rs`
called plain `fetch()` with no Bearer token. Every `/api/smart-home/*`
call returned 401, page stuck on "Loading…" forever.

```finding
id: smart_home-shfetch-no-auth-header
reviewer: claude
file: syntaur-gateway/src/pages/smart_home.rs
line: 731
claim: shFetch helper called plain fetch() with no Authorization: Bearer header. Every /api/smart-home/* call from the page returned 401, leaving the UI stuck on "Loading…" placeholders with no devices visible.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:731 (pre-fix) — `async function shFetch(path, opts) { const r = await fetch(path, opts || {}); ... }` with no Authorization injection. Verified in DB on prod that user_id=1 owns 18 rooms + 3 esphome_proxy devices, so the data was fine — purely a JS auth-header omission.
resolution: fix-in-place — shFetch now reads syntaur_token from sessionStorage|localStorage via shToken() and sets `Authorization: Bearer <tok>` when present. Mirrors the dashboard.rs::sdFetch + music.rs::authFetch pattern.
```

```finding
id: smart_home-energy-fetch-no-auth
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 1441
claim: loadEnergyAnomalies, loadEnergyMonth, loadEnergyDay use plain fetch() instead of shFetch — they will 401 in production.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:1441, :1530, :1575 — three `fetch('/api/smart-home/energy/...')` calls with `credentials: 'same-origin'` only.
resolution: fix-in-place — same auth-header pattern threaded into each `fetch()` opts object using shToken(). Better long-term refactor would be to push them through shFetch, but the inline option-merge keeps blast radius minimal for the hotfix commit.
```

```finding
id: smart_home-maud-hallucinated-syntax-error
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 62
claim: "@for (slug, glyph, name, sub) in &[..]" was supposedly replaced with "@syntaur-gateway/src/terminal/forwarding.rs (slug, glyph, name, sub) in &[..]" — invalid Maud syntax that would fail compilation.
verdict: FALSE
evidence: syntaur-gateway/src/pages/smart_home.rs:62 quote → "@for (slug, glyph, name, sub) in &[". cargo check passed in 1m 39s with no errors. Direct read confirms the actual code is correct.
resolution: wont-fix: Gemini hallucination. The reviewer appears to have generated a plausible-looking corruption from token-pattern matching against an unrelated terminal/forwarding.rs path elsewhere in the workspace.
```

```finding
id: smart_home-light-kinds-tdz-fragile
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 1305
claim: LIGHT_KINDS / SECURITY_KINDS const declarations live below loadRoomCards/renderRoomCard which reference them; called at IIFE init before line 1305 runs, this could throw ReferenceError if the network round-trip resolves before the const is declared.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:1044 calls loadRoomCards() at IIFE init; renderRoomCard at :861 uses LIGHT_KINDS at :865-ish; consts declared at :1305-1306 (well below). Fragile but masked by async — the fetch promise hasn't resolved by the time line 1305 executes synchronously.
resolution: tracked — pre-existing. Fix: hoist LIGHT_KINDS / SECURITY_KINDS to the top of the IIFE near shToken(). Out of scope for the hotfix commit; works in practice today.
```

```finding
id: smart_home-clock-date-dead-code
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 794
claim: tickClock() updates `sh-clock` and `sh-date` DOM elements, but neither id is rendered anywhere in this page or in shared.rs's shell.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:794 ($('sh-clock')) + :800 ($('sh-date')); grep confirms no matching id in the rendered HTML.
resolution: tracked — pre-existing. Currently the function is harmless dead code (it null-checks before mutating). The clock UI was probably planned but dropped from the locked layout. Cleanup: remove tickClock() + setInterval(tickClock, 1000) entirely. Out of scope for the hotfix.
```

```finding
id: smart_home-backdrop-home-env-var
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 0
claim: handle_backdrop is safe from path traversal (whitelist match), but std::env::var("HOME") may be unreliable in containerized contexts; a config-dir provider (dirs/dirs-next/etc.) would be safer.
verdict: DEPENDENT
evidence: syntaur-gateway/src/pages/smart_home.rs:1 — file references env::var("HOME") in handle_backdrop. The Docker container sets HOME=/home/syntaur reliably (verified at /proc/<pid>/environ).
resolution: tracked — pre-existing. Real concern only on a host where the syntaur-gateway process inherits a stripped env; not the case in the Docker app on TrueNAS today. Move to a `cfg.data_dir` plumb-through if the deployment matrix expands.
```

```finding
id: smart_home-anomaly-energyday-xss-innerhtml
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 1480
claim: renderAnomalyCallout and loadEnergyDay insert device_name into innerHTML; a malicious device name like `<img src=x onerror=alert(1)>` would execute in the dashboard.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:1480-ish (anomaly host.innerHTML) and :1610-ish (loadEnergyDay leaderboard rows) — `innerHTML = ... + (row.device_name || ...) + ...`.
resolution: tracked — pre-existing. Single-tenant in practice (Sean names his own devices) so attack surface is minimal today, but real defense-in-depth concern. Fix: switch to textContent for name insertion or pass through escapeHtml(). Out of scope for the auth-header hotfix.
```

```finding
id: smart_home-backdrop-default-tovec-clone
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 1684
claim: handle_backdrop clones BACKDROP_DEFAULT into a new Vec<u8> on every fallback even though the static byte slice could be served directly via Body::from.
verdict: TRUE
evidence: syntaur-gateway/src/pages/smart_home.rs:1684 — `Err(_) => default.to_vec(),`. The `default: &[u8]` is a static byte slice (one of BACKDROP_MORNING/MIDDAY/EVENING/NIGHT, all `include_bytes!`), so the to_vec() allocates+copies the embedded image bytes on every fallback hit instead of streaming the static slice directly via Body::from(&'static [u8]).
resolution: tracked — pre-existing pattern preserved through the c4bfb2a rename. Trivial perf (one Vec alloc per request, image is ~2 MB). Fix: switch to `Body::from(default)` to avoid the copy. Out of scope for v0.6.6.
```

```finding
id: smart_home-gemini-forwarding-rs-cross-talk
reviewer: gemini
file: syntaur-gateway/src/pages/smart_home.rs
line: 1
claim: Multiple Gemini reviews of smart_home.rs included findings about forwarding.rs (rusqlite::Connection::open per-request, no connection pool, missing SSH tunnel lifecycle, no bind_host validation).
verdict: FALSE
evidence: syntaur-gateway/src/pages/smart_home.rs:1 (review-scope file) is unrelated to syntaur-gateway/src/terminal/forwarding.rs:1 (the file the reviewer raised findings about). The smart_home.rs commit does not touch forwarding.rs; Gemini cross-talked between files in the same review pass — likely a context-window artifact where the reviewer pulled in an unrelated file.
resolution: wont-fix in this commit. Forwarding-related findings, if real, belong in a separate review pass scoped to that file. Logging here so review_triage doesn't refuse to ship over scope-confused reviewer output.
```
