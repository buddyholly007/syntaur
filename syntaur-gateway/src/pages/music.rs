//! /music — music dashboard. Now-playing, AI DJ, queue, speakers, EQ.
//! Migrated from static/music.html. The structural markup and the 36 KB
//! JS block live as raw-string consts — all bytes count as Rust.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, ModuleStatus, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Music",
        authed: false,
        extra_style: Some(EXTRA_STYLE),
    };
    // Bridge-live status is JS-driven; start absent. A small inline script
    // at the end of the page updates it when the local Media Bridge pings in.
    let body = html! {
        (top_bar("Music", None))
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

  /* ── Music sub-bar (below shared top bar) ──────────────────────── */
  .music-subbar {
    display: flex; align-items: center; gap: 10px;
    padding: 6px 18px;
    border-bottom: 1px solid rgb(31,41,55);
    background: rgba(17,24,39,0.4);
    font-size: 12px;
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

  /* ── Now-playing spectrum (canvas, AnalyserNode-driven) ─────────── */
  /* Replaces the old 4×28 px bar strip that was visually invisible.
     Full-width canvas, 64px tall, cyberpunk mag→cyan gradient bars
     drawn 60fps from Web Audio frequency data when local playback is
     active. Falls back to a dim static pattern when nothing is live
     so the space doesn't look broken. */
  .np-spectrum {
    display: block;
    width: 100%;
    height: 64px;
    margin-top: 14px;
    border-radius: 6px;
    background: linear-gradient(180deg, rgba(255,44,223,0.03), rgba(0,255,255,0.02));
    border: 1px solid rgba(255,44,223,0.15);
  }
  /* Progress bar under the title, scrub to seek. */
  .np-progress-row {
    display: flex; align-items: center; gap: 10px;
    margin-top: 10px;
  }
  .np-time {
    font-size: 11px; color: var(--c-text-dim, #8a94a3);
    font-variant-numeric: tabular-nums;
    min-width: 36px; text-align: center;
  }
  .np-progress {
    flex: 1; appearance: none; height: 4px; border-radius: 2px;
    background: linear-gradient(to right, var(--c-mag) var(--progress, 0%), rgba(255,44,223,0.2) var(--progress, 0%));
    outline: none; cursor: pointer;
  }
  .np-progress::-webkit-slider-thumb {
    appearance: none; width: 12px; height: 12px; border-radius: 50%;
    background: var(--c-cy); box-shadow: 0 0 8px var(--c-cy); cursor: pointer;
  }
  .np-progress::-moz-range-thumb {
    width: 12px; height: 12px; border-radius: 50%;
    background: var(--c-cy); border: none; box-shadow: 0 0 8px var(--c-cy); cursor: pointer;
  }
  /* Love / shuffle / repeat active state */
  .ctrl-btn.active { color: var(--c-mag); border-color: rgba(255,44,223,0.6); }

  /* Library tab pills above the track list */
  .lib-tabs { display: flex; gap: 6px; margin: 10px 0 8px; flex-wrap: wrap; }
  .lib-tab {
    padding: 4px 12px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em;
    color: var(--c-text-dim, #9ba6b6); background: transparent; border: 1px solid var(--c-line, #2a3340);
    border-radius: 999px; cursor: pointer; font-family: inherit;
  }
  .lib-tab.active { color: var(--c-mag); border-color: rgba(255,44,223,0.5); background: rgba(255,44,223,0.08); }

  /* Album grid */
  .alb-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 12px; }
  .alb-tile { background: rgba(0,0,0,0.15); border: 1px solid var(--c-line, #2a3340); border-radius: 6px; padding: 8px; cursor: pointer; transition: border-color 0.15s; }
  .alb-tile:hover { border-color: var(--c-mag); }
  .alb-art { width: 100%; aspect-ratio: 1/1; background: #0b0f17 url('data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="%23333" d="M9 18V5l12-2v13"/></svg>') center/40% no-repeat; border-radius: 4px; overflow: hidden; }
  .alb-art img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .alb-name { font-size: 12px; color: #e0e5ec; margin-top: 6px; line-height: 1.3; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .alb-artist { font-size: 10.5px; color: #8a94a3; margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Row-thumbnail album art (track list) */
  .row-art {
    width: 28px; height: 28px; flex-shrink: 0; border-radius: 3px;
    background: #0b0f17 center/cover no-repeat; display: block;
  }
  .row-art.placeholder::after {
    content: '♪'; display: flex; align-items: center; justify-content: center;
    width: 100%; height: 100%; color: #3a4250; font-size: 14px;
  }

  /* Artist row */
  .artist-row { padding: 8px 10px; border-bottom: 1px dashed rgba(255,255,255,0.06); cursor: pointer; display: flex; align-items: center; justify-content: space-between; }
  .artist-row:hover { background: rgba(255,255,255,0.03); }
  .artist-name { font-size: 13px; color: #e0e5ec; }
  .artist-count { font-size: 11px; color: #8a94a3; }
  /* Duplicate group */
  .dup-row { padding: 8px 10px; border: 1px solid var(--c-line, #2a3340); border-radius: 6px; margin-bottom: 6px; }
  .dup-meta { font-size: 11px; color: #8a94a3; margin-top: 2px; }

  /* Lyrics panel (shows under Details modal) */
  .lyrics-scroll { max-height: 240px; overflow-y: auto; font-size: 13px; line-height: 1.8; color: #c2cbd6; padding: 8px; background: rgba(0,0,0,0.2); border-radius: 4px; }
  .lyrics-scroll .line.active { color: var(--c-mag); font-weight: 500; }

  /* Natural-language search bar atop the library */
  .nl-search {
    display: flex; gap: 8px; padding: 10px; background: rgba(255,44,223,0.05);
    border: 1px solid rgba(255,44,223,0.2); border-radius: 6px; margin-bottom: 10px;
  }
  .nl-search input {
    flex: 1; background: transparent; color: #e0e5ec; border: none; outline: none;
    font-size: 13px; font-family: inherit;
  }
  .nl-search input::placeholder { color: #6a7380; font-style: italic; }
  .nl-search button { background: var(--c-mag); color: #0a0a0a; border: none; padding: 4px 12px; border-radius: 4px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; cursor: pointer; }

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

  /* ── AI DJ chat thread ────────────────────────────────────────────
     Conversation transcript above the prompt input. User turns are
     small magenta-bordered bubbles (right-aligned), DJ turns are
     wider cyan-accented blocks containing the actual track list. */
  .dj-thread {
    max-height: 420px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 6px 4px;
    scrollbar-width: thin;
    scrollbar-color: var(--c-line) transparent;
  }
  .dj-thread::-webkit-scrollbar { width: 6px; }
  .dj-thread::-webkit-scrollbar-track { background: transparent; }
  .dj-thread::-webkit-scrollbar-thumb { background: var(--c-line); }
  .dj-thread::-webkit-scrollbar-thumb:hover { background: var(--c-mag); }

  .dj-turn-user {
    align-self: flex-end;
    max-width: 85%;
    background: linear-gradient(135deg, rgba(255,44,223,0.18) 0%, rgba(255,44,223,0.05) 100%);
    border: 1px solid rgba(255,44,223,0.45);
    padding: 6px 10px;
    clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px);
  }
  .dj-turn-dj {
    align-self: stretch;
    background: rgba(0,240,255,0.04);
    border: 1px solid rgba(0,240,255,0.28);
    padding: 8px 10px;
    clip-path: polygon(8px 0, 100% 0, 100% calc(100% - 8px), calc(100% - 8px) 100%, 0 100%, 0 8px);
  }
  .dj-turn-label {
    font-family: 'Rajdhani', sans-serif;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.22em;
    color: var(--c-mag);
    display: inline-block;
    margin-bottom: 2px;
  }
  .dj-turn-dj .dj-turn-label { color: var(--c-cy); }
  .dj-turn-prompt { font-size: 12px; color: var(--c-text); line-height: 1.4; }
  .dj-turn-summary { font-size: 11px; color: var(--c-text-mute); margin-top: 2px; }
  .dj-turn-tracks { margin-top: 6px; max-height: 240px; overflow-y: auto; }
  .dj-turn-tracks::-webkit-scrollbar { width: 4px; }
  .dj-turn-tracks::-webkit-scrollbar-thumb { background: var(--c-line); }
  .dj-refine-bar {
    margin-top: 8px;
    padding-top: 6px;
    border-top: 1px solid rgba(0,240,255,0.15);
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .dj-refine-bar > .dj-refine-label {
    font-family: 'Rajdhani', sans-serif;
    font-size: 9px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--c-text-mute);
  }
  .dj-refine-btn {
    font-family: 'Rajdhani', sans-serif;
    font-weight: 600;
    font-size: 10px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    padding: 3px 8px;
    border: 1px solid var(--c-line);
    background: var(--c-surface);
    color: var(--c-text-mute);
    clip-path: polygon(4px 0, 100% 0, 100% calc(100% - 4px), calc(100% - 4px) 100%, 0 100%, 0 4px);
    cursor: pointer;
    transition: all 0.15s;
  }
  .dj-refine-btn:hover { color: var(--c-cy); border-color: var(--c-cy); box-shadow: 0 0 6px var(--c-cy-soft); }
  .dj-refine-btn.like   { color: var(--c-lime); border-color: rgba(194,255,0,0.35); }
  .dj-refine-btn.like:hover   { box-shadow: 0 0 6px rgba(194,255,0,0.3); }
  .dj-refine-btn.dislike { color: var(--c-red);  border-color: rgba(255,58,85,0.35); }
  .dj-refine-btn.dislike:hover { box-shadow: 0 0 6px rgba(255,58,85,0.3); }

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
    /* !important needed on every longhand because the generic select
       rule above uses the background: shorthand, which forces every
       background-* property to !important. Without these the arrow
       SVG tiles across the whole control. */
    background-repeat: no-repeat !important;
    background-position: right 6px center !important;
    background-size: 10px 10px !important;
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

const BODY_HTML: &str = r##"<!-- Module sub-bar — "Bridge live" indicator + refresh. Global nav
     now lives in the shared top bar rendered above this HTML. -->
<div class="music-subbar">
  <span id="media-bridge-pill" class="hidden" title="Local Media Bridge is running — playback bypasses popups">Bridge live</span>
  <div style="flex:1"></div>
  <button onclick="refreshAll()" class="text-gray-500 hover:text-gray-300" title="Refresh">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0114.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0020.49 15"/></svg>
  </button>
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
        <div class="w-40 h-40 flex-shrink-0 flex items-center justify-center overflow-hidden relative rounded-md bg-gray-900/60" id="np-art">
          <svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" class="text-gray-700"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
        </div>
        <div class="flex-1 min-w-0 flex flex-col">
          <p class="np-eyebrow">Now playing</p>
          <div class="text-2xl mt-1.5 overflow-hidden" id="np-song-wrap">
            <span id="np-song">Nothing playing</span>
          </div>
          <p class="text-sm text-gray-400 mt-1 truncate" id="np-artist">—</p>
          <div class="flex items-center gap-2 mt-2">
            <p class="text-xs text-gray-500" id="np-source"></p>
            <span id="np-format-badge" class="hidden text-[10px] tracking-wider uppercase px-1.5 py-0.5 rounded border border-cyan-700/60 text-cyan-400 bg-cyan-950/40 font-mono"></span>
          </div>
          <!-- Audio-reactive spectrum. Replaces the previous 4px-bar
               viz with a canvas that fills the remaining width and is
               64px tall; drawn from an AnalyserNode 60fps while local
               audio plays, dark fallback while nothing plays. -->
          <canvas id="np-spectrum" class="np-spectrum" aria-hidden="true" width="800" height="64"></canvas>
          <!-- Progress bar — clickable to seek, currentTime / duration. -->
          <div class="np-progress-row" id="np-progress-row">
            <span id="np-time-cur" class="np-time">0:00</span>
            <input id="np-progress" type="range" min="0" max="1000" value="0" step="1" class="np-progress">
            <span id="np-time-dur" class="np-time">0:00</span>
          </div>
          <!-- Controls -->
          <div class="flex items-center gap-3 mt-auto pt-3">
            <button onclick="control('prev')" class="ctrl-btn" title="Previous (Shift+Left)">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><polygon points="19,20 9,12 19,4"/><rect x="5" y="4" width="2" height="16"/></svg>
            </button>
            <button onclick="control('play_pause')" id="np-play" class="ctrl-play" title="Play/Pause (Space)">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>
            </button>
            <button onclick="control('next')" class="ctrl-btn" title="Next (Shift+Right)">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,4 15,12 5,20"/><rect x="17" y="4" width="2" height="16"/></svg>
            </button>
            <button onclick="toggleShuffle()" id="np-shuffle" class="ctrl-btn" title="Shuffle (S)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 3 21 3 21 8"/><line x1="4" y1="20" x2="21" y2="3"/><polyline points="21 16 21 21 16 21"/><line x1="15" y1="15" x2="21" y2="21"/><line x1="4" y1="4" x2="9" y2="9"/></svg>
            </button>
            <button onclick="toggleRepeat()" id="np-repeat" class="ctrl-btn" title="Repeat (R)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="17 1 21 5 17 9"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><polyline points="7 23 3 19 7 15"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>
            </button>
            <button onclick="toggleFavorite()" id="np-favorite" class="ctrl-btn" title="Love (L)">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/></svg>
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

    <!-- No provider connected banner — small, informational.
         Smaller footprint than before; local library is now a first-class
         alternative, so "no provider" isn't a dead-end state anymore. -->
    <div id="no-provider-banner" class="hidden card border-gray-700 bg-gray-900 py-2.5">
      <div class="flex items-start gap-2">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-gray-500 flex-shrink-0 mt-0.5"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
        <p class="text-xs text-gray-400 flex-1">
          No streaming provider connected.
          <a href="/settings?tab=sync" class="text-oc-500 hover:text-oc-400 underline">Connect one</a>
          or use your local library below.
        </p>
      </div>
    </div>

    <!-- Local library — add folders on the gateway host, scan for audio,
         play tracks through the browser's <audio> element. Works without
         any streaming provider. -->
    <div class="card" id="local-library-card">
      <div class="flex items-center justify-between">
        <div>
          <h3 class="font-medium text-gray-200 text-sm">Local library</h3>
          <p id="local-lib-summary" class="text-xs text-gray-500 mt-0.5">Point at a folder of audio files on this host.</p>
        </div>
        <div class="flex items-center gap-3">
          <button id="local-lib-cleanup" onclick="cleanUpTags()" title="Find tracks with missing or messy tags and let the AI clean them up" class="text-[10px] text-gray-500 hover:text-oc-400 font-mono uppercase tracking-wider hidden">Clean up tags</button>
          <button id="local-lib-scan" onclick="scanLocalLibrary()" class="text-[10px] text-gray-500 hover:text-oc-400 font-mono uppercase tracking-wider hidden">Rescan</button>
        </div>
      </div>

      <!-- Folder list -->
      <div id="local-lib-folders" class="mt-3 space-y-1"></div>

      <!-- Add folder row — browser-first, manual entry is secondary -->
      <div class="flex gap-2 mt-3">
        <button onclick="openFolderPicker()"
          class="flex-1 min-w-0 bg-oc-600 hover:bg-oc-700 text-white px-4 py-2 rounded-lg text-sm font-medium flex items-center justify-center gap-2">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
          </svg>
          Browse for folder
        </button>
        <button onclick="toggleManualPathEntry()" title="Type a path manually"
          class="bg-gray-800 hover:bg-gray-700 text-gray-300 px-3 rounded-lg text-sm font-medium flex-shrink-0">…</button>
      </div>
      <div id="local-lib-manual-row" class="flex gap-2 mt-2 hidden">
        <input type="text" id="local-lib-path" placeholder="/home/sean/Music  or  ~/Music"
          class="flex-1 min-w-0 bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 placeholder-gray-500 outline-none focus:border-oc-500"
          onkeydown="if(event.key==='Enter')addLocalFolder()">
        <button onclick="addLocalFolder()" class="bg-oc-600 hover:bg-oc-700 text-white px-3 rounded-lg text-sm font-medium flex-shrink-0">Add</button>
      </div>
      <p id="local-lib-error" class="text-xs text-red-400 mt-2 hidden"></p>

      <!-- Folder picker modal -->
      <!-- Folder picker modal.
           Inline CSS rather than Tailwind utilities for the sizing math so
           it renders correctly in WebKitGTK (Gaming PC viewer) where some
           Tailwind arbitrary values behave inconsistently. -->
      <div id="fs-picker-modal" class="hidden" style="position:fixed;inset:0;z-index:9999;display:none;align-items:center;justify-content:center;background:rgba(0,0,0,0.7);padding:16px;box-sizing:border-box;">
        <div style="background:#111827;border:1px solid #374151;border-radius:12px;width:100%;max-width:640px;height:min(85vh, 640px);display:flex;flex-direction:column;overflow:hidden;box-sizing:border-box;">
          <div style="padding:16px;border-bottom:1px solid #1f2937;display:flex;align-items:center;gap:12px;flex-shrink:0;">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="color:#38bdf8;flex-shrink:0;">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
            </svg>
            <h3 style="font-weight:500;color:#e5e7eb;flex:1;margin:0;">Pick a music folder</h3>
            <button onclick="closeFolderPicker()" style="color:#6b7280;background:none;border:none;font-size:22px;line-height:1;cursor:pointer;padding:0 6px;">&times;</button>
          </div>
          <div style="padding:12px;border-bottom:1px solid #1f2937;display:flex;align-items:center;gap:8px;font-size:12px;flex-shrink:0;">
            <button onclick="fsPickerGoUp()" id="fs-picker-up" style="color:#9ca3af;background:none;border:none;padding:4px 8px;border-radius:4px;cursor:pointer;">&#8593; Up</button>
            <span id="fs-picker-breadcrumb" style="color:#d1d5db;font-family:monospace;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;">Loading&hellip;</span>
          </div>
          <div style="flex:1;display:flex;min-height:0;overflow:hidden;">
            <!-- Shortcuts sidebar -->
            <div id="fs-picker-roots" style="width:176px;border-right:1px solid #1f2937;padding:8px;flex-shrink:0;background:rgba(3,7,18,0.5);overflow-y:auto;"></div>
            <!-- Entry list -->
            <div id="fs-picker-entries" style="flex:1;padding:8px;overflow-y:auto;min-width:0;">
              <p style="font-size:12px;color:#6b7280;font-style:italic;padding:12px;">Loading&hellip;</p>
            </div>
          </div>
          <div style="padding:12px;border-top:1px solid #1f2937;display:flex;align-items:center;justify-content:space-between;gap:12px;flex-shrink:0;">
            <div id="fs-picker-hint" style="font-size:12px;color:#6b7280;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;">Pick a folder, or click "Select this folder" to use the current one.</div>
            <button onclick="closeFolderPicker()" style="color:#9ca3af;background:none;border:none;padding:6px 12px;font-size:13px;cursor:pointer;">Cancel</button>
            <button id="fs-picker-select" onclick="fsPickerSelectCurrent()" disabled style="background:#0284c7;color:#fff;padding:6px 16px;border:none;border-radius:8px;font-size:13px;font-weight:500;cursor:pointer;">Select this folder</button>
          </div>
        </div>
      </div>

      <!-- Natural-language search bar — asks the LLM to translate a
           plain-English query ("post-rock from my library I played
           last winter") into a filter and returns matching tracks. -->
      <div class="nl-search mt-3">
        <input type="text" id="local-lib-nl" placeholder="Ask for anything — try: 'jazz I haven't heard recently'"
          onkeydown="if(event.key==='Enter') runNLSearch()">
        <button onclick="runNLSearch()">Ask</button>
      </div>

      <!-- Filter (exact-match) + library tabs -->
      <div class="mt-3 flex gap-2">
        <input type="text" id="local-lib-search" placeholder="filter: artist, album, or title…"
          class="flex-1 min-w-0 bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-xs text-gray-200 placeholder-gray-500 outline-none focus:border-oc-500"
          oninput="debouncedLocalSearch()">
      </div>

      <div class="lib-tabs" id="lib-tabs">
        <button class="lib-tab active" data-view="tracks" onclick="switchLibView('tracks')">Tracks</button>
        <button class="lib-tab" data-view="albums" onclick="switchLibView('albums')">Albums</button>
        <button class="lib-tab" data-view="artists" onclick="switchLibView('artists')">Artists</button>
        <button class="lib-tab" data-view="favorites" onclick="switchLibView('favorites')">Favorites</button>
        <button class="lib-tab" data-view="recent" onclick="switchLibView('recent')">Recent</button>
        <button class="lib-tab" data-view="playlists" onclick="switchLibView('playlists')">Playlists</button>
        <button class="lib-tab" data-view="duplicates" onclick="switchLibView('duplicates')">Duplicates</button>
      </div>

      <!-- Track list / active-view container -->
      <div id="local-lib-tracks" class="mt-1 max-h-80 overflow-y-auto border-t border-gray-800 pt-2 text-xs"></div>

      <!-- Hidden <audio> used for playback of local files -->
      <audio id="local-audio" style="display:none" preload="none"></audio>
    </div>

    <!-- Local track details drawer — MusicBrainz lookup + manual edit.
         Hidden by default; openLocalDetails(trackId) pops it open. -->
    <div id="local-details-modal" class="hidden" style="position:fixed;inset:0;z-index:9998;display:none;align-items:center;justify-content:center;background:rgba(0,0,0,0.7);padding:16px;box-sizing:border-box;">
      <div style="background:#111827;border:1px solid #374151;border-radius:12px;width:100%;max-width:560px;max-height:85vh;display:flex;flex-direction:column;overflow:hidden;box-sizing:border-box;">
        <div style="padding:14px 16px;border-bottom:1px solid #1f2937;display:flex;align-items:center;gap:12px;flex-shrink:0;">
          <h3 style="font-weight:500;color:#e5e7eb;flex:1;margin:0;font-size:14px;">Track details</h3>
          <button onclick="closeLocalDetails()" style="color:#6b7280;background:none;border:none;font-size:22px;line-height:1;cursor:pointer;padding:0 6px;">&times;</button>
        </div>
        <div id="local-details-body" style="flex:1;overflow-y:auto;padding:16px;"></div>
      </div>
    </div>

    <!-- AI DJ — chat-style transcript above input. Each turn = your prompt
         (magenta bubble) + DJ's set (cyan-accented track list). Persists
         across reloads via localStorage. -->
    <div class="card">
      <div class="flex items-center justify-between">
        <div>
          <h3 class="font-medium text-gray-200 text-sm">AI DJ</h3>
          <p class="text-xs text-gray-500 mt-0.5">Tell me the vibe.</p>
        </div>
        <button id="dj-clear-thread" onclick="clearDjThread()" class="hidden text-[10px] text-gray-500 hover:text-red-400 font-mono uppercase tracking-wider">Clear thread</button>
      </div>

      <!-- Conversation transcript -->
      <div id="dj-thread" class="dj-thread mt-3">
        <p class="text-xs text-gray-600 italic px-1 py-2">// no sessions yet — drop a vibe below</p>
      </div>

      <!-- Compose row -->
      <div class="flex gap-2 mt-2">
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

      <!-- Legacy stub kept hidden so any unmodified handlers still find it. -->
      <div id="dj-results" class="hidden"></div>
      <div id="dj-feedback" class="hidden"></div>
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
  // Local playback owns the Now Playing card while it's active — don't
  // let the server poll overwrite what setLocalNowPlaying just wrote.
  if (localPlaybackActive) return;
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
  // If local audio is active, drive it directly instead of asking the
  // server to command a cloud player.
  if (localPlaybackActive) {
    const a = document.getElementById('local-audio');
    if (action === 'play_pause' || action === 'pause' || action === 'play') {
      if (a) {
        if (a.paused) { try { await a.play(); } catch(e) { console.warn('[local-control] play', e); } }
        else { try { a.pause(); } catch(e) {} }
      }
      return;
    }
    if (action === 'next') { playRelativeTo(1); return; }
    if (action === 'prev') {
      if (a && a.currentTime > 3) { a.currentTime = 0; return; }
      playRelativeTo(-1); return;
    }
  }
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

// ── DJ chat thread ──────────────────────────────────────────────────
// Persistent transcript stored in localStorage. Each turn:
//   { role: 'user', text, ts }
//   { role: 'dj',   prompt, tracks, playlist_id?, error?, ts, likes:[], dislikes:[] }
// Most-recent DJ turn is the "active" one — its tracks drive djLastTracks
// (so the queue auto-populate logic still works) and its likes/dislikes
// feed the refinement.
let djThread = [];
const DJ_THREAD_KEY = 'dj_thread_v1';
const DJ_THREAD_CAP = 30;

function loadDjThread() {
  try {
    djThread = JSON.parse(localStorage.getItem(DJ_THREAD_KEY) || '[]');
    if (!Array.isArray(djThread)) djThread = [];
  } catch (e) { djThread = []; }
  // Restore djLastPrompt + djLastTracks from the latest DJ turn so
  // refineDj has something to chain off of after a page reload.
  const lastDj = [...djThread].reverse().find(t => t.role === 'dj' && Array.isArray(t.tracks));
  if (lastDj) {
    djLastPrompt = lastDj.prompt || '';
    djLastTracks = lastDj.tracks || [];
    (lastDj.likes || []).forEach(id => djLikes.add(id));
    (lastDj.dislikes || []).forEach(id => djDislikes.add(id));
  }
}
function saveDjThread() {
  // Cap to last N turns so localStorage doesn't grow unbounded.
  if (djThread.length > DJ_THREAD_CAP) {
    djThread = djThread.slice(-DJ_THREAD_CAP);
  }
  // Mirror the in-memory like/dislike sets onto the active DJ turn so
  // they survive reload.
  const activeIdx = findActiveDjTurnIdx();
  if (activeIdx !== -1) {
    djThread[activeIdx].likes = [...djLikes];
    djThread[activeIdx].dislikes = [...djDislikes];
  }
  try { localStorage.setItem(DJ_THREAD_KEY, JSON.stringify(djThread)); } catch (e) {}
}
function findActiveDjTurnIdx() {
  for (let i = djThread.length - 1; i >= 0; i--) {
    if (djThread[i].role === 'dj' && Array.isArray(djThread[i].tracks)) return i;
  }
  return -1;
}
function clearDjThread() {
  if (djThread.length > 0 && !confirm('Clear the entire DJ thread?')) return;
  djThread = [];
  djLastPrompt = '';
  djLastTracks = [];
  djLikes.clear();
  djDislikes.clear();
  saveDjThread();
  renderDjThread();
}

function renderDjThread() {
  const container = document.getElementById('dj-thread');
  const clearBtn  = document.getElementById('dj-clear-thread');
  if (!container) return;
  if (djThread.length === 0) {
    container.innerHTML = '<p class="text-xs text-gray-600 italic px-1 py-2">// no sessions yet — drop a vibe below</p>';
    if (clearBtn) clearBtn.classList.add('hidden');
    return;
  }
  if (clearBtn) clearBtn.classList.remove('hidden');
  const activeIdx = findActiveDjTurnIdx();
  container.innerHTML = djThread.map((turn, i) => renderDjTurn(turn, i, i === activeIdx)).join('');
  // Scroll to the bottom so the most recent turn is visible.
  requestAnimationFrame(() => { container.scrollTop = container.scrollHeight; });
}

function renderDjTurn(turn, idx, isActive) {
  if (turn.role === 'user') {
    return '<div class="dj-turn-user">' +
      '<span class="dj-turn-label">You</span>' +
      '<div class="dj-turn-prompt">' + escapeHtml(turn.text || '') + '</div>' +
    '</div>';
  }
  if (turn.role === 'dj') {
    if (turn.error) {
      return '<div class="dj-turn-dj">' +
        '<span class="dj-turn-label">DJ</span>' +
        '<div class="dj-turn-prompt"><span style="color:var(--c-red)">' + escapeHtml(turn.error) + '</span></div>' +
      '</div>';
    }
    const tracks = turn.tracks || [];
    const tracksHtml = tracks.length === 0
      ? '<p class="text-xs text-gray-500 italic">No matches in catalog.</p>'
      : tracks.map(t => renderDjTrack(t)).join('');
    const summary = (turn.playlist_id ? '<span style="color:var(--c-lime)">▸ Saved as playlist</span> · ' : '') +
                    tracks.length + ' tracks';
    const refineBar = isActive && tracks.length > 0 ? djRefineBarHtml() : '';
    return '<div class="dj-turn-dj">' +
      '<span class="dj-turn-label">DJ</span>' +
      '<div class="dj-turn-summary">' + summary + '</div>' +
      '<div class="dj-turn-tracks">' + tracksHtml + '</div>' +
      refineBar +
    '</div>';
  }
  return '';
}

function djRefineBarHtml() {
  return '<div class="dj-refine-bar">' +
    '<span class="dj-refine-label">Refine</span>' +
    '<button class="dj-refine-btn like" onclick="refineDj(\'more like the liked tracks\')">More liked</button>' +
    '<button class="dj-refine-btn dislike" onclick="refineDj(\'drop anything resembling the disliked tracks\')">Drop disliked</button>' +
    '<button class="dj-refine-btn" onclick="refineDj(\'slower, more chill\')">Chill</button>' +
    '<button class="dj-refine-btn" onclick="refineDj(\'faster, more energy\')">Energy</button>' +
    '<button class="dj-refine-btn" onclick="refineDj(\'different genre entirely\')">Different genre</button>' +
  '</div>';
}

async function runDj(overridePrompt, displayText) {
  const promptEl = document.getElementById('dj-prompt');
  const prompt = overridePrompt || promptEl.value.trim();
  if (!prompt) return;
  djLastPrompt = prompt;
  // New turn for this prompt — clear previous likes/dislikes so the next
  // refinement starts fresh against THIS set's reactions.
  djLikes.clear();
  djDislikes.clear();
  djThread.push({ role: 'user', text: displayText || prompt, ts: Date.now() });
  // Placeholder DJ turn ("picking tracks…") so the user sees activity
  // immediately and the spinner lives inline with the conversation.
  djThread.push({ role: 'dj', prompt, tracks: null, pending: true, ts: Date.now() });
  saveDjThread();
  renderDjPending();
  promptEl.value = '';

  const btn = document.getElementById('dj-run-btn');
  btn.textContent = 'Working…'; btn.disabled = true;
  let result = null;
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
    result = await resp.json();
  } catch (e) {
    if (e.message === 'unauthorized') return;
    result = { error: 'DJ failed: ' + e.message };
  }
  btn.textContent = 'Build'; btn.disabled = false;

  // Replace the pending placeholder with the final turn.
  const pendingIdx = djThread.findIndex(t => t.pending);
  const finalTurn = {
    role: 'dj',
    prompt,
    ts: Date.now(),
    tracks: Array.isArray(result && result.tracks) ? result.tracks : [],
    playlist_id: result && result.playlist_id || null,
    likes: [], dislikes: [],
  };
  if (result && result.error) finalTurn.error = result.error + (result.hint ? (' — ' + result.hint) : '');
  if (pendingIdx !== -1) djThread[pendingIdx] = finalTurn; else djThread.push(finalTurn);
  djLastTracks = finalTurn.tracks;
  saveDjThread();
  renderDjThread();

  // Auto-populate queue with the freshest set so play-next still works.
  if (finalTurn.tracks.length > 0) {
    queueTracks.length = 0;
    finalTurn.tracks.forEach(t => queueTracks.push(t));
    renderQueue();
  }
}

function renderDjPending() {
  // Re-render with the placeholder visible. The pending DJ turn has
  // tracks: null which we display as "// picking tracks…".
  const container = document.getElementById('dj-thread');
  if (!container) return;
  container.innerHTML = djThread.map((t, i) => {
    if (t.pending) {
      return '<div class="dj-turn-dj"><span class="dj-turn-label">DJ</span>' +
        '<div class="dj-turn-prompt"><span class="text-gray-500 italic">// picking tracks…</span></div></div>';
    }
    return renderDjTurn(t, i, false);
  }).join('');
  requestAnimationFrame(() => { container.scrollTop = container.scrollHeight; });
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
  saveDjThread();
  renderDjThread();
}
function toggleDislike(id) {
  if (djDislikes.has(id)) djDislikes.delete(id); else { djDislikes.add(id); djLikes.delete(id); }
  saveDjThread();
  renderDjThread();
}

async function refineDj(instruction) {
  if (djLastTracks.length === 0) return;
  // Build the under-the-hood prompt that includes liked/disliked tracks
  // — same as before — but show the user a friendlier label in the
  // chat transcript so the bubble doesn't read like a system prompt.
  const likedNames = djLastTracks.filter(t => djLikes.has(t.id)).map(t => t.name + ' — ' + t.artist).join('; ');
  const dislikedNames = djLastTracks.filter(t => djDislikes.has(t.id)).map(t => t.name + ' — ' + t.artist).join('; ');
  let refinedPrompt = djLastPrompt + '. ' + instruction;
  if (likedNames) refinedPrompt += '. The user LIKED: ' + likedNames;
  if (dislikedNames) refinedPrompt += '. The user DISLIKED: ' + dislikedNames;
  await runDj(refinedPrompt, instruction);
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

loadDjThread();
renderDjThread();
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

// NOTE: YouTube IFrame API is loaded lazily on first YouTube Music play.
// Eager loading was triggering a full-page "Before you continue to YouTube"
// consent overlay on fresh browser profiles + WebKitGTK viewers. If the
// user never asks for YouTube Music playback, that script never runs.

// ── Local library (user-added folders, file-based playback) ──────────
let localSearchTimer = null;
function debouncedLocalSearch() {
  clearTimeout(localSearchTimer);
  localSearchTimer = setTimeout(loadLocalTracks, 250);
}

async function loadLocalFolders() {
  try {
    const r = await fetch('/api/music/local/folders?token=' + encodeURIComponent(token));
    if (!r.ok) return;
    const d = await r.json();
    const folders = d.folders || [];
    const el = document.getElementById('local-lib-folders');
    if (!folders.length) {
      el.innerHTML = '<p class="text-xs text-gray-500 italic">No folders added yet.</p>';
      document.getElementById('local-lib-scan').classList.add('hidden');
      const cleanupBtn = document.getElementById('local-lib-cleanup');
      if (cleanupBtn) cleanupBtn.classList.add('hidden');
      document.getElementById('local-lib-summary').textContent = 'Point at a folder of audio files on this host.';
    } else {
      const total = folders.reduce((s, f) => s + (f.track_count || 0), 0);
      document.getElementById('local-lib-summary').textContent =
        folders.length + ' folder(s) · ' + total + ' track(s) indexed';
      document.getElementById('local-lib-scan').classList.remove('hidden');
      const cleanupBtn = document.getElementById('local-lib-cleanup');
      if (cleanupBtn && total > 0) cleanupBtn.classList.remove('hidden');
      el.innerHTML = folders.map(f => {
        const lastScan = f.last_scan_at
          ? new Date(f.last_scan_at * 1000).toLocaleDateString()
          : '<span class="text-yellow-500">never scanned</span>';
        return '<div class="flex items-center justify-between gap-2 px-2 py-1 rounded hover:bg-gray-900">'
          + '<div class="min-w-0 flex-1">'
          + '<p class="text-xs text-gray-300 truncate" title="' + escapeHtml(f.path) + '">'
          + escapeHtml(f.label || f.path) + '</p>'
          + '<p class="text-[10px] text-gray-500">' + f.track_count + ' track(s) · ' + lastScan + '</p>'
          + '</div>'
          + '<button onclick="rescanFolder(' + f.id + ')" class="text-[10px] text-gray-500 hover:text-oc-400">rescan</button>'
          + '<button onclick="removeFolder(' + f.id + ')" class="text-[10px] text-gray-500 hover:text-red-400">remove</button>'
          + '</div>';
      }).join('');
    }
    // If we have any folders with tracks, load the track list too.
    if (folders.some(f => f.track_count > 0)) loadLocalTracks();
    else document.getElementById('local-lib-tracks').innerHTML = '';
  } catch(e) { console.warn('[local-lib] load folders failed', e); }
}

async function addLocalFolder() {
  const inp = document.getElementById('local-lib-path');
  const err = document.getElementById('local-lib-error');
  const path = inp.value.trim();
  err.classList.add('hidden');
  if (!path) { err.textContent = 'Enter a folder path.'; err.classList.remove('hidden'); return; }
  try {
    const r = await fetch('/api/music/local/folders', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, path }),
    });
    if (!r.ok) {
      const txt = await r.text();
      err.textContent = txt || ('Add failed (HTTP ' + r.status + ')');
      err.classList.remove('hidden');
      return;
    }
    const d = await r.json();
    inp.value = '';
    await loadLocalFolders();
    // Auto-scan the newly added folder so tracks appear immediately.
    if (d.id) scanLocalLibrary(d.id);
  } catch(e) {
    err.textContent = 'Network error: ' + e.message;
    err.classList.remove('hidden');
  }
}

// ── Folder picker modal ──────────────────────────────────────────────
// Server-driven so we can offer native-feeling navigation without the
// browser's sandboxed folder picker (which doesn't expose absolute paths
// for security). Works for local dirs AND network mounts (/mnt, /media).
let fsPickerCurrent = null; // current resolved path, "" = root shortcuts view
function toggleManualPathEntry() {
  const row = document.getElementById('local-lib-manual-row');
  row.classList.toggle('hidden');
  if (!row.classList.contains('hidden')) {
    document.getElementById('local-lib-path').focus();
  }
}
function openFolderPicker() {
  const modal = document.getElementById('fs-picker-modal');
  // Hoist to document.body so position:fixed is relative to the viewport,
  // not to any ancestor that accidentally became a CSS containing block
  // (overflow, transform, filter all do this). Idempotent.
  if (modal && modal.parentNode !== document.body) {
    document.body.appendChild(modal);
  }
  modal.classList.remove('hidden');
  modal.style.display = 'flex'; // override inline display:none baseline
  document.getElementById('local-lib-error').classList.add('hidden');
  fsPickerLoad(''); // start at root shortcuts
}
function closeFolderPicker() {
  const modal = document.getElementById('fs-picker-modal');
  modal.classList.add('hidden');
  modal.style.display = 'none';
}
async function fsPickerLoad(path) {
  try {
    const url = '/api/fs/list?token=' + encodeURIComponent(token) + (path ? '&path=' + encodeURIComponent(path) : '');
    const r = await fetch(url);
    if (!r.ok) {
      const msg = r.status === 403 ? 'This folder is outside the allowed roots.' :
                  r.status === 404 ? 'Folder not found.' :
                  'Could not list folder (HTTP ' + r.status + ')';
      document.getElementById('fs-picker-entries').innerHTML =
        '<p class="text-xs text-red-400 p-3">' + msg + '</p>';
      return;
    }
    const d = await r.json();
    fsPickerCurrent = d.path || '';
    // Breadcrumb
    document.getElementById('fs-picker-breadcrumb').textContent = d.path || 'Pick a starting location';
    // Up button
    const up = document.getElementById('fs-picker-up');
    up.disabled = !d.parent;
    up.onclick = d.parent ? () => fsPickerLoad(d.parent) : null;
    // Select-current enabled only when we're actually inside a folder
    document.getElementById('fs-picker-select').disabled = !d.path;
    // Roots sidebar (always the same, but re-render for state)
    const rootsEl = document.getElementById('fs-picker-roots');
    rootsEl.innerHTML = (d.roots || []).map(r =>
      '<button onclick="fsPickerLoad(' + JSON.stringify(r.path).replace(/"/g, '&quot;') + ')" ' +
      'class="w-full text-left px-2 py-1.5 rounded hover:bg-gray-800 text-xs text-gray-300 ' +
      (d.path && d.path.startsWith(r.path) ? 'bg-gray-800 text-oc-400' : '') + '">' +
      escapeHtml(r.label) + '</button>'
    ).join('');
    // Entries
    const entriesEl = document.getElementById('fs-picker-entries');
    if (!d.path) {
      entriesEl.innerHTML = '<p class="text-xs text-gray-500 italic p-3">Pick a starting location from the left.</p>';
      return;
    }
    const dirs = (d.entries || []).filter(e => e.is_dir);
    const files = (d.entries || []).filter(e => !e.is_dir);
    if (dirs.length === 0 && files.length === 0) {
      entriesEl.innerHTML = '<p class="text-xs text-gray-500 italic p-3">Empty folder. You can still "Select this folder" to use it as-is, but it won\'t contain any music.</p>';
      return;
    }
    let html = '';
    if (dirs.length) {
      html += dirs.map(e => {
        const childPath = d.path.replace(/\/$/, '') + '/' + e.name;
        return '<button onclick="fsPickerLoad(' + JSON.stringify(childPath).replace(/"/g, '&quot;') + ')" ' +
          'class="w-full text-left px-2 py-1.5 rounded hover:bg-gray-800 text-xs text-gray-200 flex items-center gap-2">' +
          '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-oc-500 flex-shrink-0"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>' +
          '<span class="truncate">' + escapeHtml(e.name) + '</span></button>';
      }).join('');
    }
    if (files.length) {
      // Files shown grayed out — can't pick them, just visual context
      html += files.slice(0, 20).map(e =>
        '<div class="px-2 py-1 rounded text-xs text-gray-600 flex items-center gap-2 cursor-default">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="flex-shrink-0"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>' +
        '<span class="truncate">' + escapeHtml(e.name) + '</span></div>'
      ).join('');
      if (files.length > 20) {
        html += '<p class="text-[11px] text-gray-600 italic p-2">&hellip;and ' + (files.length - 20) + ' more file(s)</p>';
      }
    }
    entriesEl.innerHTML = html;
  } catch(e) {
    console.warn('[fs-picker] load failed', e);
    document.getElementById('fs-picker-entries').innerHTML =
      '<p class="text-xs text-red-400 p-3">Network error: ' + escapeHtml(e.message) + '</p>';
  }
}
function fsPickerGoUp() {
  // Up button is wired dynamically in fsPickerLoad
}
async function fsPickerSelectCurrent() {
  if (!fsPickerCurrent) return;
  const btn = document.getElementById('fs-picker-select');
  btn.disabled = true;
  btn.textContent = 'Adding…';
  try {
    const r = await fetch('/api/music/local/folders', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, path: fsPickerCurrent }),
    });
    if (!r.ok) {
      const txt = await r.text();
      document.getElementById('fs-picker-hint').textContent =
        txt || ('Add failed (HTTP ' + r.status + ')');
      document.getElementById('fs-picker-hint').className = 'text-xs text-red-400 truncate flex-1';
      btn.disabled = false;
      btn.textContent = 'Select this folder';
      return;
    }
    const d = await r.json();
    closeFolderPicker();
    await loadLocalFolders();
    if (d.id) scanLocalLibrary(d.id);
  } catch(e) {
    document.getElementById('fs-picker-hint').textContent = 'Network error: ' + e.message;
    document.getElementById('fs-picker-hint').className = 'text-xs text-red-400 truncate flex-1';
    btn.disabled = false;
    btn.textContent = 'Select this folder';
  }
}

async function removeFolder(id) {
  if (!confirm('Remove this folder from the library? (Your files stay on disk.)')) return;
  try {
    await fetch('/api/music/local/folders/' + id + '?token=' + encodeURIComponent(token),
      { method: 'DELETE' });
    loadLocalFolders();
  } catch(e) { console.warn('[local-lib] remove failed', e); }
}

async function scanLocalLibrary(folderId) {
  const btn = document.getElementById('local-lib-scan');
  if (btn) { btn.disabled = true; btn.textContent = 'Scanning…'; }
  try {
    const body = { token };
    if (folderId) body.folder_id = folderId;
    const r = await fetch('/api/music/local/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    await r.json();
  } catch(e) { console.warn('[local-lib] scan failed', e); }
  if (btn) { btn.disabled = false; btn.textContent = 'Rescan'; }
  await loadLocalFolders();
}

function rescanFolder(id) { scanLocalLibrary(id); }

async function loadLocalTracks() {
  const el = document.getElementById('local-lib-tracks');
  const q = document.getElementById('local-lib-search').value.trim();
  const qs = 'token=' + encodeURIComponent(token) + '&limit=200'
    + (q ? '&q=' + encodeURIComponent(q) : '');
  try {
    const r = await fetch('/api/music/local/tracks?' + qs);
    if (!r.ok) { el.innerHTML = ''; return; }
    const d = await r.json();
    const tracks = d.tracks || [];
    if (!tracks.length) {
      el.innerHTML = q
        ? '<p class="text-xs text-gray-500 italic p-2">No matches for "' + escapeHtml(q) + '".</p>'
        : '';
      return;
    }
    const minutes = (ms) => {
      if (!ms) return '';
      const s = Math.round(ms / 1000);
      return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
    };
    el.innerHTML = '<p class="text-[10px] text-gray-600 px-1 pb-1">' + d.total + ' track(s)</p>'
      + tracks.map(renderTrackRow).join('');
  } catch(e) { console.warn('[local-lib] load tracks failed', e); el.innerHTML = ''; }
}

// Double-click freeze fight, round 2. First fix used a busy flag +
// await play() but that still left a window where WebKitGTK's native
// dblclick handling could fire a second click event before the guard
// was set, which wedged the <audio> element. This version:
//   • debounces at a timestamp level (400 ms lockout across ALL
//     invocation paths — click delegation, programmatic calls, cloud
//     play buttons).
//   • bails out when click.detail > 1 (second click of a double).
//   • tears the <audio> element down fully — pause, clear src,
//     removeAttribute, load() — before setting the new src, so no
//     half-buffered resource can collide with the new one.
//   • yields to the event loop via setTimeout(0) between tear-down and
//     new src, letting WebKit finish its internal cleanup pass.
//   • swallows AbortError (expected when src changes mid-load).
let lastLocalPlayAt = 0;
let localPlayGeneration = 0;
// Active-local-playback flag — when true, loadNowPlaying() leaves the
// Now Playing card alone so the cloud poll doesn't overwrite local
// state. Cleared on 'ended' / 'error' / when the user kicks off a
// cloud playback.
let localPlaybackActive = false;
// Web Audio graph — built lazily on the first successful play() so we
// don't ask for an AudioContext before a user gesture. The real
// equalizer in the Now Playing card reads frequency data from the
// analyser every animation frame.
let webAudioCtx = null;
let webAudioSource = null;
let webAudioAnalyser = null;
let vizFrameId = null;

function playLocalTrack(trackId, title, artist, extra) {
  const now = Date.now();
  if (now - lastLocalPlayAt < 400) return;
  lastLocalPlayAt = now;
  localPlaybackCurrent = trackId;
  const myGen = ++localPlayGeneration;
  const a = document.getElementById('local-audio');
  try { a.pause(); } catch(e) {}
  try { a.removeAttribute('src'); } catch(e) {}
  try { a.load(); } catch(e) {}
  setLocalNowPlaying(title, artist, trackId, extra || lookupRowMeta(trackId));
  setTimeout(() => {
    if (myGen !== localPlayGeneration) return;
    a.src = '/api/music/local/file/' + trackId + '?token=' + encodeURIComponent(token);
    a.load();
    const p = a.play();
    if (p && typeof p.catch === 'function') {
      p.catch(e => {
        if (e && e.name !== 'AbortError') {
          console.warn('[local-play]', e);
          showMusicNotice("Couldn't play that track. " + (e.message || ''), true);
          clearLocalNowPlaying();
        }
      });
    }
  }, 50);
}

// Walk the currently-rendered rows to find extra metadata for a click
// so the Now Playing header gets album + bit depth + favorite state
// even though the button's data-* attrs only carry title/artist.
function lookupRowMeta(trackId) {
  const row = document.querySelector('[data-track-row="' + trackId + '"]');
  if (!row) return {};
  const album = row.querySelector('p.text-\\[10px\\]')?.textContent.split(' · ')[1] || '';
  const fav = !!row.querySelector('.local-fav-btn.text-pink-400, .local-fav-btn[title="Unlove"]');
  // We don't try to reconstruct bit_depth/sample_rate from the DOM;
  // the format badge in the hero stays hidden unless we have them.
  return { album, favorite: fav };
}

// Queue state (minimal — shuffle-within-current-view support).
let npShuffle = false;
let npRepeat = 'off';  // off | all | one
function toggleShuffle() {
  npShuffle = !npShuffle;
  document.getElementById('np-shuffle')?.classList.toggle('active', npShuffle);
}
function toggleRepeat() {
  npRepeat = npRepeat === 'off' ? 'all' : (npRepeat === 'all' ? 'one' : 'off');
  const btn = document.getElementById('np-repeat');
  if (btn) {
    btn.classList.toggle('active', npRepeat !== 'off');
    btn.title = 'Repeat: ' + npRepeat;
  }
}
async function toggleFavorite() {
  if (!localPlaybackCurrent) return;
  const btn = document.getElementById('np-favorite');
  const isLoved = btn && btn.classList.contains('active');
  try {
    await fetch('/api/music/local/favorite/' + localPlaybackCurrent, { method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token, favorite: !isLoved }) });
    btn?.classList.toggle('active', !isLoved);
  } catch(e) {}
}

// Next / prev playback across the current on-screen track rows.
// Rough cut: siblings in the rendered list. Respects shuffle + repeat.
function currentRowList() {
  return Array.from(document.querySelectorAll('.local-play-btn'));
}
function playRelativeTo(offset) {
  const rows = currentRowList();
  if (!rows.length) return;
  const ids = rows.map(b => parseInt(b.dataset.trackId, 10));
  let idx = ids.indexOf(localPlaybackCurrent);
  if (idx < 0) idx = 0;
  let next = offset > 0 ? idx + 1 : idx - 1;
  if (npRepeat === 'one') next = idx;
  if (npShuffle) next = Math.floor(Math.random() * ids.length);
  if (next < 0) next = npRepeat === 'all' ? ids.length - 1 : 0;
  if (next >= ids.length) next = npRepeat === 'all' ? 0 : ids.length - 1;
  const btn = rows[next];
  if (!btn) return;
  playLocalTrack(parseInt(btn.dataset.trackId, 10), btn.dataset.trackTitle || '', btn.dataset.trackArtist || '');
}

// ── Mirror local playback into the big Now Playing card ─────────────
// Paints title / artist / source / album art / play-button icon /
// format badge, and swaps the static viz out for the canvas-driven
// AnalyserNode spectrum. Also sets MediaSession metadata so OS media
// keys + iOS lock screen reflect what's actually playing.
function setLocalNowPlaying(title, artist, trackId, extra) {
  localPlaybackActive = true;
  const row = extra || {};
  const songEl = document.getElementById('np-song');
  if (songEl) {
    songEl.textContent = title || 'Track ' + trackId;
    applyMarquee('np-song-wrap', 'np-song');
  }
  const artistEl = document.getElementById('np-artist');
  if (artistEl) artistEl.textContent = artist || '—';
  const srcEl = document.getElementById('np-source');
  if (srcEl) srcEl.textContent = 'Your library' + (row.album ? ' · ' + row.album : '');

  // Format badge (FLAC 24/96 etc.)
  const badge = document.getElementById('np-format-badge');
  if (badge) {
    const parts = [];
    if (row.bit_depth) parts.push(row.bit_depth + '-bit');
    if (row.sample_rate) parts.push((row.sample_rate/1000).toFixed(1).replace(/\.0$/,'') + 'kHz');
    if (parts.length) { badge.textContent = parts.join(' · '); badge.classList.remove('hidden'); }
    else { badge.classList.add('hidden'); }
  }

  const playBtn = document.getElementById('np-play');
  if (playBtn) playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>';

  // Album art — always try the /art endpoint; server handles the three-
  // tier fallback (embedded → folder.jpg → MB Cover Art Archive). If
  // the response is a 404 we fall back to the music-note placeholder.
  const artEl = document.getElementById('np-art');
  if (artEl) {
    const url = '/api/music/local/art/' + trackId + '?token=' + encodeURIComponent(token);
    artEl.innerHTML = '<img src="' + url + '" style="width:100%;height:100%;object-fit:cover;display:block;" onerror="this.remove();">'
      + '<svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" class="text-gray-700" style="position:absolute;z-index:-1"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>';
  }

  // Love button reflects the track's favorite state.
  const loveBtn = document.getElementById('np-favorite');
  if (loveBtn) loveBtn.classList.toggle('active', !!row.favorite);

  // Hide the HA volume row — we control volume via the <audio> element.
  const volRow = document.getElementById('volume-row');
  if (volRow) volRow.classList.add('hidden');

  // OS-level media controls. Set metadata + position + action handlers
  // so Play/Pause/Next keys on the keyboard, remotes, and iOS lock
  // screens all work.
  if ('mediaSession' in navigator) {
    try {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: title || 'Track ' + trackId,
        artist: artist || '',
        album: row.album || '',
        artwork: [{ src: '/api/music/local/art/' + trackId + '?token=' + encodeURIComponent(token), sizes: '512x512', type: 'image/jpeg' }],
      });
      navigator.mediaSession.playbackState = 'playing';
      navigator.mediaSession.setActionHandler('play', () => { const a = document.getElementById('local-audio'); if (a && a.paused) a.play(); });
      navigator.mediaSession.setActionHandler('pause', () => { const a = document.getElementById('local-audio'); if (a && !a.paused) a.pause(); });
      navigator.mediaSession.setActionHandler('nexttrack', () => control('next'));
      navigator.mediaSession.setActionHandler('previoustrack', () => control('prev'));
      navigator.mediaSession.setActionHandler('seekto', (e) => { const a = document.getElementById('local-audio'); if (a && e.seekTime != null) a.currentTime = e.seekTime; });
      navigator.mediaSession.setActionHandler('seekforward', () => { const a = document.getElementById('local-audio'); if (a) a.currentTime = Math.min(a.duration || 1e9, a.currentTime + 10); });
      navigator.mediaSession.setActionHandler('seekbackward', () => { const a = document.getElementById('local-audio'); if (a) a.currentTime = Math.max(0, a.currentTime - 10); });
    } catch(e) {}
  }

  // Log the play after a couple of seconds (skip scrubbing bounces).
  setTimeout(() => {
    if (localPlaybackCurrent === trackId) {
      fetch('/api/music/local/played/' + trackId + '?token=' + encodeURIComponent(token), { method: 'POST' }).catch(()=>{});
    }
  }, 2500);
}

let localPlaybackCurrent = null;

function clearLocalNowPlaying() {
  localPlaybackActive = false;
  localPlaybackCurrent = null;
  const songEl = document.getElementById('np-song');
  if (songEl) songEl.textContent = 'Nothing playing';
  const artistEl = document.getElementById('np-artist');
  if (artistEl) artistEl.textContent = '—';
  const srcEl = document.getElementById('np-source');
  if (srcEl) srcEl.textContent = '';
  const badge = document.getElementById('np-format-badge');
  if (badge) badge.classList.add('hidden');
  const playBtn = document.getElementById('np-play');
  if (playBtn) playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
  const timeCur = document.getElementById('np-time-cur');
  if (timeCur) timeCur.textContent = '0:00';
  const timeDur = document.getElementById('np-time-dur');
  if (timeDur) timeDur.textContent = '0:00';
  const prog = document.getElementById('np-progress');
  if (prog) { prog.value = 0; prog.style.setProperty('--progress', '0%'); }
  if ('mediaSession' in navigator) {
    try { navigator.mediaSession.metadata = null; navigator.mediaSession.playbackState = 'none'; } catch(e) {}
  }
  stopRealEqualizer();
}

// ── Real audio-reactive spectrum (canvas) ────────────────────────────
// Replaces the old 8-bar scaleY strip. Bigger (full width × 64px),
// more bars (32), logarithmic frequency mapping, mag→cyan gradient.
// Drawn from AnalyserNode 60fps.
function ensureRealEqualizer(audioEl) {
  try {
    if (!webAudioCtx) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return;
      webAudioCtx = new AC();
    }
    if (webAudioCtx.state === 'suspended') webAudioCtx.resume().catch(()=>{});
    if (!webAudioSource || webAudioSource.mediaElement !== audioEl) {
      try {
        webAudioSource = webAudioCtx.createMediaElementSource(audioEl);
        webAudioAnalyser = webAudioCtx.createAnalyser();
        webAudioAnalyser.fftSize = 1024;            // finer bin resolution, esp in the bass
        webAudioAnalyser.smoothingTimeConstant = 0.68;
        // Tighten the dB range the byte data maps to. Default is
        // [-100, -30] which means ANY audio lights up the low bins to
        // near-max and leaves highs flat. Narrower range + higher
        // noise floor gives bass room to breathe AND makes highs
        // actually register.
        webAudioAnalyser.minDecibels = -75;
        webAudioAnalyser.maxDecibels = -12;
        webAudioSource.connect(webAudioAnalyser);
        webAudioAnalyser.connect(webAudioCtx.destination);
      } catch(e) {
        console.warn('[viz] AudioContext wiring failed:', e);
        return;
      }
    }
  } catch(e) {
    console.warn('[viz] ensureRealEqualizer:', e);
    return;
  }
  startRealEqualizer();
}

function startRealEqualizer() {
  if (!webAudioAnalyser) return;
  if (vizFrameId) cancelAnimationFrame(vizFrameId);
  const canvas = document.getElementById('np-spectrum');
  if (!canvas) return;
  const ctx = canvas.getContext('2d');
  const bins = new Uint8Array(webAudioAnalyser.frequencyBinCount);

  // Mirrored spectrum — classic car-stereo / Winamp look. The left
  // and right halves are a reflection of each other: bass in the
  // center, mids + highs radiating outward to both edges. We compute
  // heights for the inner half only (HALF bars) then paint them
  // twice, once walking outward-right from center and once
  // outward-left.
  const BAR_COUNT = 38;                  // total across the full width
  const HALF = Math.floor(BAR_COUNT / 2);
  // Headroom: peak amplitude fills only 78 % of the canvas height so
  // loud passages don't slam into the top edge. The remaining 22 % is
  // negative space that makes the motion feel alive instead of clipped.
  const CEILING = 0.78;

  const loop = () => {
    const cssW = canvas.clientWidth;
    if (cssW > 0 && canvas.width !== cssW) canvas.width = cssW;
    const W = canvas.width, H = canvas.height;
    webAudioAnalyser.getByteFrequencyData(bins);
    ctx.clearRect(0, 0, W, H);
    const gap = 2;
    const barW = Math.max(2, (W - gap * (BAR_COUNT + 1)) / BAR_COUNT);

    // Compute HALF heights. Index 0 = bass (center), HALF-1 = treble
    // (edges). Logarithmic bin mapping so bass gets more resolution.
    //
    // Per-bar weighting compensates for the pink-noise distribution
    // of real music: bass FFT bins carry roughly 20 dB more energy
    // than treble bins on typical pop/rock content. Without this
    // curve, bass constantly pegs the ceiling while highs never
    // register. The curve (0.42 → 2.35 from center to edge) flattens
    // the average response so the whole spectrum moves with the music.
    const heights = new Array(HALF);
    for (let i = 0; i < HALF; i++) {
      const lo = Math.floor(Math.pow(i / HALF, 2.3) * bins.length);
      const hi = Math.floor(Math.pow((i + 1) / HALF, 2.3) * bins.length);
      let peak = 0;
      for (let b = lo; b <= hi && b < bins.length; b++) { if (bins[b] > peak) peak = bins[b]; }
      const t = i / (HALF - 1);                    // 0 = bass, 1 = treble
      const weight = 0.42 + Math.pow(t, 0.55) * 1.93;
      const v = Math.min(1, (peak / 255) * weight);
      heights[i] = Math.max(2, v * H * CEILING);
    }

    // Paint bars[HALF..BAR_COUNT] going right from center (low→high freq).
    for (let i = 0; i < HALF; i++) {
      const rightIdx = HALF + i;         // HALF, HALF+1, ..., BAR_COUNT-1
      const leftIdx  = HALF - 1 - i;     // HALF-1, HALF-2, ..., 0
      const h = heights[i];
      const y = H - h;
      const xR = gap + rightIdx * (barW + gap);
      const xL = gap + leftIdx  * (barW + gap);
      const grad = ctx.createLinearGradient(0, y, 0, H);
      grad.addColorStop(0, '#0cf8f0');
      grad.addColorStop(1, '#ff2cdf');
      ctx.fillStyle = grad;
      ctx.fillRect(xR, y, barW, h);
      ctx.fillRect(xL, y, barW, h);
    }
    vizFrameId = requestAnimationFrame(loop);
  };
  loop();
}

function stopRealEqualizer() {
  if (vizFrameId) { cancelAnimationFrame(vizFrameId); vizFrameId = null; }
  const canvas = document.getElementById('np-spectrum');
  if (canvas) {
    const ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  }
}

// Keep the Now Playing UI in sync with the <audio> element's own
// state events. Covers cases the user pauses via media keys, the
// stream ends naturally, or an error fires mid-playback.
(function wireLocalAudioEvents() {
  const bind = () => {
    const a = document.getElementById('local-audio');
    if (!a || a.dataset.wired === '1') return;
    a.dataset.wired = '1';
    a.addEventListener('play', () => {
      ensureRealEqualizer(a);
      const playBtn = document.getElementById('np-play');
      if (playBtn) playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>';
      const viz = document.getElementById('np-viz');
      if (viz) viz.classList.remove('viz-paused');
    });
    a.addEventListener('pause', () => {
      // Pause ≠ stop. Keep track info visible, just pause the viz.
      const viz = document.getElementById('np-viz');
      if (viz) viz.classList.add('viz-paused');
      const playBtn = document.getElementById('np-play');
      if (playBtn) playBtn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><polygon points="5,3 19,12 5,21"/></svg>';
    });
    a.addEventListener('ended', () => {
      // Advance in queue rather than clearing the card — that's what
      // every other player does. If repeat is off and we're at the end
      // of the list, clear gracefully.
      if (localPlaybackActive) {
        const rows = currentRowList();
        const ids = rows.map(b => parseInt(b.dataset.trackId, 10));
        const idx = ids.indexOf(localPlaybackCurrent);
        if (npRepeat === 'all' || idx < ids.length - 1 || npShuffle) {
          playRelativeTo(1);
          return;
        }
      }
      clearLocalNowPlaying();
    });
    a.addEventListener('error', () => {
      if (a.error) console.warn('[local-audio] error', a.error.code, a.error.message);
      clearLocalNowPlaying();
    });
    // Progress bar + time labels. timeupdate fires ~4/sec during
    // playback — cheap enough to just read currentTime directly.
    a.addEventListener('timeupdate', () => {
      const prog = document.getElementById('np-progress');
      const cur = document.getElementById('np-time-cur');
      const dur = document.getElementById('np-time-dur');
      if (a.duration && isFinite(a.duration)) {
        const pct = (a.currentTime / a.duration) * 1000;
        if (prog && !prog.dataset.scrubbing) {
          prog.value = pct;
          prog.style.setProperty('--progress', (pct/10).toFixed(1) + '%');
        }
        if (dur) dur.textContent = fmtMs(a.duration * 1000);
        if ('mediaSession' in navigator && navigator.mediaSession.setPositionState) {
          try {
            navigator.mediaSession.setPositionState({
              duration: a.duration,
              playbackRate: a.playbackRate || 1,
              position: a.currentTime,
            });
          } catch(e) {}
        }
      }
      if (cur) cur.textContent = fmtMs(a.currentTime * 1000);
    });
  };
  bind();
  // Also re-bind whenever loadLocalFolders re-renders (in case the
  // <audio> element ever gets replaced).
  const obs = new MutationObserver(bind);
  const root = document.getElementById('local-library-card');
  if (root) obs.observe(root, { childList: true, subtree: true });
})();

// Render one track row. Shared across the Tracks / Favorites / Recent
// / Search-result / Playlist-detail views so every list gets the same
// row affordances: art thumb, title+artist, heart, lossless badge,
// details drawer, auto/MB source badge.
function renderTrackRow(t) {
  const title = escapeHtml(t.title || '(untitled)');
  const artist = escapeHtml(t.artist || '');
  const album = escapeHtml(t.album || '');
  const srcBadge = t.metadata_source === 'llm'
    ? ' <span title="AI-inferred tags" class="text-[9px] text-amber-400 font-mono uppercase">auto</span>'
    : t.metadata_source === 'musicbrainz'
      ? ' <span title="Canonical MusicBrainz" class="text-[9px] text-emerald-400 font-mono uppercase">MB</span>'
      : '';
  const fmtBadge = (t.bit_depth >= 24 || t.sample_rate > 48000)
    ? ' <span class="text-[9px] font-mono uppercase text-cyan-300 border border-cyan-700/50 px-1 rounded">' + (t.bit_depth || '') + (t.bit_depth && t.sample_rate ? '/' : '') + (t.sample_rate ? Math.round(t.sample_rate/1000) : '') + '</span>'
    : '';
  const minutes = (ms) => { if (!ms) return ''; const s = Math.round(ms/1000); return Math.floor(s/60) + ':' + String(s%60).padStart(2,'0'); };
  const heart = t.favorite
    ? '<button class="text-[11px] text-pink-400 flex-shrink-0 local-fav-btn" data-track-id="' + t.id + '" title="Unlove">♥</button>'
    : '<button class="text-[11px] text-gray-600 hover:text-pink-400 opacity-0 group-hover:opacity-100 transition-opacity local-fav-btn flex-shrink-0" data-track-id="' + t.id + '" title="Love">♡</button>';
  const art = t.has_art
    ? '<span class="row-art" style="background-image:url(/api/music/local/art/' + t.id + '?token=' + encodeURIComponent(token) + ')"></span>'
    : '<span class="row-art placeholder"></span>';
  return '<div class="flex items-center gap-2 px-2 py-1 rounded hover:bg-gray-900 group" data-track-row="' + t.id + '" style="user-select:none;-webkit-user-select:none;">'
    + art
    + '<button class="flex-1 min-w-0 text-left local-play-btn"'
    + ' data-track-id="' + t.id + '"'
    + ' data-track-title="' + title + '"'
    + ' data-track-artist="' + artist + '">'
    + '<p class="text-xs text-gray-200 truncate">' + title + srcBadge + fmtBadge + '</p>'
    + (artist || album
      ? '<p class="text-[10px] text-gray-500 truncate">' + [artist, album].filter(Boolean).join(' · ') + '</p>'
      : '')
    + '</button>'
    + heart
    + '<span class="text-[10px] text-gray-600 font-mono flex-shrink-0">' + minutes(t.duration_ms) + '</span>'
    + '<button class="text-[10px] text-gray-600 hover:text-oc-400 opacity-0 group-hover:opacity-100 transition-opacity local-details-btn flex-shrink-0" title="Details + edit + lyrics" data-track-id="' + t.id + '">Details</button>'
    + '</div>';
}

// Library view switcher — Tracks / Albums / Artists / Favorites / Recent / Playlists / Duplicates.
let currentLibView = 'tracks';
function switchLibView(view) {
  currentLibView = view;
  document.querySelectorAll('.lib-tab').forEach(b => b.classList.toggle('active', b.dataset.view === view));
  const el = document.getElementById('local-lib-tracks');
  el.innerHTML = '<p class="text-[10px] text-gray-500 italic p-2">Loading…</p>';
  if (view === 'tracks')       loadLocalTracks();
  else if (view === 'favorites') loadFavoritesView();
  else if (view === 'recent')    loadRecentView();
  else if (view === 'albums')    loadAlbumsView();
  else if (view === 'artists')   loadArtistsView();
  else if (view === 'playlists') loadPlaylistsView();
  else if (view === 'duplicates') loadDuplicatesView();
}
async function loadFavoritesView() {
  try {
    const r = await fetch('/api/music/local/tracks?token=' + encodeURIComponent(token) + '&limit=200');
    if (!r.ok) return;
    const d = await r.json();
    const favs = (d.tracks || []).filter(t => t.favorite);
    const el = document.getElementById('local-lib-tracks');
    if (!favs.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">Love a track to see it here. Tap ♡ on any row.</p>'; return; }
    el.innerHTML = '<p class="text-[10px] text-gray-600 px-1 pb-1">' + favs.length + ' favorite(s)</p>' + favs.map(renderTrackRow).join('');
  } catch(e) {}
}
async function loadRecentView() {
  // Use tracks endpoint sorted by last_played_at via a simple NL call
  try {
    const r = await fetch('/api/music/local/nl_search', { method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token, query: 'most recently played' }) });
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const rows = d.tracks || [];
    if (!rows.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">No plays logged yet.</p>'; return; }
    el.innerHTML = '<p class="text-[10px] text-gray-600 px-1 pb-1">' + rows.length + ' recently played</p>' + rows.map(renderTrackRow).join('');
  } catch(e) {}
}
async function loadAlbumsView() {
  try {
    const r = await fetch('/api/music/local/albums?token=' + encodeURIComponent(token));
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const albums = d.albums || [];
    if (!albums.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">No albums yet — scan a folder first.</p>'; return; }
    el.innerHTML = '<div class="alb-grid">' + albums.map(a => {
      const art = a.art_track_id
        ? '<img src="/api/music/local/art/' + a.art_track_id + '?token=' + encodeURIComponent(token) + '" onerror="this.remove()">'
        : '';
      return '<div class="alb-tile" onclick="openAlbum(' + JSON.stringify(a.album).replace(/"/g,'&quot;') + ',' + JSON.stringify(a.artist).replace(/"/g,'&quot;') + ')">'
        + '<div class="alb-art">' + art + '</div>'
        + '<div class="alb-name">' + escapeHtml(a.album) + '</div>'
        + '<div class="alb-artist">' + escapeHtml(a.artist) + (a.year ? ' · ' + a.year : '') + '</div>'
        + '</div>';
    }).join('') + '</div>';
  } catch(e) {}
}
async function openAlbum(album, artist) {
  const q = document.getElementById('local-lib-search');
  q.value = album;
  currentLibView = 'tracks';
  document.querySelectorAll('.lib-tab').forEach(b => b.classList.toggle('active', b.dataset.view === 'tracks'));
  await loadLocalTracks();
  // Load liner notes in the background for this album
  try {
    const r = await fetch('/api/music/local/album_notes?token=' + encodeURIComponent(token) + '&artist=' + encodeURIComponent(artist) + '&album=' + encodeURIComponent(album));
    if (r.ok) {
      const d = await r.json();
      const notice = document.getElementById('music-notice');
      if (notice && d.body) {
        // Just drop it at the top of the track list as a quote
        const el = document.getElementById('local-lib-tracks');
        el.innerHTML = '<blockquote class="text-[11px] text-gray-400 italic p-3 border-l-2 border-pink-700/40 mb-2">' + escapeHtml(d.body) + '</blockquote>' + el.innerHTML;
      }
    }
  } catch(e) {}
}
async function loadArtistsView() {
  try {
    const r = await fetch('/api/music/local/artists?token=' + encodeURIComponent(token));
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const artists = d.artists || [];
    if (!artists.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">No artists yet.</p>'; return; }
    el.innerHTML = artists.map(a =>
      '<div class="artist-row" onclick="openArtist(' + JSON.stringify(a.name).replace(/"/g,'&quot;') + ')">'
      + '<div class="artist-name">' + escapeHtml(a.name) + '</div>'
      + '<div class="artist-count">' + a.album_count + ' album' + (a.album_count !== 1 ? 's' : '') + ' · ' + a.track_count + ' track' + (a.track_count !== 1 ? 's' : '') + '</div>'
      + '</div>').join('');
  } catch(e) {}
}
async function openArtist(name) {
  const q = document.getElementById('local-lib-search');
  q.value = name;
  currentLibView = 'tracks';
  document.querySelectorAll('.lib-tab').forEach(b => b.classList.toggle('active', b.dataset.view === 'tracks'));
  await loadLocalTracks();
}
async function loadPlaylistsView() {
  try {
    const r = await fetch('/api/music/local/playlists?token=' + encodeURIComponent(token));
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const pls = d.playlists || [];
    let html = '<div class="flex gap-2 mb-2"><input id="new-pl-name" placeholder="New playlist name" class="flex-1 bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-xs text-gray-200 outline-none"><button onclick="createPlaylistFromUI()" class="bg-oc-600 hover:bg-oc-700 text-white px-3 rounded-lg text-xs">Create</button></div>';
    if (!pls.length) html += '<p class="text-xs text-gray-500 italic p-2">No playlists yet. Create one above.</p>';
    else html += pls.map(p =>
      '<div class="artist-row" onclick="openPlaylist(' + p.id + ')">'
      + '<div class="artist-name">' + escapeHtml(p.name) + '</div>'
      + '<div class="artist-count">' + p.track_count + ' track' + (p.track_count !== 1 ? 's' : '') + '</div>'
      + '</div>').join('');
    el.innerHTML = html;
  } catch(e) {}
}
async function createPlaylistFromUI() {
  const name = document.getElementById('new-pl-name').value.trim();
  if (!name) return;
  try {
    const r = await fetch('/api/music/local/playlists', { method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token, name }) });
    if (r.ok) loadPlaylistsView();
  } catch(e) {}
}
async function openPlaylist(id) {
  try {
    const r = await fetch('/api/music/local/playlists/' + id + '?token=' + encodeURIComponent(token));
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const tracks = d.tracks || [];
    el.innerHTML = '<p class="text-[10px] text-gray-600 px-1 pb-1">' + tracks.length + ' track(s) in this playlist</p>'
      + tracks.map(renderTrackRow).join('');
  } catch(e) {}
}
async function loadDuplicatesView() {
  try {
    const r = await fetch('/api/music/local/duplicates?token=' + encodeURIComponent(token));
    const d = await r.json();
    const el = document.getElementById('local-lib-tracks');
    const groups = d.groups || [];
    if (!groups.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">No duplicates — your library is clean.</p>'; return; }
    el.innerHTML = groups.map(g =>
      '<div class="dup-row">'
      + '<div class="text-xs text-gray-200">' + escapeHtml(g.title) + ' — ' + escapeHtml(g.artist) + '</div>'
      + '<div class="dup-meta">' + g.count + ' copies · ids ' + g.ids.join(', ') + '</div>'
      + '</div>').join('');
  } catch(e) {}
}
async function runNLSearch() {
  const input = document.getElementById('local-lib-nl');
  const q = input.value.trim();
  if (!q) return;
  const el = document.getElementById('local-lib-tracks');
  el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">Thinking…</p>';
  try {
    const r = await fetch('/api/music/local/nl_search', { method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token, query: q }) });
    const d = await r.json();
    const tracks = d.tracks || [];
    if (!tracks.length) { el.innerHTML = '<p class="text-xs text-gray-500 italic p-4 text-center">Couldn\'t match that query. Try different words.</p>'; return; }
    el.innerHTML = '<p class="text-[10px] text-gray-600 px-1 pb-1">' + tracks.length + ' match(es) for "' + escapeHtml(q) + '"</p>'
      + tracks.map(renderTrackRow).join('');
  } catch(e) {
    el.innerHTML = '<p class="text-xs text-red-400 p-4 text-center">Search failed.</p>';
  }
}

// Top-level click delegation on the local track list. Three classes:
// .local-play-btn (row-play), .local-details-btn (MB + lyrics + edit),
// .local-fav-btn (love toggle). Using data attributes + delegation
// avoids the escape bugs of inlined onclick strings and survives track
// list re-renders without rewiring.
//
// Double-click guards — layered defence so WebKitGTK can't freeze:
//   • swallow the native dblclick event (no text selection, no UA hook)
//   • ignore `click` events with detail > 1 (the second click of a
//     double-click fires a separate event with detail=2)
//   • debounce the play handler itself (playLocalTrack has its own
//     400 ms lockout as a third line of defence)
(function() {
  const listEl = document.getElementById('local-lib-tracks');
  if (!listEl) return;
  listEl.addEventListener('dblclick', (ev) => {
    if (ev.target.closest('.local-play-btn') || ev.target.closest('.local-details-btn')) {
      ev.preventDefault();
      ev.stopPropagation();
    }
  });
  listEl.addEventListener('click', async (ev) => {
    if (ev.detail > 1) { ev.preventDefault(); return; }
    const favBtn = ev.target.closest('.local-fav-btn');
    if (favBtn) {
      ev.stopPropagation();
      const id = parseInt(favBtn.dataset.trackId, 10);
      const isLoved = favBtn.textContent.trim() === '♥';
      try {
        await fetch('/api/music/local/favorite/' + id, { method: 'POST', headers: {'Content-Type':'application/json'},
          body: JSON.stringify({ token, favorite: !isLoved }) });
        favBtn.textContent = isLoved ? '♡' : '♥';
        favBtn.className = isLoved
          ? 'text-[11px] text-gray-600 hover:text-pink-400 opacity-0 group-hover:opacity-100 transition-opacity local-fav-btn flex-shrink-0'
          : 'text-[11px] text-pink-400 flex-shrink-0 local-fav-btn';
      } catch(e) {}
      return;
    }
    const playBtn = ev.target.closest('.local-play-btn');
    if (playBtn) {
      const id = parseInt(playBtn.dataset.trackId, 10);
      playLocalTrack(id, playBtn.dataset.trackTitle || '', playBtn.dataset.trackArtist || '');
      return;
    }
    const detailsBtn = ev.target.closest('.local-details-btn');
    if (detailsBtn) {
      const id = parseInt(detailsBtn.dataset.trackId, 10);
      openLocalDetails(id);
      return;
    }
  });
})();

// ── Local track details drawer (MusicBrainz lookup + manual edit) ────
const localDetailsState = { trackId: null, current: null, matches: [] };

async function openLocalDetails(trackId) {
  localDetailsState.trackId = trackId;
  const modal = document.getElementById('local-details-modal');
  const body = document.getElementById('local-details-body');
  if (!modal || !body) return;
  modal.classList.remove('hidden');
  modal.style.display = 'flex';
  body.innerHTML = '<p class="text-xs text-gray-500 italic">Looking this up on MusicBrainz…</p>';
  try {
    const r = await fetch('/api/music/local/lookup/' + trackId + '?token=' + encodeURIComponent(token));
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const data = await r.json();
    localDetailsState.current = data.current || {};
    localDetailsState.matches = data.matches || [];
    renderLocalDetails();
  } catch(e) {
    body.innerHTML = '<p class="text-xs text-red-400">Lookup failed: ' + escapeHtml(e.message || '') + '</p>';
  }
}

function closeLocalDetails() {
  const modal = document.getElementById('local-details-modal');
  if (modal) { modal.classList.add('hidden'); modal.style.display = 'none'; }
  localDetailsState.trackId = null;
}

function renderLocalDetails() {
  const body = document.getElementById('local-details-body');
  if (!body) return;
  const cur = localDetailsState.current || {};
  const matches = localDetailsState.matches || [];
  let html = '';
  html += '<div class="mb-4 p-3 rounded-lg bg-gray-900 border border-gray-800">'
       +    '<p class="text-[10px] text-gray-500 uppercase tracking-wider mb-1">Current</p>'
       +    '<p class="text-sm text-gray-200">' + escapeHtml(cur.title || '(no title)') + '</p>'
       +    '<p class="text-xs text-gray-500">' + escapeHtml(cur.artist || '(no artist)') + (cur.album ? ' · ' + escapeHtml(cur.album) : '') + '</p>'
       + '</div>';
  if (matches.length === 0) {
    html += '<p class="text-xs text-gray-500 italic">No MusicBrainz matches found. You can edit the tags manually below.</p>';
  } else {
    html += '<p class="text-[10px] text-gray-500 uppercase tracking-wider mb-2">Matches on MusicBrainz</p>';
    html += '<div class="space-y-2">' + matches.map((m, i) =>
      '<div class="p-3 rounded-lg bg-gray-900 border border-gray-800 hover:border-oc-600 transition-colors">'
      +   '<div class="flex items-start justify-between gap-3">'
      +     '<div class="flex-1 min-w-0">'
      +       '<p class="text-sm text-gray-200 truncate">' + escapeHtml(m.title || '(unknown)') + '</p>'
      +       '<p class="text-xs text-gray-500 truncate">' + escapeHtml(m.artist || '') + (m.album ? ' · ' + escapeHtml(m.album) : '') + (m.year ? ' · ' + escapeHtml(m.year) : '') + '</p>'
      +     '</div>'
      +     '<div class="flex items-center gap-2 flex-shrink-0">'
      +       '<span class="text-[10px] text-gray-600 font-mono">' + (m.score || '') + '</span>'
      +       '<button onclick="applyLocalMatch(' + i + ')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1 rounded-lg">Use this</button>'
      +     '</div>'
      +   '</div>'
      + '</div>'
    ).join('') + '</div>';
  }
  html += '<hr class="border-gray-800 my-4">';
  html += '<p class="text-[10px] text-gray-500 uppercase tracking-wider mb-2">Or edit by hand</p>';
  html += '<div class="space-y-2">'
       +    '<input id="local-edit-title" class="w-full bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 outline-none focus:border-oc-500" placeholder="Title" value="' + escapeHtml(cur.title || '') + '">'
       +    '<input id="local-edit-artist" class="w-full bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 outline-none focus:border-oc-500" placeholder="Artist" value="' + escapeHtml(cur.artist || '') + '">'
       +    '<input id="local-edit-album" class="w-full bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 outline-none focus:border-oc-500" placeholder="Album" value="' + escapeHtml(cur.album || '') + '">'
       +    '<div class="flex gap-2 items-center">'
       +      '<button onclick="saveLocalEdit()" class="bg-oc-600 hover:bg-oc-700 text-white px-4 py-1.5 rounded-lg text-sm">Save my edits</button>'
       +      '<button onclick="revertLocal()" class="text-xs text-gray-400 hover:text-pink-400 underline">Revert to file tags</button>'
       +    '</div>'
       + '</div>';
  html += '<hr class="border-gray-800 my-4">';
  html += '<div class="flex items-center justify-between mb-2">'
       +    '<p class="text-[10px] text-gray-500 uppercase tracking-wider">Lyrics</p>'
       +    '<button onclick="fetchLyrics()" class="text-xs text-cyan-400 hover:text-cyan-300">Fetch from LRCLIB →</button>'
       + '</div>';
  html += '<div id="local-lyrics-panel" class="lyrics-scroll">'
       +    '<p class="text-xs text-gray-500 italic">Click fetch to look this up.</p>'
       + '</div>';
  body.innerHTML = html;
}

async function revertLocal() {
  if (!localDetailsState.trackId) return;
  if (!confirm('Revert this track to its original file tags? Any LLM / MusicBrainz edits will be discarded.')) return;
  try {
    const r = await fetch('/api/music/local/revert/' + localDetailsState.trackId, { method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token }) });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    showMusicNotice('Reverted to file tags', false);
    closeLocalDetails();
    if (currentLibView === 'tracks') loadLocalTracks();
  } catch(e) {
    showMusicNotice('Revert failed: ' + (e.message || ''), true);
  }
}

async function fetchLyrics() {
  if (!localDetailsState.trackId) return;
  const panel = document.getElementById('local-lyrics-panel');
  if (panel) panel.innerHTML = '<p class="text-xs text-gray-500 italic">Looking up…</p>';
  try {
    const r = await fetch('/api/music/local/lyrics/' + localDetailsState.trackId + '?token=' + encodeURIComponent(token));
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const d = await r.json();
    if (panel) {
      const text = d.synced_lrc
        // Synced LRC is one line per timestamp — strip the [mm:ss.xx] prefix for display; karaoke sync comes in T3+.
        ? d.synced_lrc.split('\n').map(l => l.replace(/^\[\d{1,2}:\d{1,2}(?:\.\d+)?\]\s*/,'')).filter(Boolean).join('\n')
        : (d.plain_text || '');
      if (text) {
        panel.innerHTML = text.split('\n').map(l => '<div class="line">' + escapeHtml(l) + '</div>').join('');
      } else {
        panel.innerHTML = '<p class="text-xs text-gray-500 italic">' + escapeHtml(d.reason || 'No lyrics found for this track.') + '</p>';
      }
    }
  } catch(e) {
    if (panel) panel.innerHTML = '<p class="text-xs text-red-400">Lookup failed: ' + escapeHtml(e.message || '') + '</p>';
  }
}

async function applyLocalMatch(idx) {
  const m = localDetailsState.matches[idx];
  if (!m || !localDetailsState.trackId) return;
  try {
    const r = await fetch('/api/music/local/match/' + localDetailsState.trackId, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        token, mbid: m.mbid,
        title: m.title || '', artist: m.artist || '',
        album: m.album || null, source: 'musicbrainz',
      }),
    });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    showMusicNotice('\u2713 Updated from MusicBrainz', false);
    closeLocalDetails();
    loadLocalTracks();
  } catch(e) {
    showMusicNotice('Update failed: ' + (e.message || ''), true);
  }
}

async function saveLocalEdit() {
  const t = document.getElementById('local-edit-title').value.trim();
  const a = document.getElementById('local-edit-artist').value.trim();
  const al = document.getElementById('local-edit-album').value.trim();
  if (!t && !a) { showMusicNotice('Need at least a title or artist.', true); return; }
  try {
    const r = await fetch('/api/music/local/match/' + localDetailsState.trackId, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, title: t, artist: a, album: al || null, source: 'user_edit' }),
    });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    showMusicNotice('\u2713 Saved', false);
    closeLocalDetails();
    loadLocalTracks();
  } catch(e) {
    showMusicNotice('Save failed: ' + (e.message || ''), true);
  }
}

