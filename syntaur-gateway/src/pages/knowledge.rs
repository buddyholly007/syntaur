//! /knowledge — document index browser, uploader, and search UI.
//!
//! Backed by the `Indexer` (FTS5 + vector embeddings) and the connector
//! framework. Shows per-source stats, lets the user upload new documents
//! (PDF / plain text), and runs ad-hoc searches against the hybrid index.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Knowledge",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    // Custom in-page top bar instead of the shared one — the library
    // theme needs parchment + sepia tones, not the standard gray.
    let body = html! {
        (page_body())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        // Background image is set in CSS via body::before
        // (see EXTRA_STYLE → /library-bg.webp). The hand-authored SVG
        // watermark was removed when we got the photographic backdrop
        // — kept the const around in source comments for reference.

        // ── Top bar — leather-bound spine ──────────────────────────────
        div class="lib-topbar" {
            div class="lib-topbar-inner" {
                div class="flex items-center gap-3 min-w-0" {
                    a href="/" class="flex items-center gap-2 hover:opacity-80 flex-shrink-0" {
                        img src="/app-icon.jpg" class="h-8 w-8 rounded" alt="";
                        span class="lib-brand" { "Syntaur" }
                    }
                    span class="lib-fleuron" aria-hidden="true" { "❦" }
                    span class="lib-section-label" { "Codex Knowledge" }
                }
                div class="flex items-center gap-4 text-sm" {
                    a href="/" class="lib-link" { "Home" }
                    a href="/settings" class="lib-link" { "Settings" }
                    a href="/profile" class="lib-link" { "Profile" }
                    label class="lib-inline-label" { "Agent" }
                    select id="agent-filter" class="lib-select" onchange="onAgentChange()" {
                        option value="" { "All agents" }
                    }
                }
            }
        }

        // ── Body — 60/40 split, mirroring dashboard layout ─────────────
        div class="lib-body" {
            // LEFT (60%) — the inquiry: search + results
            div class="lib-left" {
                section class="lib-card" {
                    div class="lib-card-eyebrow" { "Inquiry" }
                    h2 class="lib-card-title" { "Search the Codex" }
                    div class="lib-divider" aria-hidden="true" {}
                    div class="lib-search-row" {
                        input type="text" id="search-q"
                            class="lib-input"
                            placeholder="Pose a question of the index…"
                            onkeydown="if(event.key==='Enter')runSearch()";
                        select id="search-source" class="lib-select" {
                            option value="" { "all sources" }
                        }
                        button onclick="runSearch()" class="lib-btn-primary" { "Inquire" }
                    }
                    div id="search-results" class="mt-5 space-y-3" {}
                }
            }

            // RIGHT (40%) — the reference shelf
            div class="lib-right" {
                // Stats — three illuminated medallions
                div class="grid grid-cols-3 gap-3" {
                    div class="lib-stat" {
                        div class="lib-stat-label" { "Folios" }
                        div class="lib-stat-value" id="stat-docs" { "·" }
                    }
                    div class="lib-stat" {
                        div class="lib-stat-label" { "Passages" }
                        div class="lib-stat-value" id="stat-chunks" { "·" }
                    }
                    div class="lib-stat" {
                        div class="lib-stat-label" { "Vaults" }
                        div class="lib-stat-value" id="stat-sources" { "·" }
                    }
                }

                // Upload — a scribe's in-tray
                section class="lib-card" {
                    div class="lib-card-eyebrow" { "Add a folio" }
                    h2 class="lib-card-title" { "Inscribe new knowledge" }
                    div class="lib-divider" aria-hidden="true" {}
                    p class="lib-prose" {
                        "PDF, DOCX, XLSX, PPTX, ODT, ODS, EPUB, RTF, EML, CSV, JSON, "
                        "YAML, Markdown, HTML, source code, and plain text. The text is "
                        "extracted, divided into passages, embedded, and added to the "
                        "index without delay."
                    }
                    div class="flex items-center gap-2 mt-3 mb-3" {
                        label class="lib-inline-label" { "Place into" }
                        select id="upload-agent" class="lib-select" {
                            option value="shared" { "Shared (all agents)" }
                        }
                    }
                    div id="drop-zone"
                        class="lib-drop-zone"
                        onclick="document.getElementById('file-input').click()" {
                        svg class="lib-drop-quill" width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" {
                            // Stylized quill — generic clip-art convention, not anyone's IP
                            path d="M3 21l3-3M6 18c4-4 9-9 13-13l2 2C17 11 12 16 8 20l-2-2z";
                            path d="M14 6l2 2";
                        }
                        div class="lib-drop-headline" { "Place a folio here" }
                        div class="lib-drop-sub" { "Click, or set it down upon this tray" }
                        input type="file" id="file-input" multiple class="hidden" onchange="handleFiles(this.files)";
                    }
                    div id="upload-status" class="mt-3 space-y-1 text-sm" {}
                }

                // Sources — the catalog
                section class="lib-card" {
                    div class="flex items-center justify-between" {
                        div {
                            div class="lib-card-eyebrow" { "Catalog" }
                            h2 class="lib-card-title" { "Connectors & vaults" }
                        }
                        button onclick="loadStats()" class="lib-btn-ghost" { "Refresh" }
                    }
                    div class="lib-divider" aria-hidden="true" {}
                    div id="sources-list" class="space-y-2" {}
                }

                // Recent docs — the day's intake
                section class="lib-card" {
                    div class="flex items-center justify-between" {
                        div {
                            div class="lib-card-eyebrow" { "Recently inscribed" }
                            h2 class="lib-card-title" { "New folios" }
                        }
                        select id="docs-source-filter" class="lib-select" onchange="loadDocs()" {
                            option value="" { "all vaults" }
                        }
                    }
                    div class="lib-divider" aria-hidden="true" {}
                    div id="docs-list" class="space-y-2" {}
                }
            }
        }
    }
}

