//! Dashboard theme engine — `oklch()` CSS tokens + JS sunrise ticker.
//!
//! Used by `/dashboard` and the Settings → Appearance live preview.
//! Per-module themes (Music cyberpunk, Knowledge parchment, Coders CRT)
//! keep their existing CSS and are unaffected — this engine only acts
//! where the body has `class="syntaur-ambient"`.
//!
//! Three CSS custom properties drive everything:
//!   --accent-h    hue (in degrees; preset + daily shift)
//!   --bg-l        background lightness (dark ≈0.14, light ≈0.98)
//!   --fg-l        foreground lightness (derived by JS when mode flips)
//!
//! All surface colors are `oklch(var(...) chroma var(--accent-h))` so
//! flipping the lightness pair is the only thing needed to switch
//! light ⇄ dark at sunrise/sunset. `oklch()` is Safari 15.4+ / Chrome
//! 111+, which covers everything we target in 2026.

pub const THEME_STYLE: &str = r##"
/* ── Syntaur dashboard theme tokens ─────────────────────────────────── */
:root {
  --accent-h: 135;      /* sage default; set by JS from user_appearance */
  --accent-c: 0.06;     /* chroma: low = calm, higher would pop */
  --bg-l:   0.14;
  --fg-l:   0.94;
  --line-c: 0.008;

  --bg:       oklch(var(--bg-l) 0.008 var(--accent-h));
  --bg-elev:  oklch(calc(var(--bg-l) + 0.030) 0.010 var(--accent-h));
  --bg-card:  oklch(calc(var(--bg-l) + 0.055) 0.012 var(--accent-h));
  --bg-hover: oklch(calc(var(--bg-l) + 0.075) 0.014 var(--accent-h));

  --fg:       oklch(var(--fg-l) 0.010 var(--accent-h));
  --fg-dim:   oklch(calc(var(--fg-l) - 0.18) 0.010 var(--accent-h));
  --fg-mute:  oklch(calc(var(--fg-l) - 0.32) 0.010 var(--accent-h));
  --fg-faint: oklch(calc(var(--fg-l) - 0.48) 0.010 var(--accent-h));

  --line:     oklch(calc(var(--bg-l) + 0.055) var(--line-c) var(--accent-h));
  --line-soft:oklch(calc(var(--bg-l) + 0.025) var(--line-c) var(--accent-h));

  --accent:     oklch(0.68 var(--accent-c) var(--accent-h));
  --accent-ink: oklch(0.20 var(--accent-c) var(--accent-h));
  --accent-soft:oklch(0.68 var(--accent-c) var(--accent-h) / 0.14);
  --accent-line:oklch(0.68 var(--accent-c) var(--accent-h) / 0.38);

  --danger: oklch(0.70 0.12 25);
  --warn:   oklch(0.76 0.10 70);
  --ok:     oklch(0.72 0.10 145);

  --shadow-soft: 0 1px 2px rgb(0 0 0 / 0.12), 0 8px 24px rgb(0 0 0 / 0.10);
  --radius-card: 16px;
  --radius-chip: 10px;
  --space: 8px;

  /* Ribbon-backdrop palette — five filament hues + a bright cream core.
     Default (:root) is the MIDDAY palette used when body.tod-midday. The
     morning + evening buckets override these via body.tod-*. See the
     concept at ~/dashboard-concept/index.html for source-of-truth
     values. */
  --rib-a:    #e8a84a;  /* warm gold */
  --rib-b:    #f3d078;  /* butter */
  --rib-c:    #c87a2c;  /* copper */
  --rib-d:    #6da89d;  /* sage teal */
  --rib-e:    #b4d3c5;  /* pale mint */
  --rib-core: #fff5de;  /* cream filament highlight */

  /* Time-of-day backdrop palette — layered gradient bg behind ribbons.
     Default is MIDDAY navy; morning/evening override via body.tod-*. */
  --bg-top:       #041A33;
  --bg-bot:       #0B3A6A;
  --bg-vignette:  rgba(60, 150, 220, 0.22);
  --bg-vignette2: rgba(220, 170, 90, 0.10);
}

