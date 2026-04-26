//! /library — Document & Photo library page.
//!
//! Faceted browser over `library_files`. Phase 1 surfaces:
//!   • Inbox  — low-confidence ingests waiting on a kind/vendor/year decision
//!   • All    — everything, default sort by scan_date desc
//!   • Photos — kind=photo, masonry grid, links into face clusters
//!   • Docs   — kind ∈ {receipt, statement, manual, personal_doc, tax_form}
//!   • Tax    — relative_path starting with `tax/`, year picker
//!   • Tags   — tag CRUD + filter (system + user tags)
//!
//! All data load is client-side from /api/library/*. The page is mostly
//! a layout skeleton + JS that talks to the existing endpoints — no new
//! server endpoints are introduced beyond what Phases 1-8 already wired.
//!
//! Persona: Maxine ("the librarian") — calm card-catalog aesthetic, tan
//! linen + ink, minimal chrome. Distinct from Mushi's tea-house warmth
//! and Cortex's pearl-on-charcoal Garamond — this room reads like a
//! reading room.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Library",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        (top_bar_standard("Library"))
        (library_hero())
        (chip_bar())
        div class="lib-shell" {
            aside class="lib-rail" {
                (rail_filters())
                (rail_tags())
                (rail_inbox_count())
            }
            main class="lib-main" id="lib-main" {
                section id="lib-tab-all" class="lib-tab active" {
                    div class="lib-toolbar" {
                        input id="lib-search" type="text" placeholder="Search files…" oninput="onSearch()" {}
                        select id="lib-sort" onchange="onSort()" {
                            option value="date_desc" selected { "Newest first" }
                            option value="date_asc" { "Oldest first" }
                            option value="size_desc" { "Largest first" }
                        }
                        span class="flex-1" {}
                        button class="lib-btn" onclick="openUploader()" { "+ Upload" }
                    }
                    div id="lib-grid" class="lib-grid" {
                        div class="lib-empty" { "Loading…" }
                    }
                }
                section id="lib-tab-inbox" class="lib-tab" {
                    div class="lib-banner" { "Items below were ingested at low confidence — confirm a kind so the system learns." }
                    div id="lib-inbox" class="lib-list" {}
                }
                section id="lib-tab-photos" class="lib-tab" {
                    div class="lib-toolbar" {
                        select id="lib-cluster" onchange="onClusterFilter()" {
                            option value="" selected { "All photos" }
                        }
                        span class="flex-1" {}
                    }
                    div id="lib-photos" class="lib-masonry" {}
                }
                section id="lib-tab-tax" class="lib-tab" {
                    div class="lib-toolbar" {
                        select id="lib-year" onchange="onYearChange()" {}
                        button class="lib-btn" onclick="exportYear()" { "Export year zip" }
                        button class="lib-btn" onclick="shareYear()" { "Share with CPA" }
                    }
                    div id="lib-tax-summary" class="lib-tax-summary" {}
                    div id="lib-tax-grid" class="lib-grid" {}
                }
                section id="lib-tab-audit" class="lib-tab" {
                    h3 { "Audit log" }
                    p class="lib-hint" { "Every read, share, edit, and export is recorded here." }
                    div id="lib-audit" class="lib-audit" {}
                }
            }
        }

        div id="lib-upload-modal" class="lib-modal" hidden {
            div class="lib-modal-card" {
                h3 { "Upload to library" }
                input id="lib-upload-input" type="file" multiple accept="image/*,application/pdf,.txt,.md,.docx" {}
                div id="lib-upload-progress" class="lib-progress" {}
                div class="lib-modal-foot" {
                    button class="lib-btn-ghost" onclick="closeUploader()" { "Cancel" }
                    button class="lib-btn" onclick="runUpload()" { "Ingest" }
                }
            }
        }

        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn library_hero() -> Markup {
    html! {
        div class="lib-hero" {
            div class="lib-hero-inner" {
                span class="lib-mark" aria-hidden="true" { "❘❘" }
                span class="lib-section" { "Library" }
                span class="lib-subtle" { "· Maxine, your librarian." }
                div class="flex-1" {}
                a href="/scheduler" class="lib-link" { "Scheduler" }
                a href="/tax" class="lib-link" { "Tax" }
            }
        }
    }
}