const EXTRA_STYLE: &str = r##"
  /* EB Garamond — Open Font License (free) — Renaissance serif. Used for
     all display + body text on the Knowledge page so it reads like a
     scholar's notebook, not the rest of the dashboard. */
  @import url('https://fonts.googleapis.com/css2?family=EB+Garamond:ital,wght@0,400;0,500;0,600;0,700;1,400;1,500&display=swap');

  /* ── Library palette ─────────────────────────────────────────────
     Aged leather background; parchment-cream cards; sepia ink for text;
     gold + burgundy for accents. Dark theme but warm — candlelit study,
     not a back-lit screen. */
  :root {
    --lib-bg:        #1d1408;
    --lib-bg-2:      #251a0c;
    --lib-paper:     #f4ead5;
    --lib-paper-2:   #ede1c4;
    --lib-paper-edge: #d9c89e;
    --lib-ink:       #2a1f0e;
    --lib-ink-mute:  #6b5a3e;
    --lib-ink-faint: #a08e6a;
    --lib-gold:      #c89b3c;
    --lib-gold-soft: rgba(200, 155, 60, 0.4);
    --lib-burgundy:  #762a2a;
    --lib-line:      #b8a578;
    --lib-rule:      rgba(106, 84, 42, 0.35);
  }

  body {
    background: var(--lib-bg);
    color: var(--lib-paper);
    font-family: 'EB Garamond', 'Iowan Old Style', Georgia, serif;
  }
  /* Full-bleed photographic background — Renaissance-alchemist scholar's
     desk image, fixed to the viewport so it doesn't scroll with content.
     Two darkening overlays keep the cards readable: a deep tint over the
     whole image plus a stronger center-band gradient where the cards land. */
  body::before {
    content: '';
    position: fixed;
    inset: 0;
    z-index: 0;
    background-image:
      /* center-band darken so cards have a calmer surface to sit on */
      linear-gradient(to bottom, rgba(7,4,1,0.30) 0%, rgba(7,4,1,0.55) 35%, rgba(7,4,1,0.55) 65%, rgba(7,4,1,0.30) 100%),
      /* the actual scene */
      url('/library-bg.webp');
    background-size: cover, cover;
    background-position: center, center;
    background-repeat: no-repeat, no-repeat;
    pointer-events: none;
  }
  /* Extra warm vignette over everything — pulls the eye toward center. */
  body::after {
    content: '';
    position: fixed;
    inset: 0;
    z-index: 0;
    pointer-events: none;
    background: radial-gradient(ellipse 100% 80% at center, transparent 0%, transparent 40%, rgba(7,4,1,0.5) 100%);
  }

  /* ── Top bar — leather spine with gold tooling ──────────────────── */
  .lib-topbar {
    border-bottom: 1px solid var(--lib-gold);
    background: linear-gradient(180deg, #1d1408 0%, #110a04 100%);
    box-shadow: 0 1px 0 rgba(200,155,60,0.25), 0 4px 12px rgba(0,0,0,0.5);
    position: sticky; top: 0; z-index: 40;
  }
  .lib-topbar-inner {
    max-width: 1280px;
    margin: 0 auto;
    padding: 12px 24px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .lib-brand {
    font-family: 'EB Garamond', serif;
    font-weight: 600;
    font-size: 18px;
    letter-spacing: 0.04em;
    color: var(--lib-paper);
  }
  .lib-fleuron {
    font-size: 18px;
    color: var(--lib-gold);
    margin: 0 4px;
  }
  .lib-section-label {
    font-family: 'EB Garamond', serif;
    font-weight: 500;
    font-style: italic;
    font-size: 17px;
    color: var(--lib-paper-edge);
    letter-spacing: 0.02em;
  }
  .lib-link {
    font-family: 'EB Garamond', serif;
    color: var(--lib-paper-edge);
    text-decoration: none;
    transition: color 0.15s;
  }
  .lib-link:hover { color: var(--lib-gold); }

  /* ── Body shell + 60/40 split ─────────────────────────────────── */
  .lib-body {
    display: flex;
    height: calc(100vh - 53px);
    max-width: 1280px;
    margin: 0 auto;
  }
  .lib-left  { width: 60%; overflow-y: auto; padding: 24px 18px 24px 24px; }
  .lib-right { width: 40%; overflow-y: auto; padding: 24px 24px 24px 18px; display: flex; flex-direction: column; gap: 16px; }
  .lib-left  > section { margin-bottom: 16px; }

  /* Custom scrollbars — sepia thumbs to match the theme. */
  .lib-left::-webkit-scrollbar, .lib-right::-webkit-scrollbar { width: 8px; }
  .lib-left::-webkit-scrollbar-track, .lib-right::-webkit-scrollbar-track { background: transparent; }
  .lib-left::-webkit-scrollbar-thumb, .lib-right::-webkit-scrollbar-thumb { background: var(--lib-ink-faint); border-radius: 4px; }
  .lib-left::-webkit-scrollbar-thumb:hover, .lib-right::-webkit-scrollbar-thumb:hover { background: var(--lib-gold); }

  /* ── Cards — parchment sheets ─────────────────────────────────── */
  .lib-card {
    background: var(--lib-paper);
    background-image:
      radial-gradient(ellipse at 20% 30%, rgba(216,194,148,0.4), transparent 50%),
      radial-gradient(ellipse at 80% 70%, rgba(216,194,148,0.3), transparent 55%);
    color: var(--lib-ink);
    padding: 22px 26px;
    /* Deckled-edge hint via thin border + inner shadow + outer drop. */
    border: 1px solid var(--lib-paper-edge);
    box-shadow:
      0 1px 0 rgba(255,255,255,0.4) inset,
      0 0 0 4px rgba(244,234,213,0.05),
      0 6px 14px rgba(0,0,0,0.45);
    border-radius: 2px;
    position: relative;
  }
  .lib-card-eyebrow {
    font-family: 'EB Garamond', serif;
    font-style: italic;
    font-size: 12px;
    color: var(--lib-burgundy);
    letter-spacing: 0.08em;
    text-transform: lowercase;
  }
  .lib-card-title {
    font-family: 'EB Garamond', serif;
    font-weight: 600;
    font-size: 22px;
    color: var(--lib-ink);
    margin-top: 2px;
    letter-spacing: 0.005em;
  }
  /* Divider: gold rule with a centered fleuron — gives a manuscript
     section break feel instead of the usual flat horizontal line. */
  .lib-divider {
    margin: 14px 0 18px 0;
    height: 12px;
    background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='200' height='12' viewBox='0 0 200 12'><line x1='0' y1='6' x2='86' y2='6' stroke='%23c89b3c' stroke-width='0.6'/><line x1='114' y1='6' x2='200' y2='6' stroke='%23c89b3c' stroke-width='0.6'/><text x='100' y='10' font-family='Georgia' font-size='12' fill='%23c89b3c' text-anchor='middle'>❦</text></svg>");
    background-repeat: no-repeat;
    background-position: center;
    background-size: 100% 12px;
  }
  .lib-prose {
    font-family: 'EB Garamond', serif;
    font-size: 15px;
    line-height: 1.55;
    color: var(--lib-ink);
  }
  .lib-prose-mute { color: var(--lib-ink-mute); }

  /* ── Stats — illuminated medallions ───────────────────────────── */
  .lib-stat {
    background: var(--lib-paper);
    border: 1px solid var(--lib-paper-edge);
    color: var(--lib-ink);
    padding: 14px 12px;
    text-align: center;
    box-shadow: 0 4px 8px rgba(0,0,0,0.35);
    border-radius: 2px;
    position: relative;
  }
  .lib-stat-label {
    font-family: 'EB Garamond', serif;
    font-style: italic;
    font-size: 11px;
    color: var(--lib-burgundy);
    letter-spacing: 0.1em;
    text-transform: lowercase;
  }
  .lib-stat-value {
    font-family: 'EB Garamond', serif;
    font-weight: 600;
    font-size: 28px;
    color: var(--lib-ink);
    margin-top: 4px;
    line-height: 1;
  }

  /* ── Inputs / forms ───────────────────────────────────────────── */
  .lib-search-row { display: flex; gap: 8px; }
  .lib-input {
    flex: 1;
    background: #fffaf0 !important;
    border: 1px solid var(--lib-line) !important;
    border-radius: 2px !important;
    padding: 9px 12px;
    font-family: 'EB Garamond', serif;
    font-size: 16px;
    color: var(--lib-ink);
    box-shadow: 0 1px 2px rgba(0,0,0,0.1) inset;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .lib-input::placeholder { color: var(--lib-ink-faint); font-style: italic; }
  .lib-input:focus {
    border-color: var(--lib-gold) !important;
    outline: none;
    box-shadow: 0 0 0 2px var(--lib-gold-soft), 0 1px 2px rgba(0,0,0,0.1) inset;
  }
  .lib-select {
    appearance: none;
    -webkit-appearance: none;
    background: #fffaf0 !important;
    border: 1px solid var(--lib-line) !important;
    border-radius: 2px !important;
    color: var(--lib-ink) !important;
    font-family: 'EB Garamond', serif !important;
    font-size: 14px !important;
    padding: 7px 26px 7px 10px !important;
    background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 10 10' fill='none' stroke='%236b5a3e' stroke-width='1.5'><path d='M2 4l3 3 3-3'/></svg>") !important;
    background-repeat: no-repeat !important;
    background-position: right 8px center !important;
    background-size: 10px 10px !important;
    cursor: pointer;
    transition: border-color 0.15s;
  }
  .lib-select:hover, .lib-select:focus {
    border-color: var(--lib-gold) !important;
    outline: none !important;
  }
  .lib-select option { background: #fffaf0; color: var(--lib-ink); }

  /* Top-bar variant of the agent filter — sepia on dark leather.
     Use background-color (longhand) instead of `background:` shorthand —
     the shorthand resets background-repeat to its default (`repeat`),
     which would tile the dropdown's caret SVG across the whole control. */
  .lib-topbar .lib-select {
    background-color: rgba(244,234,213,0.06) !important;
    color: var(--lib-paper) !important;
    border-color: var(--lib-ink-faint) !important;
    background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 10 10' fill='none' stroke='%23c89b3c' stroke-width='1.5'><path d='M2 4l3 3 3-3'/></svg>") !important;
  }
  .lib-topbar .lib-select option { background: #1d1408; color: var(--lib-paper); }

  .lib-inline-label {
    font-family: 'EB Garamond', serif;
    font-style: italic;
    font-size: 13px;
    color: var(--lib-ink-mute);
  }
  .lib-topbar .lib-inline-label { color: var(--lib-paper-edge); }

  /* ── Buttons — wax-seal primary, ghost secondary ──────────────── */
  .lib-btn-primary {
    background: linear-gradient(180deg, var(--lib-burgundy) 0%, #5e2020 100%);
    color: #f4ead5;
    border: 1px solid #4a1818;
    border-radius: 2px;
    padding: 8px 18px;
    font-family: 'EB Garamond', serif;
    font-size: 15px;
    font-weight: 500;
    letter-spacing: 0.04em;
    cursor: pointer;
    box-shadow:
      0 1px 0 rgba(255,255,255,0.15) inset,
      0 2px 4px rgba(0,0,0,0.3);
    transition: all 0.15s;
  }
  .lib-btn-primary:hover {
    background: linear-gradient(180deg, #8a3232 0%, var(--lib-burgundy) 100%);
    box-shadow:
      0 1px 0 rgba(255,255,255,0.2) inset,
      0 3px 6px rgba(0,0,0,0.4);
  }
  .lib-btn-ghost {
    font-family: 'EB Garamond', serif;
    font-style: italic;
    font-size: 13px;
    color: var(--lib-ink-mute);
    background: transparent;
    border: none;
    cursor: pointer;
    padding: 4px 8px;
    transition: color 0.15s;
  }
  .lib-btn-ghost:hover { color: var(--lib-burgundy); }

  /* ── Drop zone — a scribe's tray ──────────────────────────────── */
  .lib-drop-zone {
    border: 1px dashed var(--lib-line);
    border-radius: 2px;
    padding: 24px;
    text-align: center;
    cursor: pointer;
    background: rgba(255,250,240,0.5);
    color: var(--lib-ink-mute);
    transition: all 0.15s;
  }
  .lib-drop-zone:hover {
    border-color: var(--lib-gold);
    background: rgba(255,250,240,0.85);
    color: var(--lib-burgundy);
  }
  .lib-drop-quill {
    color: var(--lib-burgundy);
    margin: 0 auto 4px;
    display: block;
    opacity: 0.8;
  }
  .lib-drop-headline {
    font-family: 'EB Garamond', serif;
    font-size: 16px;
    color: var(--lib-ink);
    margin-bottom: 2px;
  }
  .lib-drop-sub {
    font-family: 'EB Garamond', serif;
    font-style: italic;
    font-size: 13px;
    color: var(--lib-ink-faint);
  }

  /* ── Search results / source rows / docs rows ─────────────────── */
  /* These get populated by the existing JS as plain HTML — we override
     the gray-on-dark default colors so they read on parchment. */
  .lib-card #search-results > *,
  .lib-card #sources-list > *,
  .lib-card #docs-list > * {
    color: var(--lib-ink) !important;
    border-color: var(--lib-rule) !important;
  }
  .hit-snippet {
    font-family: 'EB Garamond', serif;
    font-size: 15px;
    line-height: 1.5;
    color: var(--lib-ink) !important;
  }
  .hit-snippet mark {
    background: rgba(200,155,60,0.35);
    color: var(--lib-ink);
    padding: 0 2px;
    border-radius: 2px;
    font-weight: 500;
  }
  /* Anything Tailwind-classed inside the populated lists gets the
     sepia palette. The JS uses .text-gray-400, .text-gray-500, etc.
     A few !important overrides keep us from touching the JS. */
  .lib-card .text-gray-400,
  .lib-card .text-gray-500 { color: var(--lib-ink-mute) !important; }
  .lib-card .text-gray-300,
  .lib-card .text-gray-200 { color: var(--lib-ink) !important; }
  .lib-card .text-white     { color: var(--lib-ink) !important; }
  .lib-card .bg-gray-800,
  .lib-card .bg-gray-900    { background: var(--lib-paper-2) !important; }
  .lib-card .border-gray-700,
  .lib-card .border-gray-800 { border-color: var(--lib-rule) !important; }

  /* All real content sits above the photographic background image
     (see body::before / body::after rules) and its darkening overlays. */
  .lib-topbar, .lib-body { position: relative; z-index: 1; }
"##;

const PAGE_JS: &str = r#"
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }

const q = (sel) => document.querySelector(sel);
const el = (html) => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstChild; };
const esc = (s) => String(s || '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const fmtTime = (iso) => { if (!iso) return '—'; try { return new Date(iso).toLocaleString(); } catch { return iso; } };
const fmtRelative = (iso) => {
  if (!iso) return 'never';
  try {
    const then = new Date(iso).getTime();
    const diff = Math.round((Date.now() - then) / 1000);
    if (diff < 60) return diff + 's ago';
    if (diff < 3600) return Math.round(diff / 60) + 'm ago';
    if (diff < 86400) return Math.round(diff / 3600) + 'h ago';
    return Math.round(diff / 86400) + 'd ago';
  } catch { return iso; }
};

function getAgent() { return q('#agent-filter').value; }

async function apiGet(path) {
  let url = path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token);
  const agent = getAgent();
  if (agent) url += '&agent=' + encodeURIComponent(agent);
  const r = await fetch(url);
  if (r.status === 401) { sessionStorage.removeItem('syntaur_token'); window.location.href = '/'; return null; }
  return r.json();
}

async function apiPost(path, body) {
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token, ...body }),
  });
  return r.json();
}