/* Light mode flips lightness pair; JS toggles .theme-light on <html>.
   Kept for non-dashboard ambient surfaces (Settings preview, etc.) that
   don't set body.tod-*. Dashboard itself is driven by the tod classes. */
html.theme-light {
  --bg-l: 0.98;
  --fg-l: 0.22;
  --line-c: 0.010;
  --accent: oklch(0.52 var(--accent-c) var(--accent-h));
  --accent-ink: oklch(0.98 0.004 var(--accent-h));
  --shadow-soft: 0 1px 2px rgb(0 0 0 / 0.05), 0 8px 24px rgb(0 0 0 / 0.06);
}

/* ── Time-of-day palette buckets (ribbons + backdrop) ───────────────── */
/* Driven by body.tod-morning | tod-midday | tod-evening, which
   THEME_SCRIPT computes from compute() and applies on every tick. */
body.tod-morning {
  --rib-a:    #da8a34;  /* deep amber */
  --rib-b:    #f0b85a;  /* gold */
  --rib-c:    #c25d1f;  /* copper */
  --rib-d:    #6da89d;  /* sage teal */
  --rib-e:    #b4d3c5;  /* pale mint */
  --rib-core: #fffaf0;

  --bg-top:       #f6eddb;
  --bg-bot:       #e9dcc3;
  --bg-vignette:  rgba(228, 188, 120, 0.32);
  --bg-vignette2: rgba(180, 210, 205, 0.18);

  /* Concept tile tokens — exact rgba values from dashboard-concept/index.html */
  --card-bg:     rgba(255, 251, 243, 0.82);
  --card-brd:    rgba(214, 168, 92, 0.45);
  --card-glow:   inset 0 0 0 1px rgba(255, 240, 210, 0.55),
                 0 0 0 1px rgba(214, 168, 92, 0.28),
                 0 10px 32px -14px rgba(150, 100, 30, 0.18);
  --card-pad-bg: rgba(252, 245, 230, 0.75);

  /* Concept accent — deep amber for buttons, focus rings, chat bubbles. */
  --accent:      #be7224;
  --accent-ink:  #2e2414;
  --accent-soft: rgba(190, 114, 36, 0.12);
  --accent-line: rgba(190, 114, 36, 0.38);
}
body.tod-midday {
  --rib-a:    #e8a84a;
  --rib-b:    #f3d078;
  --rib-c:    #c87a2c;
  --rib-d:    #6da89d;
  --rib-e:    #b4d3c5;
  --rib-core: #fff5de;

  --bg-top:       #041a33;
  --bg-bot:       #0b3a6a;
  --bg-vignette:  rgba(60, 150, 220, 0.22);
  --bg-vignette2: rgba(220, 170, 90, 0.10);

  --card-bg:     rgba(10, 35, 65, 0.82);
  --card-brd:    rgba(180, 210, 240, 0.22);
  --card-glow:   inset 0 0 0 1px rgba(255, 230, 180, 0.12),
                 0 0 0 1px rgba(180, 210, 240, 0.18),
                 0 12px 36px -14px rgba(0, 80, 150, 0.55);
  --card-pad-bg: rgba(8, 28, 54, 0.72);

  /* Concept accent — butter gold for buttons, focus rings. */
  --accent:      #f0c06a;
  --accent-ink:  #0a1a30;
  --accent-soft: rgba(240, 192, 106, 0.15);
  --accent-line: rgba(240, 192, 106, 0.40);
}
body.tod-evening {
  --rib-a:    #e8a84a;
  --rib-b:    #f3d078;
  --rib-c:    #c06e28;  /* rust */
  --rib-d:    #2b6e7a;  /* dim teal */
  --rib-e:    #9fc7d0;  /* pale teal */
  --rib-core: #fff3d9;  /* warm cream */

  --bg-top:       #02101f;
  --bg-bot:       #07263d;
  --bg-vignette:  rgba(210, 150, 60, 0.18);
  --bg-vignette2: rgba(40, 100, 120, 0.08);

  --card-bg:     rgba(10, 30, 48, 0.86);
  --card-brd:    rgba(210, 150, 60, 0.30);
  --card-glow:   0 0 0 1px rgba(210, 150, 60, 0.22),
                 0 10px 32px -12px rgba(130, 80, 20, 0.45);
  --card-pad-bg: rgba(10, 28, 45, 0.82);

  /* Concept accent — warm amber. */
  --accent:      #e8b85a;
  --accent-ink:  #1a1208;
  --accent-soft: rgba(232, 184, 90, 0.15);
  --accent-line: rgba(232, 184, 90, 0.40);
}

