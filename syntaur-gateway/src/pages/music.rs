//! /music — music dashboard. Now-playing, AI DJ, queue, speakers, EQ.
//! Migrated from static/music.html. The structural markup and the 36 KB
//! JS block live as raw-string consts — all bytes count as Rust.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Music",
        authed: false,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (PreEscaped(BODY_HTML))
        script { (PreEscaped(MUSIC_JS)) }
    };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  /* Rajdhani — condensed display face, Open Font License (free).
     Used for HUD-style headings and the breadcrumb. */
  @import url('https://fonts.googleapis.com/css2?family=Rajdhani:wght@400;500;600;700&display=swap');

  /* ── Cyber palette ─────────────────────────────────────────────────
     Hot magenta + electric cyan on near-black. Lime for "live" status.
     No copyrighted assets — palette is a 40-year-old genre vocabulary
     (Tron, Akira, Blade Runner). */
  :root {
    --c-bg:        #07070d;
    --c-surface:   #0e0e18;
    --c-surface-2: #15152a;
    --c-line:      #2a2a45;
    --c-text:      #e8e8f0;
    --c-text-mute: #7a7a8a;
    --c-mag:       #ff2cdf;
    --c-mag-soft:  rgba(255, 44, 223, 0.35);
    --c-cy:        #00f0ff;
    --c-cy-soft:   rgba(0, 240, 255, 0.30);
    --c-lime:      #c2ff00;
    --c-amber:     #ffb800;
    --c-red:       #ff3a55;
  }

  body {
    font-family: 'Inter', sans-serif;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    text-rendering: optimizeLegibility;
    background: var(--c-bg);
    color: var(--c-text);
    /* Faint magenta→cyan corner glow + dot grid for night-city depth.
       Both are CSS-only, no images shipped. */
    background-image:
      radial-gradient(ellipse 800px 600px at 100% 0%, rgba(255,44,223,0.08), transparent 60%),
      radial-gradient(ellipse 700px 500px at 0% 100%, rgba(0,240,255,0.06), transparent 60%),
      radial-gradient(rgba(255,255,255,0.025) 1px, transparent 1px);
    background-size: 100% 100%, 100% 100%, 24px 24px;
    background-attachment: fixed;
  }

  /* Subtle CRT scanlines, always on, very low opacity. */
  body::after {
    content: '';
    position: fixed;
    inset: 0;
    pointer-events: none;
    background: linear-gradient(transparent 50%, rgba(0,240,255,0.025) 50%);
    background-size: 100% 3px;
    z-index: 9998;
    mix-blend-mode: screen;
  }

  /* HUD-style display font for headings + brand text. */
  .hud, h1, h2, h3, .top-brand, .breadcrumb {
    font-family: 'Rajdhani', 'Inter', sans-serif;
    letter-spacing: 0.04em;
  }

  /* ── Cards: clipped corners + bracket overlay ──────────────────── */
  .card {
    background: linear-gradient(180deg, var(--c-surface) 0%, var(--c-surface-2) 100%);
    border: 1px solid var(--c-line);
    padding: 1.25rem;
    position: relative;
    /* Notched corners — top-left + bottom-right cut. */
    clip-path: polygon(
      14px 0, 100% 0, 100% calc(100% - 14px),
      calc(100% - 14px) 100%, 0 100%, 0 14px
    );
    border-radius: 0;
  }
  /* Magenta bracket at top-left corner. */
  .card::before {
    content: '';
    position: absolute;
    top: 6px; left: 6px;
    width: 14px; height: 14px;
    border-top: 1px solid var(--c-mag);
    border-left: 1px solid var(--c-mag);
    opacity: 0.7;
    pointer-events: none;
  }
  /* Cyan bracket at bottom-right corner. */
  .card::after {
    content: '';
    position: absolute;
    bottom: 6px; right: 6px;
    width: 14px; height: 14px;
    border-bottom: 1px solid var(--c-cy);
    border-right: 1px solid var(--c-cy);
    opacity: 0.7;
    pointer-events: none;
  }

  /* ── Badges: terminal-style pills ────────────────────────────────── */
  .badge {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border-radius: 0;
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    font-size: 10px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    border: 1px solid currentColor;
  }
  .badge-green  { color: var(--c-lime);  background: rgba(194,255,0,0.08); }
  .badge-yellow { color: var(--c-amber); background: rgba(255,184,0,0.08); }
  .badge-gray   { color: var(--c-text-mute); background: rgba(122,122,138,0.08); }
  .badge-blue   { color: var(--c-cy);    background: rgba(0,240,255,0.08); }

  /* Bridge "live" pill. Glows softly. */
  #media-bridge-pill {
    color: var(--c-lime);
    background: rgba(194,255,0,0.1);
    border: 1px solid var(--c-lime);
    border-radius: 0;
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    padding: 2px 8px;
    box-shadow: 0 0 8px rgba(194,255,0,0.2);
  }

  /* ── Top bar tweaks ──────────────────────────────────────────────── */
  .top-brand { font-size: 1rem; font-weight: 600; letter-spacing: 0.06em; }
  .breadcrumb {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.18em;
    color: var(--c-cy);
  }
  .breadcrumb::before { content: '['; color: var(--c-mag); margin-right: 6px; }
  .breadcrumb::after  { content: ']'; color: var(--c-mag); margin-left:  6px; }

  /* ── Now-playing visualizer ─────────────────────────────────────── */
  /* 8-bar pure-CSS equalizer. We don't have real audio data on this
     page (DRM streams stay client-side), but a subtle motion makes
     the card feel alive without faking precision. Pauses when no
     track is playing via the .viz-paused class toggled by JS. */
  .np-viz {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    height: 28px;
    margin-top: 12px;
    opacity: 0.85;
  }
  .np-viz span {
    width: 4px;
    background: linear-gradient(to top, var(--c-mag) 0%, var(--c-cy) 100%);
    box-shadow: 0 0 4px rgba(255,44,223,0.5);
    transform-origin: bottom;
    animation: viz-bar 1.1s ease-in-out infinite;
  }
  .np-viz.viz-paused span { animation-play-state: paused; transform: scaleY(0.15); }
  .np-viz span:nth-child(1) { animation-delay: -0.0s; }
  .np-viz span:nth-child(2) { animation-delay: -0.2s; }
  .np-viz span:nth-child(3) { animation-delay: -0.4s; }
  .np-viz span:nth-child(4) { animation-delay: -0.6s; }
  .np-viz span:nth-child(5) { animation-delay: -0.1s; }
  .np-viz span:nth-child(6) { animation-delay: -0.3s; }
  .np-viz span:nth-child(7) { animation-delay: -0.5s; }
  .np-viz span:nth-child(8) { animation-delay: -0.7s; }
  @keyframes viz-bar {
    0%, 100% { transform: scaleY(0.25); }
    20%      { transform: scaleY(0.9); }
    40%      { transform: scaleY(0.45); }
    60%      { transform: scaleY(1.0); }
    80%      { transform: scaleY(0.35); }
  }

  /* ── Play / control buttons ──────────────────────────────────────── */
  .ctrl-btn {
    background: var(--c-surface);
    border: 1px solid var(--c-line);
    color: var(--c-text);
    width: 40px; height: 40px;
    border-radius: 0;
    clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px);
    display: flex; align-items: center; justify-content: center;
    transition: all 0.15s;
    cursor: pointer;
  }
  .ctrl-btn:hover {
    background: var(--c-surface-2);
    color: var(--c-cy);
    box-shadow: 0 0 12px var(--c-cy-soft);
  }
  .ctrl-play {
    background: linear-gradient(135deg, var(--c-mag) 0%, #c318a8 100%);
    border: 1px solid var(--c-mag);
    color: white;
    width: 56px; height: 56px;
    clip-path: polygon(10px 0, 100% 0, 100% calc(100% - 10px), calc(100% - 10px) 100%, 0 100%, 0 10px);
    box-shadow: 0 0 16px var(--c-mag-soft), inset 0 0 8px rgba(255,255,255,0.15);
    animation: pulse-glow 2.5s ease-in-out infinite;
  }
  .ctrl-play:hover {
    background: linear-gradient(135deg, #ff5ce6 0%, var(--c-mag) 100%);
    box-shadow: 0 0 24px var(--c-mag-soft), inset 0 0 8px rgba(255,255,255,0.25);
  }
  @keyframes pulse-glow {
    0%, 100% { box-shadow: 0 0 16px var(--c-mag-soft), inset 0 0 8px rgba(255,255,255,0.15); }
    50%      { box-shadow: 0 0 22px rgba(255,44,223,0.55), inset 0 0 8px rgba(255,255,255,0.25); }
  }

  /* ── Album art frame ─────────────────────────────────────────────── */
  #np-art {
    border: 1px solid var(--c-mag);
    border-radius: 0;
    clip-path: polygon(12px 0, 100% 0, 100% calc(100% - 12px), calc(100% - 12px) 100%, 0 100%, 0 12px);
    background: var(--c-surface);
    box-shadow: 0 0 24px rgba(255,44,223,0.15), inset 0 0 24px rgba(0,240,255,0.06);
  }

  /* ── Track title typography ──────────────────────────────────────── */
  #np-song {
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    letter-spacing: 0.02em;
    /* Faint chromatic-aberration hint via stacked text-shadows.
       Subtle so it reads as polish, not glitch. */
    text-shadow:
      -1px 0 0 rgba(255,44,223,0.25),
       1px 0 0 rgba(0,240,255,0.25);
  }

  /* ── Inputs / forms ──────────────────────────────────────────────── */
  input[type="text"], textarea, select {
    background: var(--c-surface) !important;
    border: 1px solid var(--c-line) !important;
    border-radius: 0 !important;
    color: var(--c-text) !important;
    transition: all 0.15s;
  }
  input[type="text"]:focus, textarea:focus, select:focus {
    border-color: var(--c-cy) !important;
    box-shadow: 0 0 8px var(--c-cy-soft) !important;
    outline: none !important;
  }
  /* DJ "Build" + group buttons get the magenta primary look. */
  #dj-run-btn, #group-btn,
  button.bg-oc-600 {
    background: linear-gradient(135deg, var(--c-mag) 0%, #c318a8 100%) !important;
    border: 1px solid var(--c-mag) !important;
    border-radius: 0 !important;
    clip-path: polygon(6px 0, 100% 0, 100% calc(100% - 6px), calc(100% - 6px) 100%, 0 100%, 0 6px);
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    box-shadow: 0 0 10px var(--c-mag-soft);
  }
  #dj-run-btn:hover, #group-btn:hover,
  button.bg-oc-600:hover {
    background: linear-gradient(135deg, #ff5ce6 0%, var(--c-mag) 100%) !important;
    box-shadow: 0 0 16px var(--c-mag-soft);
  }

  /* ── Speaker rows: notched + neon-on-select ──────────────────────── */
  .speaker-card {
    transition: all 0.15s;
    border-radius: 0;
    clip-path: polygon(6px 0, 100% 0, 100% calc(100% - 6px), calc(100% - 6px) 100%, 0 100%, 0 6px);
  }
  .speaker-card.selected {
    border-color: var(--c-cy) !important;
    background: rgba(0,240,255,0.06) !important;
    box-shadow: 0 0 10px var(--c-cy-soft);
  }

  /* ── Section headings inside cards ───────────────────────────────── */
  .card h3 {
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.16em;
    font-size: 0.8rem;
    color: var(--c-text);
  }
  .card h3::before { content: '// '; color: var(--c-mag); opacity: 0.7; }

  /* "Now playing" eyebrow label inside the hero card. */
  .np-eyebrow {
    font-family: 'Rajdhani', sans-serif;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.22em;
    color: var(--c-cy);
  }
  .np-eyebrow::before { content: '> '; color: var(--c-mag); }

  /* ── Volume slider — neon track ──────────────────────────────────── */
  input[type="range"] { accent-color: var(--c-mag); }

  /* ── Native dropdown reskin — notched, neon, custom arrow ──────── */
  select.cyber-select {
    appearance: none;
    -webkit-appearance: none;
    -moz-appearance: none;
    background-color: var(--c-surface) !important;
    border: 1px solid var(--c-line) !important;
    border-radius: 0 !important;
    color: var(--c-text) !important;
    font-family: 'Rajdhani', sans-serif !important;
    font-weight: 600 !important;
    letter-spacing: 0.08em !important;
    text-transform: uppercase !important;
    padding: 4px 22px 4px 10px !important;
    /* Inline SVG cyan caret as the dropdown arrow. */
    background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 10 10' fill='none' stroke='%2300f0ff' stroke-width='1.5'><path d='M2 4l3 3 3-3'/></svg>") !important;
    background-repeat: no-repeat;
    background-position: right 6px center;
    clip-path: polygon(5px 0, 100% 0, 100% calc(100% - 5px), calc(100% - 5px) 100%, 0 100%, 0 5px);
    cursor: pointer;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  select.cyber-select:focus,
  select.cyber-select:hover {
    border-color: var(--c-cy) !important;
    box-shadow: 0 0 6px var(--c-cy-soft) !important;
  }
  /* Dropdown popup itself (browser-controlled, but options inherit colors). */
  select.cyber-select option { background: var(--c-surface); color: var(--c-text); }

  /* ── Checkbox reskin — magenta-notched square with neon glow ───── */
  input[type="checkbox"].cyber-check {
    appearance: none;
    -webkit-appearance: none;
    width: 14px; height: 14px;
    margin: 0;
    background: var(--c-surface);
    border: 1px solid var(--c-line);
    border-radius: 0;
    clip-path: polygon(3px 0, 100% 0, 100% calc(100% - 3px), calc(100% - 3px) 100%, 0 100%, 0 3px);
    cursor: pointer;
    position: relative;
    flex-shrink: 0;
    transition: all 0.15s;
  }
  input[type="checkbox"].cyber-check:hover {
    border-color: var(--c-cy);
    box-shadow: 0 0 4px var(--c-cy-soft);
  }
  input[type="checkbox"].cyber-check:checked {
    background: var(--c-mag);
    border-color: var(--c-mag);
    box-shadow: 0 0 6px var(--c-mag-soft);
  }
  /* Inner ✓ glyph drawn via SVG mask so it stays crisp at any zoom. */
  input[type="checkbox"].cyber-check:checked::after {
    content: '';
    position: absolute;
    inset: 1px;
    background: var(--c-bg);
    clip-path: polygon(13% 48%, 36% 70%, 85% 18%, 92% 27%, 38% 90%, 6% 58%);
  }
  /* Label that wraps the checkbox gets a subtle hover state. */
  label.cyber-check-label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-family: 'Rajdhani', sans-serif;
    font-weight: 500;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    font-size: 11px;
    color: var(--c-text-mute);
    transition: color 0.15s;
  }
  label.cyber-check-label:hover { color: var(--c-text); }
  /* Same Rajdhani treatment for the inline "Tracks:" label next to the select. */
  .cyber-inline-label {
    font-family: 'Rajdhani', sans-serif;
    font-weight: 500;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    font-size: 11px;
    color: var(--c-text-mute);
  }

  .pulse { animation: pulse 2s infinite; }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:.6} }

  /* Marquee for long song names — keep, just rename gradient. */
  @keyframes marquee {
    from { transform: translateX(0); }
    to   { transform: translateX(calc(-100% + 280px)); }
  }
  .marquee-track {
    animation: marquee 12s linear infinite alternate;
    display: inline-block;
    white-space: nowrap;
  }

  /* The right-rail border separator gets a faint cyan glow. */
  .border-r.border-gray-800 { border-right-color: var(--c-line) !important; box-shadow: 1px 0 12px rgba(0,240,255,0.04); }
  .border-b.border-gray-800 { border-bottom-color: var(--c-line) !important; }"##;