fn chip_bar() -> Markup {
    html! {
        div class="lib-chipbar" {
            button id="lib-chip-all"    class="lib-chip active" onclick="showTab('all')"     { "All" }
            button id="lib-chip-inbox"  class="lib-chip"        onclick="showTab('inbox')"   { "Inbox " span id="lib-inbox-badge" {} }
            button id="lib-chip-photos" class="lib-chip"        onclick="showTab('photos')"  { "Photos" }
            button id="lib-chip-tax"    class="lib-chip"        onclick="showTab('tax')"     { "Tax" }
            button id="lib-chip-audit"  class="lib-chip"        onclick="showTab('audit')"   { "Audit" }
        }
    }
}

fn rail_filters() -> Markup {
    html! {
        section class="lib-rail-section" {
            div class="lib-rail-eyebrow" { "Filter by kind" }
            ul class="lib-kinds" {
                li class="active" data-kind="" onclick="setKind('')"            { "All" }
                li data-kind="receipt"      onclick="setKind('receipt')"       { "Receipts" }
                li data-kind="statement"    onclick="setKind('statement')"     { "Statements" }
                li data-kind="tax_form"     onclick="setKind('tax_form')"      { "Tax forms" }
                li data-kind="manual"       onclick="setKind('manual')"        { "Manuals" }
                li data-kind="personal_doc" onclick="setKind('personal_doc')"  { "Personal" }
                li data-kind="photo"        onclick="setKind('photo')"         { "Photos" }
                li data-kind="unknown"      onclick="setKind('unknown')"       { "Unknown" }
            }
        }
    }
}

fn rail_tags() -> Markup {
    html! {
        section class="lib-rail-section" {
            div class="lib-rail-eyebrow" { "Tags" }
            ul id="lib-tags" class="lib-tags" {}
            button class="lib-tag-add" onclick="addTagPrompt()" { "+ New tag" }
        }
    }
}

fn rail_inbox_count() -> Markup {
    html! {
        section class="lib-rail-section" {
            div class="lib-rail-eyebrow" { "Storage" }
            div id="lib-storage-line" class="lib-storage" { "—" }
        }
    }
}