/* Only apply when the page opts in via body class; leaves per-module
   themes (Music, Coders, etc.) alone. */
body.syntaur-ambient {
  background: var(--bg) !important;
  color: var(--fg) !important;
}
/* Transitions only engage after the first paint has landed (theme.rs
   adds .theme-ready after the initial apply). This prevents the 800ms
   fade-to-white flash on load when the cached pref differs from the
   default body color. */
body.syntaur-ambient.theme-ready {
  transition: background 600ms ease-out, color 600ms ease-out;
}
body.syntaur-ambient.theme-ready * {
  transition: background-color 400ms ease-out, border-color 400ms ease-out, color 400ms ease-out;
}

/* ── Ambient mode (opt-in via html.ambient-on) ─────────────────────── */
/* Very subtle breathing on the accent chroma — barely perceptible, but
   gives the interface a living feel. Honors prefers-reduced-motion. */
@keyframes sdBreath {
  0%, 100% { --accent-c: 0.060; }
  50%      { --accent-c: 0.066; }
}
html.ambient-on { animation: sdBreath 6.5s ease-in-out infinite; }
@media (prefers-reduced-motion: reduce) {
  html.ambient-on { animation: none; }
}

/* Drifting dusk motes — only visible in evening / night palette and
   only with ambient on. Pure CSS, no canvas. */
html.ambient-on body.syntaur-ambient::before {
  content: ""; position: fixed; inset: 0; pointer-events: none; z-index: 1;
  background-image:
    radial-gradient(1.5px 1.5px at 22% 33%, color-mix(in oklab, var(--accent) 60%, transparent) 50%, transparent 51%),
    radial-gradient(1.5px 1.5px at 68% 57%, color-mix(in oklab, var(--accent) 55%, transparent) 50%, transparent 51%),
    radial-gradient(1.5px 1.5px at 41% 81%, color-mix(in oklab, var(--accent) 50%, transparent) 50%, transparent 51%),
    radial-gradient(1.5px 1.5px at 85% 19%, color-mix(in oklab, var(--accent) 58%, transparent) 50%, transparent 51%),
    radial-gradient(1.5px 1.5px at 12% 72%, color-mix(in oklab, var(--accent) 52%, transparent) 50%, transparent 51%);
  opacity: 0.35;
  animation: sdMotes 42s linear infinite;
  background-size: 200% 200%;
}
html.theme-light.ambient-on body.syntaur-ambient::before { opacity: 0.18; }
@keyframes sdMotes {
  from { background-position: 0% 0%, 20% 40%, 80% 10%, 60% 70%, 30% 90%; }
  to   { background-position: 60% 100%, 80% 140%, 140% 110%, 120% 170%, 90% 190%; }
}
@media (prefers-reduced-motion: reduce) {
  html.ambient-on body.syntaur-ambient::before { animation: none; opacity: 0.12; }
}

/* ── SVG ribbon backdrop ────────────────────────────────────────────── */
/* Active when the dashboard injects <svg class="sd-rb">. html.rb-on is
   set pre-paint in dashboard.rs so card translucency + bg gradient land
   on the first frame. Suppresses ambient motes (they'd clash with
   filament ribbons) and replaces syntaur-ambient's flat --bg with the
   concept's layered time-of-day gradient. */
html.rb-on.ambient-on body.syntaur-ambient::before { display: none; }
/* Gradient backdrop lives in DASHBOARD_STYLE as a fixed ::after
   pseudo-element (bypassing html/body canvas propagation quirks). Here
   we just make sure syntaur-ambient's flat var(--bg) doesn't cover it. */
