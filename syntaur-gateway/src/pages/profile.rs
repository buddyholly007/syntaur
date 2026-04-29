//! /profile — user profile page: agents, personality, password, data location.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Profile",
        authed: true,
        extra_style: None,
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        div class="max-w-3xl mx-auto px-4 py-6 space-y-6" {
            div id="profile-header" class="flex items-center gap-4" {
                div class="w-12 h-12 rounded-full bg-oc-600 flex items-center justify-center text-xl font-bold" id="profile-avatar" { "?" }
                div {
                    h1 class="text-xl font-bold" id="profile-name" { "Loading..." }
                    p class="text-sm text-gray-400" id="profile-role" {}
                }
            }

            // ── My Agents ────────────────────────────────────────────
            div class="card p-5" {
                div class="flex items-center justify-between mb-3" {
                    h2 class="text-sm font-medium text-gray-300" { "My Agents" }
                    button onclick="showCreateAgent()"
                        class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded-lg" { "New Agent" }
                }
                div id="my-agents-list" class="space-y-2" {
                    p class="text-sm text-gray-500" { "Loading..." }
                }
            }

            // ── Personality ──────────────────────────────────────────
            div class="card p-5" {
                h2 class="text-sm font-medium text-gray-300 mb-1" { "AI Personality" }
                p class="text-xs text-gray-500 mb-3" {
                    "Documents that shape how your AI agent communicates. Max 4,000 characters combined."
                }
                div class="mb-3" {
                    label class="text-xs text-gray-500" { "Agent:" }
                    select id="personality-agent" class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-1 text-xs text-gray-300 ml-1"
                        onchange="loadPersonality()" {}
                }
                div id="personality-list" class="space-y-2 mb-3" {}
                div class="space-y-2 bg-gray-900 rounded-lg p-3" {
                    select id="new-doc-type" class="bg-gray-800 border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-300" {
                        option value="bio" { "Bio — about you" }
                        option value="preferences" { "Preferences — communication style" }
                        option value="writing_style" { "Writing Style — samples of your writing" }
                        option value="prompt" { "Custom Prompt — direct instructions" }
                    }
                    input type="text" id="new-doc-title" placeholder="Title"
                        class="w-full bg-gray-800 border border-gray-700 rounded px-3 py-1.5 text-xs text-white placeholder-gray-500 outline-none";
                    textarea id="new-doc-content" rows="3" placeholder="Content..."
                        class="w-full bg-gray-800 border border-gray-700 rounded px-3 py-1.5 text-xs text-white placeholder-gray-500 outline-none resize-none" {}
                    div class="flex gap-2" {
                        button onclick="addPersonalityDoc()"
                            class="text-xs bg-gray-700 hover:bg-gray-600 text-white px-3 py-1.5 rounded" { "Add" }
                        label class="text-xs bg-gray-700 hover:bg-gray-600 text-white px-3 py-1.5 rounded cursor-pointer" {
                            "Upload File"
                            input type="file" class="hidden" accept=".txt,.md,.pdf" onchange="uploadPersonalityFile(this.files[0])";
                        }
                    }
                }
            }

            // ── Password ─────────────────────────────────────────────
            div class="card p-5" {
                h2 class="text-sm font-medium text-gray-300 mb-3" { "Password" }
                div class="space-y-3 max-w-sm" {
                    input type="password" id="pw-current" placeholder="Current password"
                        class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 outline-none";
                    input type="password" id="pw-new" placeholder="New password"
                        class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 outline-none";
                    button onclick="changePassword()"
                        class="text-sm bg-gray-700 hover:bg-gray-600 text-white px-4 py-1.5 rounded-lg" id="pw-btn" { "Update Password" }
                    p id="pw-status" class="text-xs hidden" {}
                }
            }

            // ── Data Location ────────────────────────────────────────
            div class="card p-5" {
                h2 class="text-sm font-medium text-gray-300 mb-1" { "Data Location" }
                p class="text-xs text-gray-500 mb-3" {
                    "Where your documents, uploads, and agent workspaces are stored."
                }
                div class="flex gap-2 items-end max-w-lg" {
                    div class="flex-1" {
                        label class="text-xs text-gray-500" { "Current path" }
                        input type="text" id="data-location" readonly
                            class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-gray-300";
                    }
                    button onclick="changeDataLocation()"
                        class="text-sm bg-gray-700 hover:bg-gray-600 text-white px-4 py-2 rounded-lg flex-shrink-0" { "Change" }
                }
                p id="data-location-status" class="text-xs mt-2 hidden" {}
            }
        }

        // ── Create Agent dialog ──────────────────────────────────────
        div id="create-agent-dialog" class="fixed inset-0 z-50 bg-black/60 flex items-center justify-center hidden" {
            div class="bg-gray-800 rounded-xl border border-gray-700 p-6 w-full max-w-sm mx-4" {
                h3 class="font-medium mb-4" { "Create Agent" }
                div class="space-y-3" {
                    input type="text" id="ca-name" placeholder="Agent name"
                        class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 outline-none";
                    select id="ca-base" class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-gray-300" {}
                    textarea id="ca-prompt" rows="3" placeholder="Personality / system prompt (optional)"
                        class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 outline-none resize-none" {}
                    div class="flex gap-2" {
                        button onclick="doCreateAgent()"
                            class="flex-1 bg-oc-600 hover:bg-oc-700 text-white text-sm py-2 rounded-lg" id="ca-btn" { "Create" }
                        button onclick="document.getElementById('create-agent-dialog').classList.add('hidden')"
                            class="flex-1 bg-gray-700 hover:bg-gray-600 text-white text-sm py-2 rounded-lg" { "Cancel" }
                    }
                    p id="ca-error" class="text-sm text-red-400 hidden" {}
                }
            }
        }

        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