const BODY_HTML: &str = r##"<!-- Top bar — matches the dashboard so the music page feels like
     part of Syntaur, not a different app dropped onto the same domain. -->
<div class="border-b border-gray-800 bg-gray-900/50 backdrop-blur sticky top-0 z-40">
  <div class="px-4 py-2.5 flex items-center justify-between">
    <div class="flex items-center gap-3 min-w-0">
      <a href="/" class="flex items-center gap-2 hover:opacity-80 flex-shrink-0">
        <img src="/app-icon.jpg" class="h-8 w-8 rounded-lg" alt="">
        <span class="top-brand">Syntaur</span>
      </a>
      <span class="breadcrumb">Music</span>
      <span id="media-bridge-pill" class="hidden ml-1" title="Local Media Bridge is running — playback bypasses popups">Bridge live</span>
    </div>
    <div class="flex items-center gap-3 text-sm">
      <a href="/" class="text-gray-400 hover:text-white transition-colors">Home</a>
      <a href="/settings" class="text-gray-400 hover:text-gray-300">Settings</a>
      <a href="/profile" class="text-gray-400 hover:text-gray-300" title="Profile">Profile</a>
      <button onclick="refreshAll()" class="text-gray-500 hover:text-gray-300" title="Refresh">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0114.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0020.49 15"/></svg>
      </button>
    </div>
  </div>
