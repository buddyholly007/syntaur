//! /history — conversation history list + modal viewer.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "History",
        authed: true,
        extra_style: None,
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        (page_body())
        (modal())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        div class="max-w-4xl mx-auto px-4 py-6" {
            div class="flex items-center justify-between mb-6" {
                h1 class="text-2xl font-bold" { "Conversation History" }
                div class="flex items-center gap-2" {
                    input type="text" id="search"
                        class="bg-gray-800 border border-gray-700 rounded-lg px-3 py-1.5 text-sm text-white placeholder-gray-500 outline-none focus:border-oc-500 w-48"
                        placeholder="Search conversations..."
                        oninput="filterConversations()";
                }
            }

            div id="conv-list" class="space-y-2" {
                p class="text-gray-500 text-sm py-8 text-center" { "Loading conversations..." }
            }

            div id="conv-empty" class="hidden text-center py-12" {
                p class="text-gray-500 text-lg mb-2" { "No conversations yet" }
                p class="text-gray-600 text-sm mb-4" {
                    "Start chatting and your conversations will appear here."
                }
                a href="/chat" class="inline-block bg-oc-600 hover:bg-oc-700 text-white font-medium py-2 px-5 rounded-lg transition-colors text-sm" {
                    "Start a Conversation"
                }
            }
        }
    }
}

fn modal() -> Markup {
    html! {
        div id="conv-modal" class="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm hidden" {
            div class="h-full flex flex-col max-w-3xl mx-auto bg-gray-900 border-x border-gray-700" {
                div class="flex items-center justify-between p-4 border-b border-gray-700 flex-shrink-0" {
                    div {
                        h3 class="font-semibold" id="modal-title" { "Conversation" }
                        p class="text-xs text-gray-500" id="modal-meta" {}
                    }
                    div class="flex items-center gap-2" {
                        button onclick="resumeConversation()"
                               class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1 rounded-lg" {
                            "Continue in Chat"
                        }
                        button onclick="closeModal()" class="text-gray-400 hover:text-white text-xl" {
                            "×"
                        }
                    }
                }
                div class="flex-1 overflow-y-auto p-4 space-y-4" id="modal-messages" {}
            }
        }
    }
}

const PAGE_JS: &str = r#"
const token = sessionStorage.getItem('syntaur_token') || '';
// Client-side token-gate removed 2026-04-24 (module-reset bug fix).

// Phase 1.1: session token sent via Authorization: Bearer only. Server
// middleware (security::lift_bearer_to_body_and_query) copies it back into
// query + body for handlers that still read those positions.
async function api(path, opts) {
  opts = opts || {};
  opts.headers = Object.assign({ 'Authorization': 'Bearer ' + token }, opts.headers || {});
  const r = await fetch(path, opts);
  // 2026-04-25: stop bouncing on widget 401 (module-reset bug fix).
  if (r.status === 401) { return null; }
  return r.json();
}

let allConversations = [];
let currentConvId = null;

async function loadConversations() {
  try {
    const data = await api('/api/conversations?limit=50');
    if (!data) return;
    allConversations = data.conversations || [];
    if (allConversations.length === 0) {
      document.getElementById('conv-list').classList.add('hidden');
      document.getElementById('conv-empty').classList.remove('hidden');
      return;
    }
    document.getElementById('conv-empty').classList.add('hidden');
    renderConversations(allConversations);
  } catch(e) {
    document.getElementById('conv-list').innerHTML = '<p class="text-red-400 text-sm">Failed to load conversations</p>';
  }
}

function renderConversations(convs) {
  const list = document.getElementById('conv-list');
  list.innerHTML = convs.map(c => {
    const date = new Date(c.created_at * 1000);
    const timeStr = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }) + ' ' +
                    date.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
    const msgs = c.message_count || 0;
    const title = c.title || 'Untitled conversation';
    return `
      <button onclick="openConversation('${c.id}')" class="w-full text-left p-4 rounded-xl bg-gray-800 border border-gray-700 hover:border-gray-600 transition-colors">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <p class="font-medium text-sm text-gray-200 truncate">${escapeHtml(title)}</p>
            <p class="text-xs text-gray-500 mt-1">${msgs} messages &middot; ${timeStr}</p>
          </div>
          <span class="text-xs text-gray-600 flex-shrink-0">${c.agent || 'main'}</span>
        </div>
      </button>`;
  }).join('');
}

function filterConversations() {
  const q = document.getElementById('search').value.toLowerCase();
  if (!q) { renderConversations(allConversations); return; }
  const filtered = allConversations.filter(c => (c.title || '').toLowerCase().includes(q));
  renderConversations(filtered);
}

async function openConversation(id) {
  currentConvId = id;
  const modal = document.getElementById('conv-modal');
  modal.classList.remove('hidden');
  const messagesEl = document.getElementById('modal-messages');
  messagesEl.innerHTML = '<p class="text-gray-500 text-sm text-center py-4">Loading...</p>';
  try {
    const data = await api(`/api/conversations/${id}`);
    if (!data) return;
    const conv = data.conversation || {};
    const msgs = data.messages || [];
    document.getElementById('modal-title').textContent = conv.title || 'Conversation';
    const date = new Date((conv.created_at || 0) * 1000);
    document.getElementById('modal-meta').textContent =
      `${msgs.length} messages · ${date.toLocaleDateString()} · ${conv.agent || 'main'}`;
    messagesEl.innerHTML = msgs.map(m => {
      const isUser = m.role === 'user';
      if (isUser) {
        return `<div class="flex justify-end"><div class="max-w-[75%] bg-oc-900/40 border border-oc-800/40 rounded-xl px-4 py-2.5 text-sm text-gray-200">${escapeHtml(m.content)}</div></div>`;
      } else {
        return `<div class="flex gap-3"><div class="w-7 h-7 rounded-lg bg-oc-600 flex items-center justify-center text-xs font-bold flex-shrink-0 mt-0.5"><img src="/icon.svg" class="w-4 h-4"></div><div class="text-sm text-gray-300 leading-relaxed">${escapeHtml(m.content)}</div></div>`;
      }
    }).join('');
  } catch(e) {
    messagesEl.innerHTML = '<p class="text-red-400 text-sm">Failed to load messages</p>';
  }
}

function closeModal() {
  document.getElementById('conv-modal').classList.add('hidden');
  currentConvId = null;
}

function resumeConversation() {
  if (currentConvId) {
    sessionStorage.setItem('resume_conv_id', currentConvId);
    location.href = '/chat';
  }
}

function escapeHtml(s) {
  return (s || '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\n/g,'<br>');
}

document.addEventListener('keydown', e => { if (e.key === 'Escape') closeModal(); });
loadConversations();
"#;