const PAGE_JS: &str = r##"
const token = sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '';
// Client-side token-gate removed 2026-04-24 (module-reset bug fix).
const esc = (s) => String(s||'').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function authFetch(url, opts = {}) {
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;
  return fetch(url, opts);
}

let myAgents = [];
let systemAgents = [];

async function loadProfile() {
  let me = null;
  try {
    me = await authFetch('/api/me').then(r => r.json());
  } catch(e) {
    me = null;
  }
  if (me && me.user) {
    document.getElementById('profile-name').textContent = me.user.name;
    document.getElementById('profile-role').textContent = me.user.role;
    document.getElementById('profile-avatar').textContent = (me.user.name || '?')[0].toUpperCase();
  } else {
    document.getElementById('profile-name').textContent = 'Guest';
    document.getElementById('profile-role').textContent = '';
    document.getElementById('profile-avatar').textContent = '?';
  }
  myAgents = (me && me.agents) || [];
  renderAgents();

  // Load system agents for base_agent dropdown
  try {
    const h = await fetch('/health').then(r => r.json());
    systemAgents = (h.agents || []).map(a => typeof a === 'object' ? a.id : a);
    const baseSelect = document.getElementById('ca-base');
    baseSelect.innerHTML = systemAgents.map(a => `<option value="${esc(a)}">${esc(a)}</option>`).join('');
    // Populate personality agent dropdown
    const paSel = document.getElementById('personality-agent');
    const allAgents = [...systemAgents, ...myAgents.map(a => a.agent_id)];
    paSel.innerHTML = allAgents.map(a => `<option value="${esc(a)}">${esc(a)}</option>`).join('');
  } catch {}

  // Data location
  document.getElementById('data-location').value = me.data_dir || '~/.syntaur';

  loadPersonality();
}

function renderAgents() {
  const list = document.getElementById('my-agents-list');
  if (myAgents.length === 0) {
    list.innerHTML = '<p class="text-sm text-gray-500">No custom agents yet. Create one to personalize your AI.</p>';
    return;
  }
  list.innerHTML = myAgents.map(a => `
    <div class="flex items-center justify-between bg-gray-900 rounded-lg p-3">
      <div>
        <span class="font-medium text-sm">${esc(a.display_name)}</span>
        <span class="text-xs text-gray-500 ml-2">base: ${esc(a.base_agent)}</span>
        ${!a.enabled ? '<span class="text-xs text-red-400 ml-1">(disabled)</span>' : ''}
      </div>
      <button onclick="deleteAgent('${esc(a.agent_id)}')" class="text-xs text-red-400 hover:text-red-300 px-2 py-1">Delete</button>
    </div>
  `).join('');
}

function showCreateAgent() {
  document.getElementById('ca-error').classList.add('hidden');
  document.getElementById('create-agent-dialog').classList.remove('hidden');
}

async function doCreateAgent() {
  const name = document.getElementById('ca-name').value.trim();
  if (!name) { document.getElementById('ca-error').textContent = 'Name required'; document.getElementById('ca-error').classList.remove('hidden'); return; }
  const agentId = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 30) || 'my-agent';
  document.getElementById('ca-btn').textContent = 'Creating...';
  const data = await authFetch('/api/me/agents', {
    method: 'POST', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ token, agent_id: agentId, display_name: name,
      base_agent: document.getElementById('ca-base').value,
      system_prompt: document.getElementById('ca-prompt').value.trim() || null })
  }).then(r => r.json());
  document.getElementById('ca-btn').textContent = 'Create';
  if (data.ok) {
    document.getElementById('create-agent-dialog').classList.add('hidden');
    loadProfile();
  } else {
    document.getElementById('ca-error').textContent = data.error || 'Failed';
    document.getElementById('ca-error').classList.remove('hidden');
  }
}