html.rb-on body.syntaur-ambient {
  background: transparent !important;
}

/* The ribbon SVG itself: fixed, full-bleed, behind everything. */
.sd-rb {
  position: fixed;
  inset: 0;
  width: 100vw;
  height: 100vh;
  z-index: -1;
  pointer-events: none;
  display: block;
}

/* @property lets CSS smoothly interpolate numeric custom properties —
   without this, .rb.ignite would flip the opacity band instantly
   (the "click-on" flicker the prior session fought). Declaring these
   as <number> makes the 3s transition on .sd-rb .rb interpolate. */
@property --rb-base { syntax: '<number>'; inherits: true; initial-value: 0.45; }
@property --rb-peak { syntax: '<number>'; inherits: true; initial-value: 0.62; }

/* Per-ribbon structure: halo (wide soft) / mid / core (thin bright).
   Bloom is from the 18px halo stroke at low opacity — NO filter, no
   mix-blend-mode. Concept proved this reads cleanly over both the
   cream morning and the navy midday/evening backdrops. */
.sd-rb .rb {
  --rb-base: 0.45;
  --rb-peak: 0.62;
  opacity: var(--rb-base);
  animation: rbBreath var(--rb-dur, 16s) ease-in-out var(--rb-delay, 0s) infinite;
  transition: --rb-base 3s ease-in-out, --rb-peak 3s ease-in-out;
}
body.tod-morning .sd-rb .rb { --rb-base: 0.55; --rb-peak: 0.72; }
body.tod-evening .sd-rb .rb { --rb-base: 0.42; --rb-peak: 0.58; }

/* Ignite — shifts the breathing band up for ~6s without stopping the
   keyframe. Because the keyframe reads --rb-base / --rb-peak live and
   @property interpolates them over 3s, the whole oscillation drifts
   smoothly higher and back. No snap, no click-on. */
.sd-rb .rb.ignite {
  --rb-base: 0.58;
  --rb-peak: 0.80;
}
body.rb-paused .sd-rb .rb { animation-play-state: paused; }

.sd-rb .rb path { fill: none; stroke-linecap: round; stroke-linejoin: round; stroke: currentColor; }
.sd-rb .rb .halo { stroke-width: 18; opacity: 0.15; }
.sd-rb .rb .mid  { stroke-width: 6;  opacity: 0.40; }
.sd-rb .rb .core { stroke-width: 1.3; opacity: 0.95; stroke: var(--rib-core); }
body.tod-morning .sd-rb .rb .halo { stroke-width: 20; opacity: 0.10; }
body.tod-morning .sd-rb .rb .mid  { stroke-width: 4;  opacity: 0.38; }
body.tod-morning .sd-rb .rb .core { opacity: 0.85; }

@keyframes rbBreath {
  0%, 100% { opacity: var(--rb-base); }
  45%      { opacity: var(--rb-peak); }
}
@media (prefers-reduced-motion: reduce) {
  .sd-rb .rb { animation: none; opacity: var(--rb-base); }
}

/* Tile chrome — per-tod rgba bg + border + box-shadow glow, exactly
   matching the concept at ~/dashboard-concept/index.html. `!important`
   is needed to win against DASHBOARD_STYLE's `.sd-tile { background:
   var(--bg-card) }` rule which appears later in the cascade. */
html.rb-on .sd-tile {
  background: var(--card-bg) !important;
  border: 1px solid var(--card-brd) !important;
  border-radius: 18px !important;
  box-shadow: var(--card-glow) !important;
  overflow: hidden;
}

/* Per-tile hover backlight — soft color-shifted glow outside the card.
   --sd-hover-hue is assigned by RIBBON_SCRIPT at mount time, rotating
   through the ribbon palette so adjacent tiles pulse in different
   colors. Concept values exactly: inset:-6px, 130% 130% radial,
   38% mix at 82% stop. */