function onAgentChange() {
  loadStats();
  loadDocs();
}

async function loadStats() {
  try {
    const data = await apiGet('/api/knowledge/stats');
    if (!data) return;
    q('#stat-docs').textContent = (data.documents || 0).toLocaleString();
    q('#stat-chunks').textContent = (data.chunks || 0).toLocaleString();
    q('#stat-sources').textContent = (data.sources || 0).toLocaleString();

    // Populate source dropdowns (search + docs filter)
    const sources = (data.per_source || []).map(s => s.name);
    for (const selId of ['#search-source', '#docs-source-filter']) {
      const sel = q(selId);
      const cur = sel.value;
      sel.innerHTML = '<option value="">all sources</option>' +
        sources.map(s => `<option value="${esc(s)}">${esc(s)}</option>`).join('');
      if (cur && sources.includes(cur)) sel.value = cur;
    }

    // Sources list
    const list = q('#sources-list');
    if (!data.per_source || data.per_source.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-500">No sources registered yet.</p>';
    } else {
      list.innerHTML = data.per_source.map(s => `
        <div class="flex items-center justify-between bg-gray-900 rounded-lg p-3">
          <div class="min-w-0 flex-1">
            <div class="font-medium text-sm">${esc(s.name)}</div>
            <div class="text-xs text-gray-500 mt-0.5">
              ${(s.documents || 0).toLocaleString()} docs · last refresh ${fmtRelative(s.last_refresh)}
            </div>
          </div>
          <button onclick="resync('${esc(s.name)}', this)"
                  class="text-xs bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded-lg px-3 py-1.5">
            Re-sync
          </button>
        </div>
      `).join('');
    }
  } catch(e) {
    console.error('stats:', e);
  }
}