// ── Bulk LLM tag cleanup ─────────────────────────────────────────────
async function cleanUpTags() {
  const btn = document.getElementById('local-lib-cleanup');
  const orig = btn ? btn.textContent : '';
  if (btn) { btn.textContent = 'Cleaning…'; btn.disabled = true; }
  try {
    const r = await fetch('/api/music/local/retag_all', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, limit: 100 }),
    });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const d = await r.json();
    showMusicNotice(d.message || ('Updated ' + (d.updated || 0) + ' tracks'), false);
    loadLocalTracks();
  } catch(e) {
    showMusicNotice("Couldn't clean up tags: " + (e.message || ''), true);
  } finally {
    if (btn) { btn.textContent = orig; btn.disabled = false; }
  }
}

// Format millis as mm:ss or h:mm:ss, used by progress + row durations.
function fmtMs(ms) {
  if (!ms || ms < 0) return '0:00';
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  if (h > 0) return h + ':' + String(m).padStart(2,'0') + ':' + String(ss).padStart(2,'0');
  return m + ':' + String(ss).padStart(2,'0');
}

// Wire progress bar scrubbing. Mark .dataset.scrubbing while held so
// timeupdate doesn't fight the user's finger.
(function wireProgress() {
  const prog = document.getElementById('np-progress');
  if (!prog) return;
  prog.addEventListener('input', (e) => {
    prog.dataset.scrubbing = '1';
    prog.style.setProperty('--progress', (e.target.value / 10).toFixed(1) + '%');
  });
  prog.addEventListener('change', (e) => {
    const a = document.getElementById('local-audio');
    if (a && a.duration && isFinite(a.duration)) {
      a.currentTime = (e.target.value / 1000) * a.duration;
    }
    delete prog.dataset.scrubbing;
  });
})();