html.rb-on .sd-tile { --sd-hover-hue: var(--rib-a); }
html.rb-on .sd-tile::after {
  content: "";
  position: absolute;
  inset: -6px;
  border-radius: inherit;
  pointer-events: none;
  opacity: 0;
  background: radial-gradient(
    130% 130% at 50% 50%,
    transparent 58%,
    color-mix(in oklab, var(--sd-hover-hue) 38%, transparent) 82%,
    transparent 100%
  );
  transition: opacity 700ms ease;
  z-index: -1;
}
html.rb-on .sd-grid[data-mode="view"] .sd-tile:hover::after { opacity: 1; }
html.rb-on .sd-grid[data-mode="view"] .sd-tile:hover {
  border-color: color-mix(in oklab, var(--sd-hover-hue) 50%, var(--card-brd)) !important;
  box-shadow:
    inset 0 0 0 1px color-mix(in oklab, var(--sd-hover-hue) 35%, transparent),
    0 0 0 1px color-mix(in oklab, var(--sd-hover-hue) 25%, transparent),
    0 18px 42px -14px color-mix(in oklab, var(--sd-hover-hue) 40%, transparent) !important;
}

/* Input pills (chat, todo) use --card-pad-bg so they read as a soft
   inset against the translucent tile, not as opaque oklch blobs. */
html.rb-on .sd-chat-input,
html.rb-on .sd-todo-input,
html.rb-on .sd-chat-body {
  background: var(--card-pad-bg) !important;
  border-color: var(--card-brd) !important;
}

/* Top bar uses card tokens when rb-on so the navy rgba(15,23,42,0.82)
   doesn't fight the tod palette. Kills the hard-coded light gray
   foreground in favor of --fg so text reads on morning cream too.
   Adds an amber under-glow so the bar reads as a distinct band
   instead of blending into the dark-palette backdrop gradient. */
html.rb-on .syntaur-topbar {
  background: var(--card-bg) !important;
  border-bottom: 1px solid var(--accent-line) !important;
  box-shadow: 0 1px 0 var(--accent-soft), 0 6px 20px -12px var(--accent-soft) !important;
  color: var(--fg) !important;
}
html.rb-on .syntaur-topbar .brand,
html.rb-on .syntaur-topbar .brand-text,
html.rb-on .syntaur-topbar .module-name,
html.rb-on .syntaur-topbar .modules-btn,
html.rb-on .syntaur-topbar .avatar-btn .chev {
  color: var(--fg) !important;
}
html.rb-on .syntaur-topbar .crumb-sep {
  color: var(--fg-mute) !important;
}
html.rb-on .syntaur-topbar .modules-btn:hover,
html.rb-on .syntaur-topbar .avatar-btn:hover {
  background: var(--accent-soft) !important;
}
html.rb-on .syntaur-topbar .modules-btn .kbd {
  background: var(--card-pad-bg) !important;
  border-color: var(--card-brd) !important;
  color: var(--fg-dim) !important;
}

/* Widget content overflow — tile has `overflow: hidden` on the outer
   box so the ::after backlight doesn't bleed out. That means any
   content the widget renders past the card-body's flex region gets
   visually clipped at the tile edge. Concession: let card-body scroll
   internally so every byte of content stays reachable. `min-height: 0`
   is already set (line 478 of dashboard.rs) so this plays nicely with
   the parent flex. */
html.rb-on .sd-card-body {
  overflow: auto;
}

/* Accessibility tuning for rb-on dashboards (Opus-flagged WCAG AA):
   Secondary text derived from --fg-mute reads at ~3.5:1 contrast
   against the dark-palette backdrop, below AA's 4.5:1 normal-text
   requirement. Lift --fg-mute + --fg-faint by 0.04 when rb-on so
   labels / timestamps / "NEXT" pills in CALENDAR / NOW PLAYING / TO
   DO stay legible over the ribbon-backdropped tiles. */
html.rb-on body.tod-midday, html.rb-on body.tod-evening {
  --fg-mute:  oklch(calc(var(--fg-l) - 0.28) 0.010 var(--accent-h));
  --fg-faint: oklch(calc(var(--fg-l) - 0.42) 0.010 var(--accent-h));
}