</div>

<!-- Hidden player containers — audio plays through browser's audio output -->
<div id="local-players" style="position:fixed; width:0; height:0; overflow:hidden; pointer-events:none;">
  <div id="yt-player-mount"></div>
</div>

<!-- Two-panel layout, matching the dashboard.
     Left  (60%): the listening experience  — hero now-playing + queue
     Right (40%): the controls — provider, AI DJ, speakers, EQ -->
<div class="flex h-[calc(100vh-45px)]">

  <!-- LEFT: Listening (60%) -->
  <div class="w-[60%] border-r border-gray-800 overflow-y-auto px-6 py-6 space-y-4">

    <!-- Hero Now Playing -->
    <div class="card p-6">
      <div class="flex items-start gap-5">
        <div class="w-40 h-40 flex-shrink-0 flex items-center justify-center overflow-hidden" id="np-art">
          <svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" class="text-gray-700"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
        </div>
        <div class="flex-1 min-w-0 flex flex-col">
          <p class="np-eyebrow">Now playing</p>
          <div class="text-2xl mt-1.5 overflow-hidden" id="np-song-wrap">
            <span id="np-song">Nothing playing</span>
          </div>
          <p class="text-sm text-gray-400 mt-1 truncate" id="np-artist">—</p>
          <p class="text-xs text-gray-500 mt-2" id="np-source"></p>
          <!-- Pure-CSS audio visualizer — pauses when nothing is playing
               (JS toggles .viz-paused on the container based on np-play state). -->
          <div class="np-viz viz-paused" id="np-viz" aria-hidden="true">
            <span></span><span></span><span></span><span></span>
            <span></span><span></span><span></span><span></span>
          </div>
          <!-- Controls -->
          <div class="flex items-center gap-3 mt-auto pt-5">
            <button onclick="control('prev')" class="ctrl-btn" title="Previous">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><polygon points="19,20 9,12 19,4"/><rect x="5" y="4" width="2" height="16"/></svg>
            </button>
            <button onclick="control('play_pause')" id="np-play" class="ctrl-play" title="Play/Pause">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>
            </button>
            <button onclick="control('next')" class="ctrl-btn" title="Next">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,4 15,12 5,20"/><rect x="17" y="4" width="2" height="16"/></svg>
            </button>
          </div>
        </div>
      </div>
      <!-- Volume slider (hidden when no playback target supports it) -->
      <div id="volume-row" class="hidden mt-5 pt-4 border-t border-gray-700/50 flex items-center gap-3">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-gray-500 flex-shrink-0"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 010 7.07"/></svg>
        <input type="range" id="volume-slider" min="0" max="100" value="50" class="flex-1 accent-oc-500" oninput="onVolumeChange(this.value)">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-gray-500 flex-shrink-0"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 010 7.07"/><path d="M19.07 4.93a10 10 0 010 14.14"/></svg>
        <span class="text-xs text-gray-400 w-10 text-right" id="volume-label">50%</span>
      </div>
      <!-- Apple Music start button — tertiary, hidden until needed.
           JS toggles the button's own .hidden, so we must NOT wrap it
           in a separately-hidden container or it'll never appear. -->
      <button id="apple-music-start-btn" onclick="startAppleMusicPlayer()" class="hidden mt-3 text-xs text-pink-300 hover:text-pink-200 underline self-start" title="Pre-open the Apple Music player window so subsequent plays autoplay">Start Apple Music player →</button>
    </div>

    <!-- Queue (up next) — only renders when something's in it -->
    <div id="queue-card" class="card hidden">
      <div class="flex items-center justify-between mb-3">
        <h3 class="font-medium text-gray-200 text-sm">Up next</h3>
        <button onclick="clearQueue()" class="text-xs text-gray-500 hover:text-gray-300">Clear</button>
      </div>
      <div id="queue-list" class="space-y-1"></div>
    </div>

    <!-- "How this works" — collapsed by default so it doesn't shout
         at someone who just wants to listen to music. -->
    <details class="px-1">
      <summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-300 py-2 select-none">How playback works</summary>
      <ul class="text-xs text-gray-400 space-y-1.5 leading-relaxed mt-2 pl-2">
        <li><strong class="text-gray-300">Phone (default):</strong> the Syntaur Voice PWA on your phone launches Music.app with the selected track. Audio plays through the phone's output — your speakers, AirPods, or AirPlay target.</li>
        <li><strong class="text-gray-300">iOS Control Center:</strong> to AirPlay to multiple speakers at once, swipe iOS Control Center, hold the music widget, tap each speaker. Zero extra setup.</li>
        <li><strong class="text-gray-300">Home Assistant (optional):</strong> if you run HA, Syntaur can target specific HomePods, Apple TVs, or Sonos directly and group them from this page. Skip if you don't use HA — the phone path covers most cases.</li>
        <li><strong class="text-gray-300">DRM-protected streams:</strong> all major services encrypt audio. The decoder always runs on a licensed client — your phone or a smart speaker. Syntaur orchestrates commands but never touches the encrypted audio itself.</li>
      </ul>
    </details>

  </div><!-- /left -->

  <!-- RIGHT: Controls (40%) -->
  <div class="w-[40%] overflow-y-auto p-4 space-y-4">

    <!-- Music provider chip — small, status-only -->
    <div id="provider-chip" class="card bg-gray-900 border-gray-700 py-3 hidden">
      <div class="flex items-center justify-between gap-3">
        <div class="flex items-center gap-3 flex-1 min-w-0">
          <span class="flex-shrink-0 w-8 h-8 rounded-lg bg-gray-800 flex items-center justify-center" id="provider-icon">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-gray-400"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
          </span>
          <div class="flex-1 min-w-0">
            <p class="text-[10px] uppercase tracking-wider text-gray-500">Provider</p>
            <p class="text-sm font-medium truncate text-gray-200" id="provider-name">—</p>
            <p id="spotify-web-player-badge" class="hidden text-[10px] text-green-400 mt-0.5">Spotify Web Player active on this tab</p>
          </div>
        </div>
        <a href="/settings?tab=sync" class="text-xs text-gray-500 hover:text-gray-200">Change</a>
      </div>
    </div>

    <!-- No provider connected banner — only when relevant -->
    <div id="no-provider-banner" class="hidden card border-yellow-700/50 bg-yellow-900/20">
      <div class="flex items-start gap-3">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-yellow-400 flex-shrink-0 mt-0.5"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
        <div class="flex-1">
          <p class="text-sm font-medium text-yellow-300">No music provider connected</p>
          <p class="text-xs text-gray-400 mt-1">Pick a service in Sync settings — Apple Music, Spotify, YouTube Music, or Tidal — and the DJ, queue, and search all light up.</p>
          <a href="/settings?tab=sync" class="inline-block mt-2 text-xs text-yellow-300 hover:text-yellow-200 underline">Open Sync settings</a>
        </div>
      </div>
    </div>

    <!-- AI DJ -->
    <div class="card">
      <h3 class="font-medium text-gray-200 text-sm">AI DJ</h3>
      <p class="text-xs text-gray-500 mt-0.5 mb-3">Tell me the vibe.</p>
      <div class="flex gap-2">
        <input type="text" id="dj-prompt" placeholder="upbeat 80s synthwave, jazz for studying…" class="flex-1 min-w-0 bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm outline-none focus:border-oc-500" onkeydown="if(event.key==='Enter')runDj()">
        <button onmousedown="startDjStt(event)" onmouseup="stopDjStt(event)" ontouchstart="startDjStt(event)" ontouchend="stopDjStt(event)" id="dj-mic-btn" class="bg-gray-800 hover:bg-gray-700 text-gray-300 px-3 rounded-lg text-sm flex items-center flex-shrink-0" title="Hold to dictate">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
        </button>
        <button onclick="runDj()" id="dj-run-btn" class="bg-oc-600 hover:bg-oc-700 text-white px-4 rounded-lg text-sm font-medium flex-shrink-0">Build</button>
      </div>
      <div class="mt-3 flex items-center gap-4">
        <label class="cyber-check-label">
          <input type="checkbox" id="dj-create-playlist" class="cyber-check"> Save as playlist
        </label>
        <label class="cyber-check-label" style="cursor: default;">
          <span class="cyber-inline-label">Tracks</span>
          <select id="dj-count" class="cyber-select">
            <option value="10">10</option>
            <option value="15" selected>15</option>
            <option value="25">25</option>
          </select>
        </label>
      </div>
      <div id="dj-results" class="mt-4 hidden"></div>
      <div id="dj-feedback" class="hidden mt-3 flex flex-wrap items-center gap-2 pt-3 border-t border-gray-700/50">
        <span class="text-xs text-gray-500">Refine:</span>
        <button onclick="refineDj('more like the liked tracks')" class="text-xs bg-green-900/30 hover:bg-green-900/50 text-green-300 px-2 py-1 rounded">More liked</button>
        <button onclick="refineDj('drop anything resembling the disliked tracks')" class="text-xs bg-red-900/30 hover:bg-red-900/50 text-red-300 px-2 py-1 rounded">Drop disliked</button>
        <button onclick="refineDj('slower, more chill')" class="text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 px-2 py-1 rounded">Chill</button>
        <button onclick="refineDj('faster, more energy')" class="text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 px-2 py-1 rounded">Energy</button>
        <button onclick="refineDj('different genre entirely')" class="text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 px-2 py-1 rounded">Different genre</button>
      </div>
    </div>

    <!-- Speakers -->
    <div class="card">
      <h3 class="font-medium text-gray-200 text-sm">Speakers</h3>
      <p class="text-xs text-gray-500 mt-0.5 mb-3">Where music plays. Phone by default — pick another target below to override.</p>
      <div id="speakers-list" class="space-y-2">
        <p class="text-xs text-gray-500 italic">Loading…</p>
      </div>
      <div id="group-controls" class="hidden mt-4 pt-4 border-t border-gray-700/50">
        <p class="text-xs text-gray-500 mb-2">Selected (<span id="group-count">0</span>) — grouping is Home Assistant only.</p>
        <div class="flex gap-2">
          <button onclick="groupSelected()" id="group-btn" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded-lg">Group</button>
          <button onclick="ungroupSelected()" class="text-xs text-gray-400 hover:text-gray-200">Ungroup</button>
        </div>
      </div>
    </div>

    <!-- Equalizer -->
    <div class="card" id="eq-card">
      <h3 class="font-medium text-gray-200 text-sm">Equalizer</h3>
      <p class="text-xs text-gray-500 mt-0.5 mb-3" id="eq-hint">Pick a sound preset. Available presets depend on your speaker.</p>
      <div id="eq-targets" class="space-y-3">
        <p class="text-xs text-gray-500 italic">Select a speaker above to see its EQ options.</p>
      </div>
      <details class="mt-3 text-xs text-gray-500">
        <summary class="cursor-pointer hover:text-gray-300 select-none">Phone playback EQ</summary>
        <p class="mt-2 text-gray-400 leading-relaxed">EQ is controlled by iOS — open Settings → Music → EQ on your phone to pick a preset. That setting persists across all music playback on the phone.</p>
      </details>
    </div>

  </div><!-- /right -->

