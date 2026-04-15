//! /knowledge — document index browser, uploader, and search UI.
//!
//! Backed by the `Indexer` (FTS5 + vector embeddings) and the connector
//! framework. Shows per-source stats, lets the user upload new documents
//! (PDF / plain text), and runs ad-hoc searches against the hybrid index.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Knowledge",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (top_bar_standard("Knowledge"))
        (page_body())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        div class="max-w-5xl mx-auto px-4 py-6 space-y-6" {
            div class="flex items-center justify-between" {
                div {
                    h1 class="text-2xl font-bold" { "Knowledge" }
                    p class="text-gray-400 mt-1 text-sm" {
                        "Search and manage the documents Syntaur has indexed. "
                        "Connectors run in the background; uploads are ingested immediately."
                    }
                }
                div class="flex items-center gap-2" {
                    label class="text-xs text-gray-500" { "Agent:" }
                    select id="agent-filter"
                        class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-sm text-gray-300"
                        onchange="onAgentChange()" {
                        option value="" { "All agents" }
                    }
                }
            }

            // ── Stats row ─────────────────────────────────────────────
            div class="grid grid-cols-3 gap-4" {
                div class="card p-4" {
                    div class="text-xs text-gray-500 uppercase tracking-wider" { "Documents" }
                    div class="text-2xl font-semibold mt-1" id="stat-docs" { "…" }
                }
                div class="card p-4" {
                    div class="text-xs text-gray-500 uppercase tracking-wider" { "Chunks" }
                    div class="text-2xl font-semibold mt-1" id="stat-chunks" { "…" }
                }
                div class="card p-4" {
                    div class="text-xs text-gray-500 uppercase tracking-wider" { "Sources" }
                    div class="text-2xl font-semibold mt-1" id="stat-sources" { "…" }
                }
            }

            // ── Search card ───────────────────────────────────────────
            div class="card p-5" {
                h2 class="text-sm font-medium text-gray-300 mb-3" { "Search" }
                div class="flex gap-2" {
                    input type="text" id="search-q"
                        class="flex-1 bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-white placeholder-gray-500 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
                        placeholder="Search the index…"
                        onkeydown="if(event.key==='Enter')runSearch()";
                    select id="search-source" class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-300" {
                        option value="" { "all sources" }
                    }
                    button onclick="runSearch()"
                        class="bg-oc-600 hover:bg-oc-700 text-white font-medium px-4 py-2 rounded-lg transition-colors" {
                        "Search"
                    }
                }
                div id="search-results" class="mt-4 space-y-3" {}
            }

            // ── Upload card ───────────────────────────────────────────
            div class="card p-5" {
                h2 class="text-sm font-medium text-gray-300 mb-1" { "Upload a document" }
                p class="text-xs text-gray-500 mb-3" {
                    "Supported: PDF, DOCX, XLSX, PPTX, ODT, ODS, EPUB, RTF, EML, "
                    "CSV, JSON, YAML, Markdown, HTML, source code (60+ languages), "
                    "and plain text. Extracted text is chunked, embedded, and added to the index immediately."
                }
                div class="flex items-center gap-2 mb-3" {
                    label class="text-xs text-gray-500" { "Upload to:" }
                    select id="upload-agent"
                        class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-xs text-gray-300" {
                        option value="shared" { "Shared (all agents)" }
                    }
                }
                div id="drop-zone"
                    class="border-2 border-dashed border-gray-700 rounded-xl p-6 text-center cursor-pointer hover:border-oc-600 transition-colors"
                    onclick="document.getElementById('file-input').click()" {
                    div class="text-4xl mb-2" { "⬆" }
                    div class="text-sm text-gray-400" { "Click or drop files here" }
                    input type="file" id="file-input" multiple class="hidden" onchange="handleFiles(this.files)";
                }
                div id="upload-status" class="mt-3 space-y-1 text-sm" {}
            }

            // ── Sources card ──────────────────────────────────────────
            div class="card p-5" {
                div class="flex items-center justify-between mb-3" {
                    h2 class="text-sm font-medium text-gray-300" { "Connectors & sources" }
                    button onclick="loadStats()" class="text-xs text-gray-500 hover:text-gray-300" {
                        "Refresh"
                    }
                }
                div id="sources-list" class="space-y-2" {}
            }

            // ── Recent docs card ──────────────────────────────────────
            div class="card p-5" {
                div class="flex items-center justify-between mb-3" {
                    h2 class="text-sm font-medium text-gray-300" { "Recently indexed" }
                    select id="docs-source-filter" class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-1 text-xs text-gray-300" onchange="loadDocs()" {
                        option value="" { "all sources" }
                    }
                }
                div id="docs-list" class="space-y-2" {}
            }
        }
    }
}

const EXTRA_STYLE: &str = r#"
.hit-snippet { font-size: 0.875rem; line-height: 1.4; color: #d1d5db; }
.hit-snippet mark { background: rgba(14,165,233,0.3); color: #e0f2fe; padding: 0 2px; border-radius: 2px; }
"#;

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