async function deleteAgent(id) {
  if (!confirm('Delete agent "' + id + '"?')) return;
  await authFetch('/api/me/agents/' + id, { method: 'DELETE' });
  loadProfile();
}

// ── Personality ──────────────────────────────────────────────────────────
async function loadPersonality() {
  const aid = document.getElementById('personality-agent').value || 'main';
  const data = await authFetch('/api/me/personality?agent_id=' + aid).then(r => r.json());
  const list = document.getElementById('personality-list');
  const docs = data.docs || [];
  if (docs.length === 0) {
    list.innerHTML = '<p class="text-xs text-gray-500">No personality docs yet.</p>';
    return;
  }
  const totalChars = docs.reduce((s, d) => s + d.content.length, 0);
  list.innerHTML = `<p class="text-xs text-gray-500 mb-1">${totalChars.toLocaleString()} / 4,000 characters used</p>` +
    docs.map(d => `
    <div class="bg-gray-900 rounded-lg p-3">
      <div class="flex items-center justify-between mb-1">
        <div><span class="text-xs px-1.5 py-0.5 rounded bg-gray-700 text-gray-400">${esc(d.doc_type)}</span> <span class="text-sm font-medium ml-1">${esc(d.title)}</span></div>
        <button onclick="deletePersonalityDoc(${d.id})" class="text-xs text-red-400 hover:text-red-300">Delete</button>
      </div>
      <p class="text-xs text-gray-400 mt-1 line-clamp-2">${esc(d.content.slice(0,200))}</p>
    </div>
  `).join('');
}

async function addPersonalityDoc() {
  const aid = document.getElementById('personality-agent').value || 'main';
  const title = document.getElementById('new-doc-title').value.trim();
  const content = document.getElementById('new-doc-content').value.trim();
  if (!title || !content) return;
  await authFetch('/api/me/personality', {
    method: 'POST', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ token, agent_id: aid, doc_type: document.getElementById('new-doc-type').value, title, content })
  });
  document.getElementById('new-doc-title').value = '';
  document.getElementById('new-doc-content').value = '';
  loadPersonality();
}

async function uploadPersonalityFile(file) {
  if (!file) return;
  const text = await file.text();
  const aid = document.getElementById('personality-agent').value || 'main';
  await authFetch('/api/me/personality', {
    method: 'POST', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ token, agent_id: aid, doc_type: 'writing_style', title: file.name, content: text.slice(0, 4000) })
  });
  loadPersonality();
}

async function deletePersonalityDoc(id) {
  await authFetch('/api/me/personality/' + id, { method: 'DELETE' });
  loadPersonality();
}

// ── Password ─────────────────────────────────────────────────────────────
async function changePassword() {
  const btn = document.getElementById('pw-btn');
  const status = document.getElementById('pw-status');
  btn.textContent = 'Updating...'; status.classList.add('hidden');
  const data = await authFetch('/api/me/password', {
    method: 'PUT', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ token, current_password: document.getElementById('pw-current').value || null,
      new_password: document.getElementById('pw-new').value })
  }).then(r => r.json());
  status.textContent = data.ok ? 'Password updated' : (data.error || 'Failed');
  status.className = 'text-xs ' + (data.ok ? 'text-green-400' : 'text-red-400');
  status.classList.remove('hidden');
  document.getElementById('pw-current').value = '';
  document.getElementById('pw-new').value = '';
  btn.textContent = 'Update Password';
}

// ── Data Location ────────────────────────────────────────────────────────
async function changeDataLocation() {
  const newPath = prompt('Enter new data directory path:', document.getElementById('data-location').value);
  if (!newPath || newPath === document.getElementById('data-location').value) return;
  const status = document.getElementById('data-location-status');
  status.textContent = 'Migrating data... this may take a moment';
  status.className = 'text-xs text-gray-400 mt-2';
  status.classList.remove('hidden');
  const data = await authFetch('/api/me/data-location', {
    method: 'PUT', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ token, path: newPath })
  }).then(r => r.json());
  if (data.ok) {
    document.getElementById('data-location').value = newPath;
    status.textContent = 'Data location updated. ' + (data.migrated ? 'Files migrated.' : '');
    status.className = 'text-xs text-green-400 mt-2';
  } else {
    status.textContent = data.error || 'Failed';
    status.className = 'text-xs text-red-400 mt-2';
  }
}

loadProfile();
"##;