</div><!-- /split -->"##;

const MUSIC_JS: &str = r##"const token = sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }

// ── Syntaur Media Bridge (optional local companion) ──────────────────────
// Runs on the user's desktop at 127.0.0.1:18790. When alive, we prefer it
// for apple_music/spotify/tidal/youtube_music playback — avoids the
// popup entirely. Falls back to the existing Web Playback SDK / popup /
// iframe paths below if the bridge isn't installed.
const MEDIA_BRIDGE_URL = 'http://127.0.0.1:18790';
let mediaBridgeAlive = false;
let mediaBridgeAuthed = [];
async function probeMediaBridge() {
  try {
    const r = await fetch(MEDIA_BRIDGE_URL + '/status', { method: 'GET' });
    if (!r.ok) throw new Error('non-2xx');
    const s = await r.json();
    mediaBridgeAlive = true;
    mediaBridgeAuthed = s.authed_providers || [];
    const pill = document.getElementById('media-bridge-pill');
    if (pill) {
      pill.classList.remove('hidden');
      pill.textContent = '⚡ Bridge on';
      pill.title = 'Syntaur Media Bridge v' + s.version + ' — ' + s.audio_backend;
    }
  } catch (e) {
    mediaBridgeAlive = false;
    const pill = document.getElementById('media-bridge-pill');
    if (pill) pill.classList.add('hidden');
  }
}
async function mediaBridgePost(path, body) {
  if (!mediaBridgeAlive) return null;
  try {
    const r = await fetch(MEDIA_BRIDGE_URL + path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body ? JSON.stringify(body) : '{}',
    });
    return await r.json();
  } catch (e) { return null; }
}
// Probe on boot + every 10s. Kicks in silently.
probeMediaBridge();
setInterval(probeMediaBridge, 10000);

const selectedSpeakers = new Set();
let lastNowPlaying = null;
let speakersData = [];
let djLastPrompt = '';
let djLastTracks = [];
const djLikes = new Set();   // track ids the user liked
const djDislikes = new Set(); // track ids the user disliked
const queueTracks = [];

const DJ_PLACEHOLDERS = [
  "e.g. upbeat 80s synthwave",
  "e.g. jazz for studying",
  "e.g. something like Miles Davis but more modern",
  "e.g. chill background music for dinner",
  "e.g. high-energy workout, 130+ BPM",
  "e.g. songs that would be in a Wes Anderson movie",
  "e.g. new indie rock from the last year",
];
(function rotatePlaceholder(){
  const input = document.getElementById('dj-prompt');
  if (!input) return;
  let i = Math.floor(Math.random() * DJ_PLACEHOLDERS.length);
  input.placeholder = DJ_PLACEHOLDERS[i];
  setInterval(() => {
    if (document.activeElement === input || input.value) return;
    i = (i + 1) % DJ_PLACEHOLDERS.length;
    input.placeholder = DJ_PLACEHOLDERS[i];
  }, 5000);
})();