/* Music-player control buttons — 32x32 was below the 44x44 minimum
   touch target for mobile accessibility. Bump on mobile viewports
   specifically so desktop density stays tight. */
@media (max-width: 768px) {
  html.rb-on .sd-np-btn {
    width: 44px;
    height: 44px;
    font-size: 16px;
  }
}
"##;

pub const THEME_SCRIPT: &str = r##"
// Syntaur dashboard theme engine.
// Computes light/dark + accent hue from user_appearance + local time.
// Cached in localStorage so first paint on reload is instant.
(function() {
  const ACCENT_HUE = { sage: 135, indigo: 265, ochre: 70, gray: 260 };
  const DEFAULT = {
    accent: 'sage', theme_mode: 'dark', hue_shift: 0,
    latitude: null, longitude: null,
    light_start_min: 420, dark_start_min: 1140,
    ambient_mode: 0
  };
  // Weather → hue nudge. Cloudy / rainy → cooler; sunny → warmer. The
  // data lives on /api/scheduler/today (returns null today); when
  // present we apply this offset on top of hue_shift.
  const WEATHER_HUE = {
    sunny: 8, clear: 8, fair: 5, hot: 10,
    cloudy: -4, overcast: -6, fog: -6, mist: -5,
    rain: -10, drizzle: -8, snow: -12, storm: -14, thunder: -14
  };
  function weatherHueFor(summary) {
    if (!summary) return 0;
    const s = summary.toLowerCase();
    for (const k of Object.keys(WEATHER_HUE)) if (s.includes(k)) return WEATHER_HUE[k];
    return 0;
  }
  let __weatherHue = 0;

  // NOAA-ish sunrise/sunset, accurate to ~1 minute for mid-latitudes.
  // Returns {rise,set} as minutes-from-local-midnight.
  function sunriseSunset(date, lat, lon) {
    const rad = Math.PI / 180, deg = 180 / Math.PI;
    const start = Date.UTC(date.getFullYear(), 0, 0);
    const n = Math.floor((date.getTime() - start) / 86400000);
    const Jnoon = 2451545.0 + 0.0009 + (-lon / 360) + n;
    const M = (357.5291 + 0.98560028 * (Jnoon - 2451545)) % 360;
    const C = 1.9148*Math.sin(M*rad) + 0.02*Math.sin(2*M*rad);
    const lam = (M + C + 180 + 102.9372) % 360;
    const decl = Math.asin(Math.sin(lam*rad) * Math.sin(23.44*rad));
    const cosH0 = (Math.sin(-0.83*rad) - Math.sin(lat*rad)*Math.sin(decl)) / (Math.cos(lat*rad)*Math.cos(decl));
    if (cosH0 < -1 || cosH0 > 1) return { rise: 420, set: 1140 }; // polar
    const h0 = Math.acos(cosH0) * deg;
    const tz = -date.getTimezoneOffset() / 60;
    const noonMin = (12 + tz + lon/15) * 60;
    const delta = h0 * 4;  // degrees → minutes of day
    return { rise: Math.max(0, noonMin - delta), set: Math.min(1439, noonMin + delta) };
  }

  function compute(pref, now) {
    const minNow = now.getHours()*60 + now.getMinutes();
    let rise = pref.light_start_min, set = pref.dark_start_min;
    const hasGeo = pref.latitude != null && pref.longitude != null;
    if (pref.theme_mode === 'auto' && hasGeo) {
      try { const ss = sunriseSunset(now, pref.latitude, pref.longitude); rise = ss.rise; set = ss.set; } catch {}
    }
    let isLight;
    if (pref.theme_mode === 'light') isLight = true;
    else if (pref.theme_mode === 'dark') isLight = false;
    else if (pref.theme_mode === 'schedule') isLight = minNow >= rise && minNow < set;
    // `auto` without a known location: stay dark. Flipping to light
    // based on a 07:00-19:00 guess caused the "blinding white at noon"
    // regression — auto now requires geolocation to opt into light.
    else if (pref.theme_mode === 'auto' && hasGeo) isLight = minNow >= rise && minNow < set;
    else isLight = false;

    let hueShift = 0;
    if (pref.hue_shift) {
      if (minNow < rise || minNow >= set) {
        hueShift = -12;
      } else {
        const noon = (rise + set) / 2;
        const half = Math.max(90, (set - rise) / 2);
        const phase = (minNow - noon) / half;
        hueShift = Math.cos(Math.max(-1, Math.min(1, phase)) * Math.PI / 2) * 18;
      }
    }
    const baseHue = ACCENT_HUE[pref.accent] != null ? ACCENT_HUE[pref.accent] : ACCENT_HUE.sage;

    // Time-of-day bucket drives the dashboard ribbon + bg gradient
    // palette. Morning when isLight (first light through afternoon);
    // evening in the 2h before dusk + overnight; midday otherwise.
    // When the user forces dark mode during daylight, we still route
    // to midday so the navy palette reads correctly.
    let tod;
    if (isLight) {
      tod = 'morning';
    } else {
      const preDusk = minNow >= (set - 120) && minNow < set;
      const overnight = minNow < rise || minNow >= set;
      tod = (preDusk || overnight) ? 'evening' : 'midday';
    }
    return { isLight, hue: baseHue + hueShift + __weatherHue, rise, set, tod };
  }

  function apply(pref) {
    const s = compute(pref, new Date());
    const root = document.documentElement;
    if (s.isLight) root.classList.add('theme-light'); else root.classList.remove('theme-light');
    root.style.setProperty('--accent-h', String(Math.round(s.hue * 10) / 10));
    root.classList.toggle('ambient-on', !!pref.ambient_mode);
    document.body.classList.add('syntaur-ambient');
    // Swap time-of-day bucket class on body (drives ribbon + bg vars).
    document.body.classList.remove('tod-morning','tod-midday','tod-evening');
    document.body.classList.add('tod-' + s.tod);
    // Enable transitions only after the first apply has committed, so
    // the initial paint doesn't animate in from gray-950.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => document.body.classList.add('theme-ready'));
    });
    window.__syntaurThemeState = s;
  }

  // Poll /api/scheduler/today once on load for a weather summary; apply
  // its hue offset the next time the theme re-evaluates. Failures are
  // silent — weather reflection is purely decorative.
  const authFetch = (u, o) => (window.sdFetch ? window.sdFetch(u, o) : fetch(u, o));
  async function pollWeather() {
    try {
      const r = await authFetch('/api/scheduler/today', { credentials:'same-origin' });
      if (!r.ok) return;
      const d = await r.json();
      const summary = d && d.weather && d.weather.summary ? d.weather.summary : null;
      __weatherHue = weatherHueFor(summary);
      apply(cachedPref());
    } catch {}
  }

  function cachedPref() {
    try { return JSON.parse(localStorage.getItem('syntaur:appearance') || 'null') || DEFAULT; }
    catch { return DEFAULT; }
  }

  async function loadPref() {
    apply(cachedPref());
    try {
      const r = await authFetch('/api/appearance', { credentials: 'same-origin' });
      if (!r.ok) return;
      const fresh = await r.json();
      if (fresh && fresh.accent) {
        localStorage.setItem('syntaur:appearance', JSON.stringify(fresh));
        apply(fresh);
      }
    } catch {}
  }

  loadPref();
  pollWeather();
  // Re-check every 15 min so the UI crossfades at sunrise/sunset.
  setInterval(() => apply(cachedPref()), 15 * 60 * 1000);
  // Re-check weather every 30 min so a passing storm eventually
  // registers — cheap (the endpoint is local).
  setInterval(pollWeather, 30 * 60 * 1000);

  // Exposed for the Settings → Appearance live preview + save flow.
  window.SyntaurTheme = {
    apply, compute,
    ACCENT_HUES: ACCENT_HUE,
    currentPref: cachedPref,
    setPref(p) {
      localStorage.setItem('syntaur:appearance', JSON.stringify(p));
      apply(p);
    },
  };
})();
"##;