async function resync(name, btn) {
  const orig = btn.textContent;
  btn.disabled = true;
  btn.textContent = 'Running…';
  try {
    const r = await apiPost('/api/knowledge/resync/' + encodeURIComponent(name), {});
    if (r.error) {
      btn.textContent = 'Error';
      console.error('resync:', r.error);
    } else {
      btn.textContent = `✓ ${r.indexed || 0} docs`;
    }
    setTimeout(() => { btn.disabled = false; btn.textContent = orig; loadStats(); loadDocs(); }, 1500);
  } catch(e) {
    btn.disabled = false;
    btn.textContent = orig;
  }
}

async function runSearch() {
  const qtext = q('#search-q').value.trim();
  const src = q('#search-source').value;
  const out = q('#search-results');
  if (!qtext) { out.innerHTML = ''; return; }
  out.innerHTML = '<p class="text-sm text-gray-500">Searching…</p>';
  const params = new URLSearchParams({ q: qtext, k: '10' });
  if (src) params.set('source', src);
  const data = await apiGet('/api/knowledge/search?' + params.toString());
  if (!data) return;
  if (data.error) { out.innerHTML = `<p class="text-sm text-red-400">${esc(data.error)}</p>`; return; }
  const hits = data.hits || [];
  if (hits.length === 0) {
    out.innerHTML = '<p class="text-sm text-gray-500">No matches.</p>';
    return;
  }
  out.innerHTML = hits.map((h, i) => `
    <div class="bg-gray-900 rounded-lg p-3 border border-gray-800">
      <div class="flex items-center justify-between gap-2 mb-1">
        <div class="font-medium text-sm truncate">${i + 1}. ${esc(h.title || h.external_id)}</div>
        <span class="badge badge-gray text-xs flex-shrink-0">${esc(h.source)}</span>
      </div>
      <div class="hit-snippet">${renderSnippet(h.snippet)}</div>
      <div class="text-xs text-gray-600 mt-1">rank ${(h.rank || 0).toFixed(3)} · ${esc(h.external_id)}</div>
    </div>
  `).join('');
}