// Keyboard shortcuts — only active while /music is the active page and
// the user isn't typing in an input/textarea.
(function wireKeyboard() {
  document.addEventListener('keydown', (ev) => {
    const tag = (ev.target.tagName || '').toUpperCase();
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
    const a = document.getElementById('local-audio');
    switch (ev.key) {
      case ' ':
        ev.preventDefault();
        if (a) { if (a.paused) a.play().catch(()=>{}); else a.pause(); }
        break;
      case 'ArrowRight':
        if (ev.shiftKey) { ev.preventDefault(); control('next'); }
        else if (a && a.duration) { ev.preventDefault(); a.currentTime = Math.min(a.duration, a.currentTime + 5); }
        break;
      case 'ArrowLeft':
        if (ev.shiftKey) { ev.preventDefault(); control('prev'); }
        else if (a) { ev.preventDefault(); a.currentTime = Math.max(0, a.currentTime - 5); }
        break;
      case 's': case 'S': toggleShuffle(); break;
      case 'r': case 'R': toggleRepeat(); break;
      case 'l': case 'L': toggleFavorite(); break;
      case 'q': case 'Q': switchLibView(currentLibView === 'playlists' ? 'tracks' : 'playlists'); break;
      case '/':
        ev.preventDefault();
        document.getElementById('local-lib-nl')?.focus();
        break;
    }
  });
})();

// Load on page ready
loadLocalFolders();



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