const EXTRA_STYLE: &str = r#"
:root {
    --lib-bg: #f7f3ec;
    --lib-paper: #fffdf6;
    --lib-ink: #1d1b16;
    --lib-line: #d8cfbe;
    --lib-accent: #8a3324;
    --lib-mute: #6f6753;
}
body { background: var(--lib-bg); color: var(--lib-ink); font-family: 'EB Garamond', 'Georgia', serif; }
.lib-hero { padding: 14px 24px 0; }
.lib-hero-inner { display:flex; align-items:center; gap:10px; }
.lib-section { font-size: 22px; letter-spacing: 0.02em; }
.lib-mark { color: var(--lib-accent); }
.lib-subtle { color: var(--lib-mute); font-style: italic; }
.lib-link { color: var(--lib-mute); text-decoration: none; padding: 4px 10px; border-radius:4px; }
.lib-link:hover { background: var(--lib-paper); }
.flex-1 { flex: 1; }
.lib-chipbar { display:flex; gap:8px; padding: 10px 24px; border-bottom: 1px solid var(--lib-line); background: var(--lib-paper); }
.lib-chip { background: transparent; border: 1px solid var(--lib-line); border-radius: 999px; padding: 6px 14px; font-family: inherit; cursor: pointer; }
.lib-chip.active { background: var(--lib-ink); color: var(--lib-paper); }
.lib-chip:hover:not(.active) { background: #efe9dd; }
.lib-shell { display: grid; grid-template-columns: 240px 1fr; gap: 24px; padding: 18px 24px; min-height: 70vh; }
.lib-rail { background: var(--lib-paper); border: 1px solid var(--lib-line); border-radius: 8px; padding: 14px; height: fit-content; }
.lib-rail-section { margin-bottom: 22px; }
.lib-rail-eyebrow { font-size: 11px; text-transform: uppercase; letter-spacing: 0.1em; color: var(--lib-mute); margin-bottom: 8px; }
.lib-kinds { list-style: none; padding: 0; margin: 0; }
.lib-kinds li { padding: 4px 8px; border-radius: 4px; cursor: pointer; }
.lib-kinds li.active { background: var(--lib-ink); color: var(--lib-paper); }
.lib-kinds li:hover:not(.active) { background: #efe9dd; }
.lib-tags { list-style: none; padding: 0; margin: 0 0 8px; display:flex; flex-wrap: wrap; gap: 4px; }
.lib-tag { display: inline-block; padding: 2px 8px; border: 1px solid var(--lib-line); border-radius: 3px; font-size: 12px; cursor: pointer; }
.lib-tag.system { color: var(--lib-mute); font-style: italic; }
.lib-tag.active { background: var(--lib-accent); color: var(--lib-paper); border-color: var(--lib-accent); }
.lib-tag-add { background: transparent; border: 1px dashed var(--lib-line); color: var(--lib-mute); padding: 2px 8px; border-radius: 3px; font-size: 12px; cursor: pointer; }
.lib-storage { font-size: 13px; color: var(--lib-mute); }
.lib-main { background: var(--lib-paper); border: 1px solid var(--lib-line); border-radius: 8px; padding: 16px; }
.lib-tab { display: none; }
.lib-tab.active { display: block; }
.lib-toolbar { display: flex; align-items: center; gap: 8px; margin-bottom: 14px; }
.lib-toolbar input, .lib-toolbar select { padding: 6px 10px; border: 1px solid var(--lib-line); border-radius: 4px; background: white; font-family: inherit; }
.lib-toolbar input { flex: 1; }
.lib-btn { background: var(--lib-ink); color: var(--lib-paper); padding: 6px 14px; border:none; border-radius: 4px; cursor: pointer; font-family: inherit; }
.lib-btn-ghost { background: transparent; color: var(--lib-ink); padding: 6px 14px; border:1px solid var(--lib-line); border-radius: 4px; cursor: pointer; font-family: inherit; }
.lib-btn:hover { background: var(--lib-accent); }
.lib-banner { background: #f3e9d2; border-left: 3px solid var(--lib-accent); padding: 8px 12px; margin-bottom: 12px; font-style: italic; }
.lib-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 14px; }
.lib-card { border: 1px solid var(--lib-line); border-radius: 6px; overflow: hidden; background: white; cursor: pointer; transition: transform 0.1s; }
.lib-card:hover { transform: translateY(-2px); box-shadow: 0 4px 12px rgba(0,0,0,0.08); }
.lib-card-thumb { aspect-ratio: 4/3; background: #efe9dd; display:flex; align-items:center; justify-content:center; color: var(--lib-mute); font-size: 28px; }
.lib-card-thumb img { width: 100%; height: 100%; object-fit: cover; }
.lib-card-meta { padding: 8px 10px; }
.lib-card-title { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lib-card-sub { font-size: 12px; color: var(--lib-mute); }
.lib-card-actions { padding: 4px 10px 8px; display: flex; gap: 6px; }
.lib-card-actions button { background: transparent; border: 1px solid var(--lib-line); color: var(--lib-ink); padding: 2px 8px; border-radius: 3px; font-size: 11px; cursor: pointer; }
.lib-card-actions button:hover { background: var(--lib-ink); color: var(--lib-paper); }
.lib-list { display: flex; flex-direction: column; gap: 8px; }
.lib-list-row { display: grid; grid-template-columns: 60px 1fr auto auto; gap: 12px; align-items: center; padding: 8px 10px; border: 1px solid var(--lib-line); border-radius: 4px; background: white; }
.lib-masonry { columns: 4; column-gap: 10px; }
.lib-masonry .lib-card { break-inside: avoid; margin-bottom: 10px; }
.lib-empty { color: var(--lib-mute); padding: 40px; text-align: center; font-style: italic; }
.lib-modal { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display:flex; align-items:center; justify-content:center; z-index: 1000; }
.lib-modal[hidden] { display: none; }
.lib-modal-card { background: var(--lib-paper); padding: 24px; border-radius: 8px; min-width: 360px; max-width: 540px; }
.lib-modal-foot { display: flex; gap: 8px; justify-content: flex-end; margin-top: 14px; }
.lib-progress { font-family: monospace; font-size: 12px; max-height: 200px; overflow-y: auto; }
.lib-tax-summary { padding: 12px; background: white; border: 1px solid var(--lib-line); border-radius: 4px; margin-bottom: 14px; }
.lib-audit { font-family: monospace; font-size: 12px; }
.lib-audit-row { padding: 6px 8px; border-bottom: 1px solid var(--lib-line); display: grid; grid-template-columns: 140px 100px 100px 1fr; gap: 8px; }
.lib-hint { color: var(--lib-mute); font-style: italic; margin-bottom: 12px; }
@media (max-width: 900px) {
    .lib-shell { grid-template-columns: 1fr; }
    .lib-masonry { columns: 2; }
}
"#;

const PAGE_JS: &str = r#"
(function() {
  const state = { kind: '', tag: null, search: '', sort: 'date_desc', tab: 'all', cluster: '', year: 0 };

  function token() { return sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || ''; }
  function authHeaders() { const t = token(); return t ? { 'Authorization': 'Bearer ' + t } : {}; }
  async function api(path, opts) {
    opts = opts || {};
    opts.headers = Object.assign({}, opts.headers || {}, authHeaders());
    const r = await fetch(path, opts);
    if (!r.ok) throw new Error('http ' + r.status);
    const ct = r.headers.get('content-type') || '';
    return ct.includes('application/json') ? r.json() : r.text();
  }

  // ── Tabs ────────────────────────────────────────────────
  window.showTab = function(name) {
    state.tab = name;
    document.querySelectorAll('.lib-chip').forEach(c => c.classList.remove('active'));
    document.getElementById('lib-chip-' + name).classList.add('active');
    document.querySelectorAll('.lib-tab').forEach(t => t.classList.remove('active'));
    document.getElementById('lib-tab-' + name).classList.add('active');
    if (name === 'all') loadAll();
    if (name === 'inbox') loadInbox();
    if (name === 'photos') { loadClusters(); loadPhotos(); }
    if (name === 'tax') loadTax();
    if (name === 'audit') loadAudit();
  };

  // ── Filters ─────────────────────────────────────────────
  window.setKind = function(k) {
    state.kind = k;
    document.querySelectorAll('.lib-kinds li').forEach(li => li.classList.toggle('active', li.dataset.kind === k));
    if (state.tab === 'all') loadAll();
  };
  window.onSearch = function() { state.search = document.getElementById('lib-search').value; loadAll(); };
  window.onSort = function() { state.sort = document.getElementById('lib-sort').value; loadAll(); };

  // ── All grid ────────────────────────────────────────────
  async function loadAll() {
    const grid = document.getElementById('lib-grid');
    grid.innerHTML = '<div class="lib-empty">Loading…</div>';
    try {
      const params = new URLSearchParams();
      if (state.kind) params.set('kind', state.kind);
      params.set('limit', '120');
      const data = await api('/api/library/files?' + params.toString());
      let files = data.files || [];
      if (state.search) {
        const q = state.search.toLowerCase();
        files = files.filter(f =>
          (f.original_filename || '').toLowerCase().includes(q) ||
          (f.relative_path || '').toLowerCase().includes(q)
        );
      }
      if (state.sort === 'date_asc') files.sort((a,b) => a.scan_date - b.scan_date);
      if (state.sort === 'size_desc') files.sort((a,b) => b.size_bytes - a.size_bytes);
      if (files.length === 0) { grid.innerHTML = '<div class="lib-empty">No files match.</div>'; return; }
      grid.innerHTML = files.map(renderCard).join('');
    } catch (e) {
      grid.innerHTML = '<div class="lib-empty">Library unavailable.</div>';
    }
  }
  function renderCard(f) {
    const date = f.doc_date || new Date(f.scan_date * 1000).toISOString().slice(0, 10);
    const sz = (f.size_bytes / 1024).toFixed(0) + ' KB';
    const isImage = (f.content_type || '').startsWith('image/');
    const thumb = isImage
      ? '<img loading="lazy" src="/api/library/files/' + f.id + '/content">'
      : (f.kind === 'pdf' || f.content_type === 'application/pdf' ? '📄' : '📎');
    return '<div class="lib-card">'
      + '<div class="lib-card-thumb" onclick="viewFile(' + f.id + ')">' + thumb + '</div>'
      + '<div class="lib-card-meta">'
      + '<div class="lib-card-title">' + escapeHtml(f.original_filename) + '</div>'
      + '<div class="lib-card-sub">' + (f.kind || 'unknown') + ' · ' + date + ' · ' + sz + '</div>'
      + '</div>'
      + '<div class="lib-card-actions">'
      + '<button onclick="viewFile(' + f.id + ')">View</button>'
      + '<button onclick="shareFile(' + f.id + ')">Share</button>'
      + '</div>'
      + '</div>';
  }
  function escapeHtml(s) {
    return (s || '').replace(/[&<>"']/g, m =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m]));
  }
  window.viewFile = function(id) { window.open('/api/library/files/' + id + '/content', '_blank'); };

  // ── Inbox ───────────────────────────────────────────────
  async function loadInbox() {
    const c = document.getElementById('lib-inbox');
    c.innerHTML = '<div class="lib-empty">Loading…</div>';
    try {
      const data = await api('/api/library/inbox');
      const items = data.items || [];
      const badge = document.getElementById('lib-inbox-badge');
      if (badge) badge.textContent = items.length ? '· ' + items.length : '';
      if (items.length === 0) { c.innerHTML = '<div class="lib-empty">Inbox is clear.</div>'; return; }
      c.innerHTML = items.map(it =>
        '<div class="lib-list-row">'
        + '<div>' + (it.suggested_kind || 'unknown') + '</div>'
        + '<div>' + escapeHtml(it.original_filename || '') + '</div>'
        + '<div>' + (it.suggested_confidence || 0).toFixed(2) + '</div>'
        + '<div><button class="lib-btn-ghost" onclick="viewFile(' + it.file_id + ')">View</button> '
        + '<button class="lib-btn" onclick="confirmInbox(' + it.id + ', \'' + (it.suggested_kind || 'unknown') + '\')">Confirm</button></div>'
        + '</div>'
      ).join('');
    } catch (e) { c.innerHTML = '<div class="lib-empty">Inbox unavailable.</div>'; }
  }
  window.confirmInbox = async function(id, kind) {
    try {
      await api('/api/library/inbox/' + id + '/confirm', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind }),
      });
      loadInbox();
      loadAll();
    } catch (_) { alert('Confirm failed.'); }
  };

  // ── Photos ──────────────────────────────────────────────
  async function loadClusters() {
    try {
      const data = await api('/api/library/faces/clusters');
      const sel = document.getElementById('lib-cluster');
      sel.innerHTML = '<option value="">All photos</option>';
      (data.clusters || []).forEach(c => {
        const opt = document.createElement('option');
        opt.value = c.id;
        opt.textContent = (c.name || 'Unnamed cluster') + ' (' + c.photo_count + ')';
        sel.appendChild(opt);
      });
    } catch (_) {}
  }
  async function loadPhotos() {
    const grid = document.getElementById('lib-photos');
    grid.innerHTML = '<div class="lib-empty">Loading…</div>';
    try {
      const data = await api('/api/library/files?kind=photo&limit=200');
      const files = data.files || [];
      if (files.length === 0) { grid.innerHTML = '<div class="lib-empty">No photos yet.</div>'; return; }
      grid.innerHTML = files.map(renderCard).join('');
    } catch (_) { grid.innerHTML = '<div class="lib-empty">Photos unavailable.</div>'; }
  }
  window.onClusterFilter = function() { state.cluster = document.getElementById('lib-cluster').value; loadPhotos(); };

  // ── Tax ─────────────────────────────────────────────────
  async function loadTax() {
    const sel = document.getElementById('lib-year');
    if (!sel.options.length) {
      const now = new Date().getUTCFullYear();
      for (let y = now; y >= now - 7; y--) {
        const o = document.createElement('option'); o.value = y; o.textContent = y; sel.appendChild(o);
      }
      state.year = now;
    }
    const grid = document.getElementById('lib-tax-grid');
    const summary = document.getElementById('lib-tax-summary');
    grid.innerHTML = '<div class="lib-empty">Loading…</div>';
    try {
      const data = await api('/api/library/files?limit=300');
      const files = (data.files || []).filter(f =>
        (f.relative_path || '').startsWith('tax/' + state.year + '/'));
      summary.innerHTML = '<strong>' + state.year + ':</strong> ' + files.length + ' files · '
        + (files.reduce((a, f) => a + (f.size_bytes || 0), 0) / 1024 / 1024).toFixed(1) + ' MB';
      grid.innerHTML = files.length ? files.map(renderCard).join('')
        : '<div class="lib-empty">No tax docs filed for this year yet.</div>';
    } catch (_) { grid.innerHTML = '<div class="lib-empty">Tax view unavailable.</div>'; }
  }
  window.onYearChange = function() {
    state.year = parseInt(document.getElementById('lib-year').value, 10);
    loadTax();
  };
  window.exportYear = async function() {
    const y = state.year;
    if (!y) return;
    window.open('/api/library/tax/' + y + '/export', '_blank');
  };
  window.shareYear = async function() {
    try {
      const r = await api('/api/library/years/' + state.year + '/share', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ expires_in_days: 14, reason: 'CPA review' }),
      });
      prompt('Share link (expires in 14 days):', window.location.origin + r.url);
    } catch (_) { alert('Share failed.'); }
  };

  // ── Audit ───────────────────────────────────────────────
  async function loadAudit() {
    const c = document.getElementById('lib-audit');
    c.innerHTML = '<div class="lib-empty">Loading…</div>';
    try {
      const data = await api('/api/library/audit?limit=300');
      const rows = data.audit || [];
      c.innerHTML = rows.length ? rows.map(r =>
        '<div class="lib-audit-row">'
        + '<div>' + new Date(r.ts * 1000).toISOString() + '</div>'
        + '<div>' + r.action + '</div>'
        + '<div>' + escapeHtml(r.actor) + '</div>'
        + '<div>' + (r.file_id ? 'file ' + r.file_id : '') + ' ' + escapeHtml(r.reason || '') + '</div>'
        + '</div>'
      ).join('') : '<div class="lib-empty">No activity yet.</div>';
    } catch (_) { c.innerHTML = '<div class="lib-empty">Audit log unavailable.</div>'; }
  }

  // ── Tags ────────────────────────────────────────────────
  async function loadTags() {
    try {
      const data = await api('/api/library/tags');
      const ul = document.getElementById('lib-tags');
      ul.innerHTML = (data.tags || []).map(t =>
        '<li class="lib-tag ' + (t.kind === 'system' ? 'system' : '') + '" onclick="filterByTag(' + t.id + ')">'
        + escapeHtml(t.name) + ' <span style="opacity:0.6">(' + t.file_count + ')</span></li>'
      ).join('');
    } catch (_) {}
  }
  window.filterByTag = function(id) { state.tag = id; loadAll(); };
  window.addTagPrompt = async function() {
    const name = prompt('New tag name:'); if (!name) return;
    try {
      await api('/api/library/tags', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      });
      loadTags();
    } catch (_) {}
  };

  // ── Share file ──────────────────────────────────────────
  window.shareFile = async function(id) {
    try {
      const r = await api('/api/library/files/' + id + '/share', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ expires_in_days: 7 }),
      });
      prompt('Share link (expires in 7 days):', window.location.origin + r.url);
    } catch (_) { alert('Share failed.'); }
  };

  // ── Upload ──────────────────────────────────────────────
  window.openUploader = function() { document.getElementById('lib-upload-modal').hidden = false; };
  window.closeUploader = function() { document.getElementById('lib-upload-modal').hidden = true; };
  window.runUpload = async function() {
    const input = document.getElementById('lib-upload-input');
    const prog = document.getElementById('lib-upload-progress');
    if (!input.files || !input.files.length) return;
    prog.innerHTML = '';
    for (const f of input.files) {
      const fd = new FormData(); fd.append('file', f);
      prog.innerHTML += '<div>↑ ' + escapeHtml(f.name) + '…</div>';
      try {
        const r = await fetch('/api/library/ingest', { method: 'POST', headers: authHeaders(), body: fd });
        const j = await r.json();
        prog.innerHTML += '<div style="color:#3a6">  ✓ ' + j.kind + ' (conf ' + (j.confidence || 0).toFixed(2) + ')</div>';
      } catch (e) { prog.innerHTML += '<div style="color:#a33">  ✗ failed</div>'; }
    }
    setTimeout(() => { closeUploader(); loadAll(); loadInbox(); loadTags(); }, 800);
  };

  // ── Storage line ────────────────────────────────────────
  async function loadStorage() {
    try {
      const data = await api('/api/library/files?limit=10000');
      const files = data.files || [];
      const tot = files.reduce((a, f) => a + (f.size_bytes || 0), 0);
      document.getElementById('lib-storage-line').textContent =
        files.length + ' files · ' + (tot / 1024 / 1024).toFixed(1) + ' MB';
    } catch (_) {}
  }

  // ── Autosave registration ───────────────────────────────
  // Register the search box + filter state with the autosave hook so a
  // mid-session restart doesn't wipe what the user was filtering for.
  if (window.SyntaurAutosave) {
    SyntaurAutosave.register('library', 'filters', () => ({
      kind: state.kind, search: state.search, sort: state.sort, tab: state.tab,
    }));
  }

  // Boot.
  loadAll(); loadTags(); loadStorage(); loadInbox();
})();
"#;