function renderSnippet(raw) {
  // FTS5 snippet uses <<...>> markers around matches; convert to <mark>.
  return esc(raw || '').replace(/&lt;&lt;/g, '<mark>').replace(/&gt;&gt;/g, '</mark>');
}

async function loadDocs() {
  const src = q('#docs-source-filter').value;
  const out = q('#docs-list');
  const params = new URLSearchParams({ limit: '25' });
  if (src) params.set('source', src);
  const data = await apiGet('/api/knowledge/docs?' + params.toString());
  if (!data) return;
  const docs = data.documents || [];
  if (docs.length === 0) {
    out.innerHTML = '<p class="text-sm text-gray-500">No documents indexed yet.</p>';
    return;
  }
  out.innerHTML = docs.map(d => `
    <div class="flex items-center justify-between bg-gray-900 rounded-lg p-3">
      <div class="min-w-0 flex-1">
        <div class="font-medium text-sm truncate">${esc(d.title || d.external_id)}</div>
        <div class="text-xs text-gray-500 mt-0.5">
          <span class="badge badge-gray mr-1">${esc(d.source)}</span>
          indexed ${fmtRelative(d.indexed_at)} · ${(d.chunks || 0)} chunks
        </div>
      </div>
      ${d.source === 'uploaded_files'
        ? `<button onclick="deleteDoc(${d.id}, this)" class="text-xs text-red-400 hover:text-red-300 px-2 py-1">Delete</button>`
        : ''}
    </div>
  `).join('');
}