function escapeHtml(s){ if(s===null||s===undefined)return'';return String(s).replace(/[&<>"]/g, c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }

async function authFetch(url, opts) {
  opts = opts || {};
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;
  const resp = await fetch(url, opts);
  if (resp.status === 401) {
    try { sessionStorage.removeItem('syntaur_token'); } catch(e){}
    window.location.href = '/';
    throw new Error('unauthorized');
  }
  return resp;
}

async function loadNowPlaying() {
  try {
    const resp = await authFetch(`/api/music/now_playing?token=${token}`);
    const data = await resp.json();
    lastNowPlaying = data;
    const song = data.song || '';
    const artist = data.artist || '';
    const source = data.source || 'none';
    const state = data.state || 'off';

    const songEl = document.getElementById('np-song');
    songEl.textContent = song || (state === 'off' ? 'Nothing playing' : '—');
    // Apply marquee if song overflows its container
    applyMarquee('np-song-wrap', 'np-song');

    document.getElementById('np-artist').textContent = artist || (data.hint || '—');
    let sourceLine = '';
    if (source === 'phone') sourceLine = `📱 Phone (${data.device || 'My Phone'})`;
    else if (source === 'homepod') sourceLine = `🏠 HomePod`;
    else if (source === 'appletv') sourceLine = `📺 Apple TV`;
    else if (source === 'sonos') sourceLine = `🔊 Sonos`;
    else if (source === 'apple_music_recent') sourceLine = `♫ Last played on Apple Music (not live)`;
    else if (source === 'media_player') sourceLine = `🔊 ${data.device || 'Speaker'}`;
    document.getElementById('np-source').textContent = sourceLine;

    const artEl = document.getElementById('np-art');
    if (data.art_url) {
      artEl.innerHTML = `<img src="${data.art_url.replace('{w}','160').replace('{h}','160')}" class="w-full h-full object-cover" onerror="this.parentElement.innerHTML=\'<span class=\\'text-gray-600\\'>♪</span>\'">`;
    }

    const playBtn = document.getElementById('np-play');
    const viz = document.getElementById('np-viz');
    if (state === 'playing') {
      playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>';
      if (viz) viz.classList.remove('viz-paused');
    } else {
      playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
      if (viz) viz.classList.add('viz-paused');
    }

    // Show volume slider only when HA entity has volume support
    const volRow = document.getElementById('volume-row');
    if (data.entity_id && source !== 'phone' && source !== 'apple_music_recent') {
      volRow.classList.remove('hidden');
    } else {
      volRow.classList.add('hidden');
    }
  } catch(e) { /* silent except 401 */ }
}

function applyMarquee(wrapId, textId) {
  const wrap = document.getElementById(wrapId);
  const text = document.getElementById(textId);
  if (!wrap || !text) return;
  // Reset
  text.classList.remove('marquee-track');
  text.style.display = '';
  // Measure
  setTimeout(() => {
    if (text.scrollWidth > wrap.clientWidth + 4) {
      text.classList.add('marquee-track');
    }
  }, 10);
}

async function control(action) {
  try {
    const entity_id = lastNowPlaying?.entity_id || null;
    const resp = await authFetch('/api/music/control', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, action, entity_id }),
    });
    const data = await resp.json();
    if (data.error) alert(data.error + (data.hint ? '\n' + data.hint : ''));
    setTimeout(loadNowPlaying, 500);
  } catch(e) { if (e.message !== 'unauthorized') alert('Control failed: ' + e.message); }
}

async function onVolumeChange(v) {
  document.getElementById('volume-label').textContent = v + '%';
  if (!lastNowPlaying?.entity_id) return;
  try {
    await authFetch('/api/music/control', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, action: 'volume', entity_id: lastNowPlaying.entity_id, value: parseInt(v)/100 }),
    });
  } catch(e) { /* ignore */ }
}

async function loadSpeakers() {
  const list = document.getElementById('speakers-list');
  try {
    const resp = await authFetch(`/api/music/speakers?token=${token}`);
    const data = await resp.json();
    speakersData = data.speakers || [];
    if (speakersData.length === 0) {
      list.innerHTML = `<p class="text-xs text-gray-500 italic">${escapeHtml(data.note || 'No speakers detected yet.')}</p>`;
      return;
    }
    // Load persisted default target
    const defaultTarget = localStorage.getItem('syntaur_music_target') || '';

    list.innerHTML = speakersData.map(s => {
      const icon = s.kind === 'this_computer' ? '💻' : s.kind === 'phone' ? '📱' : s.kind === 'homepod' ? '🏠' : s.kind === 'appletv' ? '📺' : s.kind === 'sonos' ? '🔊' : s.kind === 'airplay' ? '📡' : '🔉';
      const sid = s.entity_id || s.id;
      const isDefault = defaultTarget === sid;
      const canControl = s.can_control !== false;
      const defaultBadge = isDefault ? '<span class="badge badge-green">Default</span>' : '';
      const controls = canControl
        ? `<button onclick="setDefaultTarget('${escapeHtml(sid)}', '${escapeHtml(s.name)}')" class="text-xs ${isDefault ? 'text-green-400' : 'text-oc-500 hover:text-oc-400'} px-2">${isDefault ? '✓ Default' : 'Set default'}</button>
           <button onclick="selectSpeaker('${escapeHtml(sid)}')" class="text-xs text-gray-400 hover:text-gray-200 px-2">Group-select</button>`
        : '';
      const hint = s.hint ? `<p class="text-[11px] text-gray-600 mt-0.5">${escapeHtml(s.hint)}</p>` : '';
      return `<div class="speaker-card bg-gray-900 border border-gray-700 rounded-lg p-3 flex items-start gap-3" data-speaker-id="${escapeHtml(sid)}">
        <span class="text-xl flex-shrink-0">${icon}</span>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 flex-wrap">
            <p class="text-sm font-medium truncate">${escapeHtml(s.name)}</p>
            ${defaultBadge}
            <span class="badge badge-${s.state === 'playing' ? 'green' : 'gray'}">${escapeHtml(s.state)}</span>
          </div>
          ${hint}
        </div>
        <div class="flex flex-col gap-1">${controls}</div>
      </div>`;
    }).join('');

    if (data.can_group) document.getElementById('group-controls').classList.remove('hidden');
    loadEqOptions();
  } catch(e) {
    if (e.message !== 'unauthorized') list.innerHTML = `<p class="text-xs text-red-400">Load failed: ${e.message}</p>`;
  }
}

async function setDefaultTarget(id, name) {
  localStorage.setItem('syntaur_music_target', id);
  // Also persist server-side so play_music tool can read it
  try {
    await authFetch('/api/music/set_preferred_target', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, entity_id: id, name }),
    });
  } catch(e) { /* server-side is best-effort */ }
  loadSpeakers();
}

function selectSpeaker(id) {
  if (selectedSpeakers.has(id)) selectedSpeakers.delete(id);
  else selectedSpeakers.add(id);
  document.querySelectorAll('.speaker-card').forEach(card => {
    card.classList.toggle('selected', selectedSpeakers.has(card.dataset.speakerId));
  });
  document.getElementById('group-count').textContent = selectedSpeakers.size;
}

async function groupSelected() {
  const ids = [...selectedSpeakers];
  if (ids.length < 2) { alert('Select 2 or more speakers to group.'); return; }
  const [leader, ...members] = ids;
  try {
    const resp = await authFetch('/api/music/group', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, action: 'join', entity_id: leader, group_members: members }),
    });
    if (resp.ok) { alert('Grouped!'); await loadSpeakers(); }
    else alert('Group failed — Home Assistant required for grouping.');
  } catch(e) { if (e.message !== 'unauthorized') alert('Error: ' + e.message); }
}

async function ungroupSelected() {
  for (const id of selectedSpeakers) {
    await authFetch('/api/music/group', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, action: 'unjoin', entity_id: id }),
    }).catch(() => {});
  }
  selectedSpeakers.clear();
  await loadSpeakers();
}

function loadEqOptions() {
  const box = document.getElementById('eq-targets');
  if (!box) return;
  // Show EQ for HA speakers with sound_mode_list
  const eqable = speakersData.filter(s => s.sound_mode_list && Array.isArray(s.sound_mode_list) && s.sound_mode_list.length > 0);
  if (eqable.length === 0) {
    box.innerHTML = '<p class="text-xs text-gray-500 italic">No speakers with EQ presets detected. HomePod and Sonos expose presets via Home Assistant; phone EQ is controlled by iOS Settings → Music.</p>';
    return;
  }
  box.innerHTML = eqable.map(s => {
    const current = s.sound_mode || '';
    const options = s.sound_mode_list.map(m => `<option value="${escapeHtml(m)}"${m === current ? ' selected' : ''}>${escapeHtml(m)}</option>`).join('');
    return `<div class="bg-gray-900 rounded-lg p-3">
      <div class="flex items-center justify-between mb-2">
        <p class="text-sm font-medium">${escapeHtml(s.name)}</p>
        <span class="text-[11px] text-gray-500">current: ${escapeHtml(current || 'default')}</span>
      </div>
      <select onchange="setEq('${escapeHtml(s.entity_id)}', this.value)" class="w-full bg-gray-800 border border-gray-700 rounded px-2 py-1 text-sm">${options}</select>
    </div>`;
  }).join('');
}

async function setEq(entity_id, sound_mode) {
  try {
    const resp = await authFetch('/api/music/eq', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, entity_id, sound_mode }),
    });
    if (resp.ok) alert(`EQ set to ${sound_mode}`);
  } catch(e) { /* ignore */ }
}

// Spotify Web Playback SDK state
let spotifyPlayer = null;
let spotifyDeviceId = null;
let spotifyReady = false;

