//! /settings/agents — manage user-owned agents.
//!
//! Lets the user create additional main-thread agents (Peter/Kyron-tier
//! privileges: cross-module reads + handoff targets) and import existing
//! agents from .md / .txt / .json files (Claude Project, ChatGPT custom
//! GPT, generic). Parsers live in `crate::agents::import`.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Agents",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
    };
    let body = html! {
        (top_bar_standard("Agents"))
        (page_body())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        div class="max-w-4xl mx-auto px-4 py-6 space-y-6" {
            div {
                h1 class="text-2xl font-bold" { "Agents" }
                p class="text-gray-400 mt-1 text-sm" {
                    "Create and manage the agents in your Syntaur. "
                    "Main-thread agents get cross-module reads and can be "
                    "picked as the dashboard's primary persona. Module "
                    "agents stay in their lane."
                }
            }
            (inline_body())
        }
    }
}

/// Public helper — the three agent-management cards (list / create / import)
/// without the page heading or outer padding. Used by pages::settings to
/// embed the manager inline under Agents → All agents.
pub fn inline_body() -> Markup {
    html! {
        div class="space-y-6" {
            // ── Existing agents ────────────────────────────────────────
            div class="card p-5" {
                div class="flex items-center justify-between mb-3" {
                    h2 class="text-sm font-medium text-gray-300" { "Your agents" }
                    button id="refresh-btn" onclick="agentsLoad()" class="text-xs text-gray-500 hover:text-gray-300" { "Refresh" }
                }
                div id="agents-list" class="space-y-2" {
                    p class="text-xs text-gray-500" { "Loading…" }
                }
            }

            // ── Create new ─────────────────────────────────────────────
            div class="card p-5" id="create-card" {
                h2 class="text-sm font-medium text-gray-300 mb-3" { "Create new agent" }
                div class="grid grid-cols-1 md:grid-cols-2 gap-3" {
                    div {
                        label class="text-xs text-gray-400 block mb-1" { "Display name" }
                        input id="new-name" class="input" placeholder="e.g. Archer" {}
                    }
                    div {
                        label class="text-xs text-gray-400 block mb-1" { "Short description (optional)" }
                        input id="new-desc" class="input" placeholder="Software architecture partner" {}
                    }
                }
                div class="mt-3" {
                    label class="text-xs text-gray-400 block mb-1" { "System prompt" }
                    textarea id="new-prompt" rows="6"
                        class="w-full bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-white placeholder-gray-500 outline-none focus:border-oc-500"
                        placeholder="You are ... — full persona prompt. This replaces the default template entirely." {}
                }
                div class="mt-3 flex items-center gap-4 flex-wrap" {
                    label class="flex items-center gap-2 text-sm text-gray-300 cursor-pointer" {
                        input type="checkbox" id="new-main" class="accent-oc-500" checked;
                        " Main-thread agent (cross-module reads + handoff)"
                    }
                    label class="text-xs text-gray-400 flex items-center gap-1" {
                        "Avatar color"
                        input type="color" id="new-color" value="#7aa2ff" class="h-7 w-9 rounded cursor-pointer bg-gray-900 border border-gray-700";
                    }
                    div class="flex-1" {}
                    button onclick="agentsCreate()" id="create-btn" class="btn-primary text-sm py-2 px-4" { "Create agent" }
                }
                p id="create-result" class="text-xs mt-2" {}
            }

            // ── Import from file ───────────────────────────────────────
            div class="card p-5" id="import-card" {
                h2 class="text-sm font-medium text-gray-300 mb-2" { "Import from another platform" }
                p class="text-xs text-gray-500 mb-3" {
                    "Drop a Markdown, plain text, or JSON file. Supported shapes: "
                    "plain system-prompt .md/.txt, Markdown with YAML frontmatter, "
                    "ChatGPT custom-GPT JSON export, Claude Project export, "
                    "generic {name, description, system_prompt} JSON."
                }
                div id="drop-zone"
                    class="relative border-2 border-dashed border-gray-600 rounded-xl p-6 text-center cursor-pointer hover:border-oc-500 transition-colors"
                    onclick="document.getElementById('import-file').click()" {
                    svg class="mx-auto mb-2" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" {
                        path d="M12 3v12m0 0l-4-4m4 4l4-4M5 21h14";
                    }
                    p class="text-sm text-gray-300" { "Click, or drop a file here" }
                    p class="text-xs text-gray-500 mt-1" { ".md  ·  .txt  ·  .json" }
                    input type="file" id="import-file" class="hidden" accept=".md,.markdown,.txt,.json,application/json,text/markdown,text/plain" onchange="agentsImport(this.files[0])";
                }
                div class="mt-3 flex items-center gap-4 flex-wrap" {
                    label class="flex items-center gap-2 text-sm text-gray-300 cursor-pointer" {
                        input type="checkbox" id="import-main" class="accent-oc-500" checked;
                        " Treat as main-thread agent"
                    }
                    label class="text-xs text-gray-400 flex items-center gap-1" {
                        "Avatar color"
                        input type="color" id="import-color" value="#f0b470" class="h-7 w-9 rounded cursor-pointer bg-gray-900 border border-gray-700";
                    }
                }
                p id="import-result" class="text-xs mt-2" {}
            }
        }
    }
}