async function deleteDoc(id, btn) {
  if (!confirm('Delete this document from the index and disk?')) return;
  btn.disabled = true; btn.textContent = '…';
  const r = await apiPost('/api/knowledge/docs/delete', { doc_id: id });
  if (r.error) { btn.disabled = false; btn.textContent = 'Delete'; alert(r.error); return; }
  loadStats();
  loadDocs();
}

async function handleFiles(files) {
  const status = q('#upload-status');
  for (const file of files) {
    const row = el(`<div class="flex items-center gap-2 text-gray-400">
      <span class="truncate flex-1">${esc(file.name)}</span>
      <span class="flex-shrink-0">uploading…</span>
    </div>`);
    status.prepend(row);
    const fd = new FormData();
    fd.append('token', token);
    fd.append('agent_id', q('#upload-agent').value || 'shared');
    fd.append('file', file);
    try {
      const r = await fetch('/api/knowledge/upload', { method: 'POST', body: fd });
      const data = await r.json();
      if (data.ok) {
        row.lastElementChild.textContent = `✓ ${data.chunks || 0} chunks`;
        row.lastElementChild.className = 'flex-shrink-0 text-green-400';
      } else {
        row.lastElementChild.textContent = '✗ ' + (data.error || 'failed');
        row.lastElementChild.className = 'flex-shrink-0 text-red-400';
      }
    } catch(e) {
      row.lastElementChild.textContent = '✗ ' + e.message;
      row.lastElementChild.className = 'flex-shrink-0 text-red-400';
    }
  }
  loadStats();
  loadDocs();
}

// Drag & drop
(function() {
  const dz = q('#drop-zone');
  dz.addEventListener('dragover', e => { e.preventDefault(); dz.classList.add('border-oc-600', 'bg-gray-900'); });
  dz.addEventListener('dragleave', e => { e.preventDefault(); dz.classList.remove('border-oc-600', 'bg-gray-900'); });
  dz.addEventListener('drop', e => {
    e.preventDefault();
    dz.classList.remove('border-oc-600', 'bg-gray-900');
    if (e.dataTransfer && e.dataTransfer.files) handleFiles(e.dataTransfer.files);
  });
})();

// Populate agent dropdowns from /health
(async function() {
  try {
    const h = await (await fetch('/health')).json();
    const agents = (h.agents || []).map(a => a.id);
    for (const selId of ['#agent-filter', '#upload-agent']) {
      const sel = q(selId);
      const isUpload = selId === '#upload-agent';
      const base = isUpload
        ? '<option value="shared">Shared (all agents)</option>'
        : '<option value="">All agents</option>';
      sel.innerHTML = base + agents.map(a =>
        `<option value="${esc(a)}">${esc(a)}</option>`
      ).join('');
    }
  } catch {}
  loadStats();
  loadDocs();
})();
"#;