function loadSpotifySDK() {
  if (window.Spotify || document.getElementById('spotify-sdk-script')) return;
  const s = document.createElement('script');
  s.id = 'spotify-sdk-script';
  s.src = 'https://sdk.scdn.co/spotify-player.js';
  document.head.appendChild(s);
}

window.onSpotifyWebPlaybackSDKReady = function() {
  // Fetch our access token from the gateway (same-origin)
  fetch('/api/music/spotify_token?token=' + token, { headers: { 'Authorization': 'Bearer ' + token } })
    .then(r => r.json())
    .then(data => {
      if (!data.access_token) {
        console.warn('Spotify token unavailable:', data);
        return;
      }
      spotifyPlayer = new Spotify.Player({
        name: 'Syntaur Web Player',
        getOAuthToken: cb => cb(data.access_token),
        volume: 0.7,
      });
      spotifyPlayer.addListener('ready', ({ device_id }) => {
        console.log('[spotify-sdk] ready device', device_id);
        spotifyDeviceId = device_id;
        spotifyReady = true;
        const indicator = document.getElementById('spotify-web-player-badge');
        if (indicator) indicator.classList.remove('hidden');
      });
      spotifyPlayer.addListener('not_ready', ({ device_id }) => {
        console.warn('[spotify-sdk] not ready', device_id);
        spotifyReady = false;
      });
      spotifyPlayer.addListener('initialization_error', ({ message }) => {
        console.warn('[spotify-sdk] init error', message);
      });
      spotifyPlayer.addListener('authentication_error', ({ message }) => {
        console.warn('[spotify-sdk] auth error', message);
        showMusicNotice('Spotify authentication expired. Reconnect in Sync settings.', true);
      });
      spotifyPlayer.addListener('account_error', ({ message }) => {
        console.warn('[spotify-sdk] account error', message, '— needs Premium for web playback');
      });
      spotifyPlayer.connect();
    })
    .catch(e => console.warn('Spotify token fetch failed:', e));
};

function showMusicNotice(msg, persist) {
  let toast = document.getElementById('sync-toast');
  if (!toast) {
    toast = document.createElement('div');
    toast.id = 'sync-toast';
    toast.className = 'fixed bottom-4 right-4 bg-gray-800 border border-gray-700 rounded-lg px-4 py-2 text-sm text-gray-200 shadow-lg z-50';
    document.body.appendChild(toast);
  }
  toast.textContent = msg;
  toast.style.opacity = '1';
  clearTimeout(toast._timer);
  if (!persist) toast._timer = setTimeout(() => { toast.style.opacity = '0'; }, 3500);
}

async function playSpotifyTrack(trackId) {
  const uri = 'spotify:track:' + trackId;
  try {
    const resp = await fetch('/api/music/spotify_play', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        token: token,
        uri: uri,
        device_id: spotifyReady ? spotifyDeviceId : null,
      }),
    });
    const data = await resp.json();
    if (data.error) {
      // Fallback: try to open spotify: URL directly
      if (data.error.includes('No active Spotify device')) {
        showMusicNotice('Opening Spotify app — install Spotify on your phone/desktop for best playback.');
        window.location.href = uri;
      } else {
        showMusicNotice(data.error);
      }
    } else {
      showMusicNotice('Playing on Spotify ✓');
    }
  } catch (e) { showMusicNotice('Play failed: ' + e.message); }
}

async function checkMusicProvider() {
  try {
    const resp = await authFetch(`/api/sync/providers?token=${token}`);
    const data = await resp.json();
    // All music catalog providers in priority order (user's preferred → others)
    const musicIds = ['apple_music', 'spotify', 'youtube_music', 'tidal'];
    const connected = musicIds
      .map(id => (data.providers || []).find(p => p.id === id))
      .filter(p => p && p.connected);

    const chip = document.getElementById('provider-chip');
    const banner = document.getElementById('no-provider-banner');

    if (connected.length === 0) {
      chip.classList.add('hidden');
      banner.classList.remove('hidden');
      window._activeMusicProvider = null;
    } else {
      banner.classList.add('hidden');
      chip.classList.remove('hidden');
      // Pick the first connected provider as active
      const active = connected[0];
      const icons = { apple_music: '🍎', spotify: '🟢', youtube_music: '📺', tidal: '🌊' };
      document.getElementById('provider-icon').textContent = icons[active.id] || '🎵';
      const label = active.display_name ? `${active.name} (${active.display_name})` : active.name;
      document.getElementById('provider-name').textContent = label;
      window._activeMusicProvider = active.id;
      if (active.id === 'spotify' && !window.Spotify) loadSpotifySDK();
      // Show the Apple Music popup-starter button when apple_music is active
      const amBtn = document.getElementById('apple-music-start-btn');
      if (amBtn) amBtn.classList.toggle('hidden', active.id !== 'apple_music');
    }
  } catch(e) { /* ignore */ }
}

async function runDj(overridePrompt) {
  const promptEl = document.getElementById('dj-prompt');
  const prompt = overridePrompt || promptEl.value.trim();
  if (!prompt) return;
  djLastPrompt = prompt;
  const btn = document.getElementById('dj-run-btn');
  btn.textContent = 'Working…'; btn.disabled = true;
  const results = document.getElementById('dj-results');
  results.classList.remove('hidden');
  results.innerHTML = '<p class="text-xs text-gray-500 italic">Picking tracks…</p>';
  try {
    const resp = await authFetch('/api/music/dj', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        token,
        prompt,
        count: parseInt(document.getElementById('dj-count').value),
        create_playlist: document.getElementById('dj-create-playlist').checked,
      }),
    });
    const data = await resp.json();
    if (data.error) {
      results.innerHTML = `<p class="text-xs text-red-400">${escapeHtml(data.error)}</p><p class="text-xs text-gray-500 mt-1">${escapeHtml(data.hint || '')}</p>`;
      return;
    }
    const tracks = data.tracks || [];
    djLastTracks = tracks;
    if (tracks.length === 0) {
      results.innerHTML = '<p class="text-xs text-gray-500">No matches found in the Apple Music catalog for those ideas.</p>';
      return;
    }
    const playlistLine = data.playlist_id ? `<p class="text-xs text-green-400 mb-3">✓ Saved as Apple Music playlist</p>` : '';
    results.innerHTML = playlistLine + '<p class="text-xs text-gray-400 mb-2">Found ' + tracks.length + ' tracks:</p>' + tracks.map(t => renderDjTrack(t)).join('');
    document.getElementById('dj-feedback').classList.remove('hidden');
    // Auto-populate queue
    queueTracks.length = 0;
    tracks.forEach(t => queueTracks.push(t));
    renderQueue();
  } catch(e) {
    if (e.message !== 'unauthorized') results.innerHTML = `<p class="text-xs text-red-400">DJ failed: ${escapeHtml(e.message)}</p>`;
  }
  btn.textContent = 'Build'; btn.disabled = false;
}

function renderDjTrack(t) {
  const art = (t.artwork && typeof t.artwork === 'string') ? t.artwork.replace('{w}','48').replace('{h}','48') : '';
  const id = t.id || '';
  const liked = djLikes.has(id);
  const disliked = djDislikes.has(id);
  const provider = t.provider || window._activeMusicProvider || '';
  let playBtn = '';
  if (provider === 'spotify' && id) {
    playBtn = `<button onclick="playSpotifyTrack('${escapeHtml(id)}')" class="text-xs text-green-400 hover:text-green-300 px-1" title="Play via Spotify Connect">▶</button>`;
  } else if (provider === 'youtube_music' && id) {
    playBtn = `<a href="https://music.youtube.com/watch?v=${escapeHtml(id)}" target="_blank" class="text-xs text-red-400 hover:text-red-300 px-1" title="Open in YouTube Music">▶</a>`;
  } else if (provider === 'apple_music' && id) {
    playBtn = `<a href="music://music.apple.com/us/song/${escapeHtml(id)}" class="text-xs text-pink-400 hover:text-pink-300 px-1" title="Play in Apple Music">▶</a>`;
  }
  return `<div class="flex items-center gap-3 py-2 border-b border-gray-800" data-track-id="${escapeHtml(id)}">
    ${art ? `<img src="${art}" class="w-10 h-10 rounded flex-shrink-0" onerror="this.style.display='none'">` : '<div class="w-10 h-10 bg-gray-800 rounded flex-shrink-0"></div>'}
    <div class="flex-1 min-w-0">
      <p class="text-sm truncate">${escapeHtml(t.name || t.query || '')}</p>
      <p class="text-xs text-gray-500 truncate">${escapeHtml(t.artist || '')}</p>
    </div>
    ${playBtn}
    <button onclick="toggleLike('${escapeHtml(id)}')" class="text-xs ${liked ? 'text-green-400' : 'text-gray-600 hover:text-green-400'} px-1" title="More like this">👍</button>
    <button onclick="toggleDislike('${escapeHtml(id)}')" class="text-xs ${disliked ? 'text-red-400' : 'text-gray-600 hover:text-red-400'} px-1" title="Drop tracks like this">👎</button>
    ${t.url ? `<a href="${escapeHtml(t.url)}" target="_blank" class="text-xs text-oc-500 hover:text-oc-400">Open ↗</a>` : ''}
  </div>`;
}