pub const AGENT_CSS: &str = EXTRA_STYLE;
pub const AGENT_JS:  &str = PAGE_JS;

const EXTRA_STYLE: &str = r#"
.agent-row {
    display: grid;
    grid-template-columns: 40px 1fr auto;
    gap: 12px;
    align-items: center;
    padding: 10px 12px;
    background: #111827;
    border: 1px solid #1f2937;
    border-radius: 10px;
    transition: border-color 0.15s;
}
.agent-row:hover { border-color: #374151; }
.agent-avatar {
    width: 36px; height: 36px;
    border-radius: 9px;
    background: #3b4252;
    display: grid; place-items: center;
    color: #fff; font-weight: 600; font-size: 14px;
}
.agent-name { color: #e5e7eb; font-size: 14px; font-weight: 500; }
.agent-meta { color: #6b7280; font-size: 12px; margin-top: 2px; }
.agent-chip {
    display: inline-block;
    padding: 1px 8px;
    font-size: 10px;
    font-weight: 500;
    border-radius: 999px;
    background: rgba(122,162,255,0.15);
    color: #7aa2ff;
    margin-right: 6px;
}
.agent-chip.module { background: rgba(156,163,175,0.15); color: #9ca3af; }
.agent-chip.imported { background: rgba(240,180,112,0.15); color: #f0b470; }
.agent-actions button {
    background: none; border: none; color: #6b7280;
    cursor: pointer; padding: 4px 8px;
    font-size: 11.5px; border-radius: 5px;
    transition: all 0.12s;
}
.agent-actions button:hover { color: #d1d5db; background: #1f2937; }
.agent-actions button.del:hover { color: #f87171; background: rgba(220,38,38,0.1); }
"#;

const PAGE_JS: &str = r#"
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }

const esc = (s) => String(s || '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

async function apiGet(path) {
  const r = await fetch(path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token));
  if (r.status === 401) { sessionStorage.removeItem('syntaur_token'); window.location.href = '/'; return null; }
  return r.json();
}
async function apiPost(path, body) {
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token, ...body }),
  });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || r.statusText);
  return data;
}
async function apiDelete(path) {
  const r = await fetch(path + '?token=' + encodeURIComponent(token), { method: 'DELETE' });
  if (!r.ok) throw new Error('delete failed');
  return r.json();
}

async function agentsLoad() {
  const box = document.getElementById('agents-list');
  const data = await apiGet('/api/agents/list');
  if (!data) return;
  const rows = data.agents || [];
  if (!rows.length) {
    box.innerHTML = '<p class="text-xs text-gray-500">No agents yet — create one below, or import from a file.</p>';
    return;
  }
  box.innerHTML = rows.map(a => {
    const initial = (a.display_name || '?').charAt(0).toUpperCase();
    const color = a.avatar_color || '#3b4252';
    const chips = [];
    if (a.is_main_thread) chips.push('<span class="agent-chip">main thread</span>');
    else chips.push('<span class="agent-chip module">module</span>');
    if (a.imported_from) chips.push(`<span class="agent-chip imported">from ${esc(a.imported_from)}</span>`);
    const desc = a.description ? esc(a.description) : '';
    return `<div class="agent-row">
      <div class="agent-avatar" style="background:${esc(color)};color:#0a0d12">${esc(initial)}</div>
      <div>
        <div class="agent-name">${esc(a.display_name)}</div>
        <div class="agent-meta">${chips.join('')}${desc ? ' ' + desc : ''}</div>
      </div>
      <div class="agent-actions">
        <button class="del" onclick="agentsDelete('${esc(a.agent_id)}','${esc(a.display_name)}')" title="Archive">Remove</button>
      </div>
    </div>`;
  }).join('');
}

async function agentsCreate() {
  const name = document.getElementById('new-name').value.trim();
  const desc = document.getElementById('new-desc').value.trim();
  const prompt = document.getElementById('new-prompt').value.trim();
  const isMain = document.getElementById('new-main').checked;
  const color = document.getElementById('new-color').value;
  const out = document.getElementById('create-result');
  const btn = document.getElementById('create-btn');
  if (!name) { out.textContent = 'Display name required.'; out.className = 'text-xs mt-2 text-red-400'; return; }
  if (!prompt) { out.textContent = 'System prompt required.'; out.className = 'text-xs mt-2 text-red-400'; return; }
  btn.disabled = true; btn.textContent = 'Creating…';
  try {
    const r = await apiPost('/api/agents/create', {
      display_name: name, description: desc || null, system_prompt: prompt,
      is_main_thread: isMain, avatar_color: color,
    });
    out.textContent = '✓ Created ' + (r.agent?.display_name || name);
    out.className = 'text-xs mt-2 text-green-400';
    document.getElementById('new-name').value = '';
    document.getElementById('new-desc').value = '';
    document.getElementById('new-prompt').value = '';
    agentsLoad();
  } catch(e) {
    out.textContent = 'Error: ' + e.message;
    out.className = 'text-xs mt-2 text-red-400';
  } finally {
    btn.disabled = false; btn.textContent = 'Create agent';
  }
}

async function agentsImport(file) {
  if (!file) return;
  const out = document.getElementById('import-result');
  const isMain = document.getElementById('import-main').checked;
  const color = document.getElementById('import-color').value;
  out.textContent = 'Parsing…'; out.className = 'text-xs mt-2 text-gray-400';
  try {
    const fd = new FormData();
    fd.append('token', token);
    fd.append('file', file);
    fd.append('is_main_thread', isMain ? '1' : '0');
    fd.append('avatar_color', color);
    const r = await fetch('/api/agents/import', { method: 'POST', body: fd });
    const data = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(data.error || 'upload failed');
    out.textContent = '✓ Imported ' + (data.agent?.display_name || file.name) + ' (' + (data.source_format || 'unknown') + ')';
    out.className = 'text-xs mt-2 text-green-400';
    agentsLoad();
  } catch(e) {
    out.textContent = 'Error: ' + e.message;
    out.className = 'text-xs mt-2 text-red-400';
  }
  document.getElementById('import-file').value = '';
}

async function agentsDelete(id, name) {
  if (!confirm('Remove agent "' + name + '"? This archives the row — conversation history is preserved.')) return;
  try {
    await apiDelete('/api/agents/' + encodeURIComponent(id));
    agentsLoad();
  } catch(e) { alert('Error: ' + e.message); }
}

// Drag-and-drop for the import zone.
(function() {
  const dz = document.getElementById('drop-zone');
  if (!dz) return;
  ['dragenter','dragover'].forEach(ev => dz.addEventListener(ev, e => {
    e.preventDefault(); dz.classList.add('border-oc-500');
  }));
  ['dragleave','drop'].forEach(ev => dz.addEventListener(ev, e => {
    e.preventDefault(); dz.classList.remove('border-oc-500');
  }));
  dz.addEventListener('drop', e => {
    const f = e.dataTransfer?.files?.[0];
    if (f) agentsImport(f);
  });
})();

agentsLoad();
"#;
