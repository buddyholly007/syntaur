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
}

/* Light mode flips lightness pair; JS toggles .theme-light on <html>. */
html.theme-light {
  --bg-l: 0.98;
  --fg-l: 0.22;
  --line-c: 0.010;
  --accent: oklch(0.52 var(--accent-c) var(--accent-h));
  --accent-ink: oklch(0.98 0.004 var(--accent-h));
  --shadow-soft: 0 1px 2px rgb(0 0 0 / 0.05), 0 8px 24px rgb(0 0 0 / 0.06);
}

/* Only apply when the page opts in via body class; leaves per-module
   themes (Music, Coders, etc.) alone. */
body.syntaur-ambient {
  background: var(--bg) !important;
  color: var(--fg) !important;
  transition: background 800ms ease-out, color 800ms ease-out;
}
body.syntaur-ambient * { transition: background-color 400ms ease-out, border-color 400ms ease-out, color 400ms ease-out; }

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
"##;

pub const THEME_SCRIPT: &str = r##"
// Syntaur dashboard theme engine.
// Computes light/dark + accent hue from user_appearance + local time.
// Cached in localStorage so first paint on reload is instant.
(function() {
  const ACCENT_HUE = { sage: 135, indigo: 265, ochre: 70, gray: 260 };
  const DEFAULT = {
    accent: 'sage', theme_mode: 'auto', hue_shift: 1,
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
    if (pref.theme_mode === 'auto' && pref.latitude != null && pref.longitude != null) {
      try { const ss = sunriseSunset(now, pref.latitude, pref.longitude); rise = ss.rise; set = ss.set; } catch {}
    }
    let isLight;
    if (pref.theme_mode === 'light') isLight = true;
    else if (pref.theme_mode === 'dark') isLight = false;
    else isLight = minNow >= rise && minNow < set;

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
    return { isLight, hue: baseHue + hueShift + __weatherHue, rise, set };
  }

  function apply(pref) {
    const s = compute(pref, new Date());
    const root = document.documentElement;
    if (s.isLight) root.classList.add('theme-light'); else root.classList.remove('theme-light');
    root.style.setProperty('--accent-h', String(Math.round(s.hue * 10) / 10));
    root.classList.toggle('ambient-on', !!pref.ambient_mode);
    document.body.classList.add('syntaur-ambient');
    window.__syntaurThemeState = s;
  }

  // Poll /api/scheduler/today once on load for a weather summary; apply
  // its hue offset the next time the theme re-evaluates. Failures are
  // silent — weather reflection is purely decorative.
  async function pollWeather() {
    try {
      const r = await fetch('/api/scheduler/today', { credentials:'same-origin' });
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
      const r = await fetch('/api/appearance', { credentials: 'same-origin' });
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