function toggleLike(id) {
  if (djLikes.has(id)) djLikes.delete(id); else { djLikes.add(id); djDislikes.delete(id); }
  renderDjResultsInPlace();
}
function toggleDislike(id) {
  if (djDislikes.has(id)) djDislikes.delete(id); else { djDislikes.add(id); djLikes.delete(id); }
  renderDjResultsInPlace();
}
function renderDjResultsInPlace() {
  const results = document.getElementById('dj-results');
  if (!results || djLastTracks.length === 0) return;
  const header = `<p class="text-xs text-gray-400 mb-2">Found ${djLastTracks.length} tracks (${djLikes.size} 👍, ${djDislikes.size} 👎):</p>`;
  results.innerHTML = header + djLastTracks.map(t => renderDjTrack(t)).join('');
}

async function refineDj(instruction) {
  if (djLastTracks.length === 0) return;
  const likedNames = djLastTracks.filter(t => djLikes.has(t.id)).map(t => `${t.name} — ${t.artist}`).join('; ');
  const dislikedNames = djLastTracks.filter(t => djDislikes.has(t.id)).map(t => `${t.name} — ${t.artist}`).join('; ');
  let refinedPrompt = djLastPrompt + '. ' + instruction;
  if (likedNames) refinedPrompt += `. The user LIKED: ${likedNames}`;
  if (dislikedNames) refinedPrompt += `. The user DISLIKED: ${dislikedNames}`;
  djLikes.clear(); djDislikes.clear();
  await runDj(refinedPrompt);
}

function renderQueue() {
  const card = document.getElementById('queue-card');
  const list = document.getElementById('queue-list');
  if (queueTracks.length === 0) { card.classList.add('hidden'); return; }
  card.classList.remove('hidden');
  list.innerHTML = queueTracks.map((t, idx) => `<div class="flex items-center gap-2 py-1.5 text-xs border-b border-gray-800" data-queue-idx="${idx}">
    <span class="text-gray-600 w-5 text-right">${idx+1}</span>
    <span class="flex-1 truncate">${escapeHtml(t.name || t.query || '')} <span class="text-gray-500">— ${escapeHtml(t.artist || '')}</span></span>
    <button onclick="moveQueue(${idx}, -1)" class="text-gray-500 hover:text-gray-200 px-1" title="Up">↑</button>
    <button onclick="moveQueue(${idx}, 1)" class="text-gray-500 hover:text-gray-200 px-1" title="Down">↓</button>
    <button onclick="removeFromQueue(${idx})" class="text-gray-500 hover:text-red-400 px-1" title="Remove">×</button>
  </div>`).join('');
}
function moveQueue(idx, delta) {
  const to = idx + delta;
  if (to < 0 || to >= queueTracks.length) return;
  [queueTracks[idx], queueTracks[to]] = [queueTracks[to], queueTracks[idx]];
  renderQueue();
}
function removeFromQueue(idx) {
  queueTracks.splice(idx, 1);
  renderQueue();
}
function clearQueue() {
  queueTracks.length = 0;
  renderQueue();
}

function refreshAll() {
  loadNowPlaying();
  loadSpeakers();
  checkMusicProvider();
}

refreshAll();
setInterval(loadNowPlaying, 5000);


// ── DJ STT ────────────────────────────────────────────────────────────────
let djSttWs = null;
let djMediaRecorder = null;
let djAudioCtx = null;
let djProcessor = null;

async function startDjStt(e) {
  e.preventDefault();
  const btn = document.getElementById('dj-mic-btn');
  btn.classList.add('bg-red-700');
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { sampleRate: 16000, channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    djAudioCtx = new AudioContext({ sampleRate: 16000 });
    const source = djAudioCtx.createMediaStreamSource(stream);
    djProcessor = djAudioCtx.createScriptProcessor(4096, 1, 1);
    const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    djSttWs = new WebSocket(`${wsProto}//${location.host}/ws/stt?token=${token}`);
    djSttWs.binaryType = 'arraybuffer';
    djSttWs.onmessage = (msg) => {
      try {
        const data = JSON.parse(msg.data);
        if (data.text) {
          document.getElementById('dj-prompt').value = data.text;
        }
      } catch(err) {}
    };
    djSttWs.onopen = () => {
      djProcessor.onaudioprocess = (ev) => {
        if (djSttWs.readyState !== 1) return;
        const input = ev.inputBuffer.getChannelData(0);
        const pcm = new Int16Array(input.length);
        for (let i = 0; i < input.length; i++) {
          pcm[i] = Math.max(-32768, Math.min(32767, input[i] * 32768));
        }
        djSttWs.send(pcm.buffer);
      };
      source.connect(djProcessor);
      djProcessor.connect(djAudioCtx.destination);
    };
    djSttWs._stream = stream;
  } catch(err) {
    console.warn('STT start failed:', err);
    btn.classList.remove('bg-red-700');
  }
}

function stopDjStt(e) {
  e.preventDefault();
  const btn = document.getElementById('dj-mic-btn');
  btn.classList.remove('bg-red-700');
  if (djProcessor) { try { djProcessor.disconnect(); } catch{} djProcessor = null; }
  if (djAudioCtx) { try { djAudioCtx.close(); } catch{} djAudioCtx = null; }
  if (djSttWs) {
    try { djSttWs._stream?.getTracks().forEach(t => t.stop()); } catch{}
    try { djSttWs.send(JSON.stringify({type:'eof'})); } catch{}
    setTimeout(() => { try { djSttWs.close(); } catch{} djSttWs = null; }, 200);
  }
}

// ── Music ducking listener (polls duck state, attenuates audio elements) ───
let lastDuckState = false;
async function pollDuckState() {
  try {
    const r = await fetch(`/api/music/duck_state?token=${token}`, { headers: { 'Authorization': 'Bearer ' + token } });
    const d = await r.json();
    const ducking = !!d.ducking;
    if (ducking !== lastDuckState) {
      lastDuckState = ducking;
      // Attenuate any <audio> elements on the page
      document.querySelectorAll('audio').forEach(a => {
        a.volume = ducking ? 0.2 : 1.0;
      });
      // Show/hide duck indicator
      let badge = document.getElementById('duck-badge');
      if (!badge) {
        badge = document.createElement('div');
        badge.id = 'duck-badge';
        badge.style.cssText = 'position:fixed;top:60px;right:16px;background:#1e3a5f;color:#7b9ef0;padding:6px 12px;border-radius:8px;font-size:11px;z-index:100;display:none';
        badge.textContent = '🔉 Music ducked — voice speaking';
        document.body.appendChild(badge);
      }
      badge.style.display = ducking ? 'block' : 'none';
    }
  } catch(e) { /* silent */ }
}
setInterval(pollDuckState, 1500);
pollDuckState();



// ── Local playback (This Computer) ────────────────────────────────────────
// When music plays "on this computer", audio goes through the browser tab's
// audio output (laptop speakers, headphones, whatever the OS is using).
// Ducking is trivial here — we control the players directly. No external
// API round-trips needed.

let ytPlayer = null;
let ytPlayerReady = false;
let ytSdkLoaded = false;
let localEventSource = null;
let localDuckingActive = false;

function loadYtIframeApi() {
  if (ytSdkLoaded) return;
  ytSdkLoaded = true;
  const s = document.createElement('script');
  s.src = 'https://www.youtube.com/iframe_api';
  document.head.appendChild(s);
}

window.onYouTubeIframeAPIReady = function() {
  ytPlayer = new YT.Player('yt-player-mount', {
    height: '1', width: '1',
    playerVars: { autoplay: 0, controls: 0 },
    events: {
      'onReady': () => {
        ytPlayerReady = true;
        console.log('[yt-iframe] ready');
      },
      'onStateChange': (e) => {
        // YT.PlayerState: -1=unstarted, 0=ended, 1=playing, 2=paused, 3=buffering, 5=cued
        if (e.data === 0) console.log('[yt-iframe] track ended');
      },
      'onError': (e) => console.warn('[yt-iframe] error', e.data),
    },
  });
};

async function playOnThisComputer(provider, trackId, uri, name, artist) {
  console.log('[local-play]', provider, trackId, name, '—', artist);
  showMusicNotice('▶ ' + (name || trackId) + (artist ? ' — ' + artist : ''), false);

  // Prefer Syntaur Media Bridge when running — no popup, no SDK auth loop.
  // Bridge drives a hidden Chromium that handles FairPlay/Widevine DRM.
  if (mediaBridgeAlive && ['apple_music','spotify','tidal','youtube_music'].includes(provider)) {
    const bridgeTrackId = (provider === 'spotify' && uri && uri.startsWith('spotify:track:'))
      ? uri.slice('spotify:track:'.length)
      : trackId;
    const res = await mediaBridgePost('/play', {
      provider, track_id: bridgeTrackId, name, artist,
    });
    if (res && res.ok) {
      console.log('[local-play] routed through media bridge');
      return;
    }
    console.warn('[local-play] bridge reachable but play failed, falling back', res);
  }

  if (provider === 'spotify') {
    // Use the Spotify Connect API to play on this tab's registered device
    if (!spotifyDeviceId) {
      // SDK hasn't connected yet — load + connect then retry
      loadSpotifySDK();
      setTimeout(() => playOnThisComputer(provider, trackId, uri, name, artist), 2000);
      return;
    }
    fetch('/api/music/spotify_play', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        token: token,
        uri: uri || ('spotify:track:' + trackId),
        device_id: spotifyDeviceId,
      }),
    }).catch(e => console.warn('[spotify-play]', e));
  } else if (provider === 'youtube_music') {
    if (!ytSdkLoaded) loadYtIframeApi();
    if (ytPlayerReady && ytPlayer) {
      ytPlayer.loadVideoById(trackId);
    } else {
      // Wait for ready
      const waitForYt = setInterval(() => {
        if (ytPlayerReady && ytPlayer) {
          clearInterval(waitForYt);
          ytPlayer.loadVideoById(trackId);
        }
      }, 500);
      setTimeout(() => clearInterval(waitForYt), 15000);
    }
  } else if (provider === 'apple_music') {
    playAppleMusicTrack(trackId, name, artist);
  } else {
    console.warn('[local-play] unknown provider:', provider);
  }
}

function localPauseAll() {
  if (ytPlayerReady && ytPlayer && typeof ytPlayer.pauseVideo === 'function') {
    ytPlayer.pauseVideo();
  }
  if (spotifyPlayer && typeof spotifyPlayer.pause === 'function') {
    spotifyPlayer.pause();
  }
}

function applyLocalDuck(active) {
  localDuckingActive = active;
  const factor = active ? 0.2 : 1.0;
  // Bridge owns Chromium audio — attenuate there too so bridge playback ducks.
  if (mediaBridgeAlive) {
    mediaBridgePost('/duck', { active, level: 0.2 });
  }
  // Spotify Web Playback SDK
  if (spotifyPlayer && typeof spotifyPlayer.setVolume === 'function') {
    spotifyPlayer.setVolume(factor);
  }
  // YouTube IFrame Player
  if (ytPlayerReady && ytPlayer && typeof ytPlayer.setVolume === 'function') {
    ytPlayer.setVolume(Math.round(factor * 100));
  }
  // Any <audio> elements (e.g., TTS playback shouldn't duck itself, but other audio)
  document.querySelectorAll('audio').forEach(a => {
    if (!a.dataset.isTts) a.volume = factor;
  });
}

function startLocalEventStream() {
  if (localEventSource) return;
  const url = '/api/music/local_events?token=' + encodeURIComponent(token);
  localEventSource = new EventSource(url);
  localEventSource.onopen = () => console.log('[local-sse] connected');
  localEventSource.onmessage = (e) => {
    try {
      const ev = JSON.parse(e.data);
      switch (ev.type) {
        case 'connected': break;
        case 'play':
          playOnThisComputer(ev.provider, ev.track_id, ev.uri, ev.name, ev.artist);
          break;
        case 'pause':
          localPauseAll();
          break;
        case 'duck':
          applyLocalDuck(true);
          break;
        case 'unduck':
          applyLocalDuck(false);
          break;
        case 'volume':
          if (typeof ev.volume === 'number') {
            if (spotifyPlayer?.setVolume) spotifyPlayer.setVolume(ev.volume);
            if (ytPlayerReady && ytPlayer?.setVolume) ytPlayer.setVolume(Math.round(ev.volume * 100));
          }
          break;
      }
    } catch(err) { console.warn('[local-sse] bad event', err); }
  };
  localEventSource.onerror = () => {
    console.warn('[local-sse] error, reconnecting in 5s');
    try { localEventSource.close(); } catch(e){}
    localEventSource = null;
    setTimeout(startLocalEventStream, 5000);
  };
}

// Start listening immediately so the gateway sees this tab as "this_computer" available
startLocalEventStream();

// Preload YouTube IFrame API eagerly too (small, cached) so playback is instant
loadYtIframeApi();



// ── Apple Music playback via controlled popup window ───────────────────────
let appleMusicWindow = null;
const APPLE_POPUP_NAME = 'syntaur-apple-music';

function isMacOS() {
  return /Mac|iPhone|iPad|iPod/.test(navigator.platform) || /Macintosh/.test(navigator.userAgent);
}

function playAppleMusicTrack(trackId, name, artist) {
  if (!trackId) { showMusicNotice('No Apple Music track id'); return; }
  const webUrl = 'https://music.apple.com/us/song/' + trackId + '?l=en-US';
  const macAppUrl = 'music://music.apple.com/us/song/' + trackId;
  if (appleMusicWindow && !appleMusicWindow.closed) {
    try {
      appleMusicWindow.location.href = webUrl;
      appleMusicWindow.focus();
      showMusicNotice('\u25b6 ' + (name || 'Apple Music') + (artist ? ' \u2014 ' + artist : ''));
      return;
    } catch(e) { appleMusicWindow = null; }
  }
  if (isMacOS()) {
    const frame = document.createElement('iframe');
    frame.style.display = 'none';
    frame.src = macAppUrl;
    document.body.appendChild(frame);
    setTimeout(() => { try { document.body.removeChild(frame); } catch(e){} }, 1200);
    showMusicNotice('\u25b6 ' + (name || 'Apple Music') + (artist ? ' \u2014 ' + artist : '') + ' (Music.app)');
    return;
  }
  appleMusicWindow = window.open(webUrl, APPLE_POPUP_NAME, 'width=900,height=700');
  if (appleMusicWindow) {
    try { appleMusicWindow.focus(); } catch(e){}
    showMusicNotice('\u25b6 ' + (name || 'Apple Music') + (artist ? ' \u2014 ' + artist : ''));
  } else {
    showMusicNotice('Popup blocked \u2014 click "Start Apple Music player" below, then try again.', true);
  }
}

function startAppleMusicPlayer() {
  if (appleMusicWindow && !appleMusicWindow.closed) {
    appleMusicWindow.focus();
    showMusicNotice('Apple Music player already running');
    return;
  }
  appleMusicWindow = window.open('https://music.apple.com/us/listen-now', APPLE_POPUP_NAME, 'width=900,height=700');
  if (appleMusicWindow) showMusicNotice('Apple Music player opened', false);
  else showMusicNotice('Popup blocked. Allow popups for this site.', true);
}

setInterval(() => { if (appleMusicWindow && appleMusicWindow.closed) appleMusicWindow = null; }, 5000);"##;
