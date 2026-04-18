//! /chat — migrated from static/chat.html. Structural markup and
//! embedded scripts live as raw-string consts below so their bytes
//! count as Rust and the file compiles type-checked through maud.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Chat",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! { (PreEscaped(BODY_HTML)) };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
  pre, code { font-family: 'JetBrains Mono', monospace; }
  .chat-md pre { white-space: pre-wrap; word-break: break-word; }
  @keyframes bounce { 0%,100%{transform:translateY(0)} 50%{transform:translateY(-4px)} }
  textarea { scrollbar-width: thin; }

  @keyframes vm-pulse { 0%,100%{box-shadow:0 0 0 0 rgba(14,165,233,0.3)} 50%{box-shadow:0 0 0 20px rgba(14,165,233,0)} }
  .vm-listening #vm-ring { border-color: #0ea5e9; animation: vm-pulse 2s infinite; }
  .vm-listening #vm-mic-icon { color: #0ea5e9; }
  .vm-speaking #vm-ring { border-color: #f59e0b; }
  .vm-speaking #vm-mic-icon { color: #f59e0b; }
  .vm-thinking #vm-ring { border-color: #8b5cf6; }
  .vm-thinking #vm-mic-icon { color: #8b5cf6; }
  .vm-playing #vm-ring { border-color: #10b981; }
  .vm-playing #vm-mic-icon { color: #10b981; }"##;

const BODY_HTML: &str = r##"<!-- Top bar -->
<div class="border-b border-gray-800 bg-gray-900/80 backdrop-blur flex-shrink-0">
  <div class="max-w-4xl mx-auto px-3 sm:px-4 py-2 sm:py-2.5 flex items-center justify-between">
    <div class="flex items-center gap-2 sm:gap-3 min-w-0">
      <a href="/" class="flex items-center gap-2 hover:opacity-80 flex-shrink-0">
        <img src="/app-icon.jpg" class="h-8 w-8 rounded-lg" alt="">
        <span class="font-semibold hidden sm:inline">Syntaur</span>
      </a>
      <div class="relative" id="agent-switcher">
        <button onclick="toggleAgentMenu()" class="flex items-center gap-1 text-gray-300 text-sm font-medium hover:text-white transition-colors">
          <span id="agent-name">Chat</span>
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="text-gray-500"><path d="M6 9l6 6 6-6"/></svg>
        </button>
        <div id="agent-menu" class="hidden absolute left-0 top-full mt-1 bg-gray-800 border border-gray-700 rounded-lg shadow-xl py-1 min-w-[160px] z-50">
          <!-- Populated by JS -->
        </div>
      </div>
    </div>
    <div class="flex items-center gap-2 sm:gap-3 text-sm flex-shrink-0">
      <button onclick="newConversation()" class="text-gray-500 hover:text-gray-300 text-xs sm:text-sm">New Chat</button>
      <a href="/" class="text-gray-500 hover:text-gray-300 text-xs sm:text-sm">Home</a>
    </div>
  </div>
</div>

<!-- Messages area -->
<div class="flex-1 overflow-y-auto" id="messages-scroll">
  <div class="max-w-3xl mx-auto px-4 py-6 space-y-6" id="messages">

    <!-- Welcome message -->
    <div class="flex gap-4" id="welcome-msg">
      <img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
      <div class="flex-1">
        <p class="text-gray-300 leading-relaxed" id="greeting">Hey! I'm your AI assistant. How can I help you today?</p>
        <div class="flex flex-wrap gap-2 mt-4">
          <button onclick="send('What can you do?')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">What can you do?</button>
          <button onclick="send('Search the web for today\\'s top tech news')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">Today's news</button>
          <button onclick="send('Help me draft a professional email')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">Draft an email</button>
          <button onclick="send('Create an Excel budget spreadsheet')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">Spreadsheet</button>
        </div>
      </div>
    </div>

  </div>
</div>

<!-- Voice mode overlay -->
<div id="voice-mode-overlay" class="fixed inset-0 z-40 bg-gray-950/95 hidden flex flex-col items-center justify-center">
  <div class="text-center max-w-sm px-6">
    <div id="vm-ring" class="w-32 h-32 mx-auto mb-6 rounded-full border-4 border-gray-700 flex items-center justify-center transition-all duration-300">
      <svg id="vm-mic-icon" width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" class="text-gray-500 transition-colors">
        <path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/>
      </svg>
    </div>
    <div id="vm-status" class="text-lg text-gray-400 mb-2">Listening...</div>
    <div id="vm-transcript" class="text-sm text-gray-500 min-h-[40px] mb-6"></div>
    <button onclick="toggleVoiceMode()" class="text-sm text-gray-600 hover:text-gray-400 border border-gray-700 rounded-xl px-6 py-2">End voice mode</button>
  </div>
</div>

<!-- Drop overlay -->
<div id="drop-overlay" class="fixed inset-0 z-50 bg-oc-600/10 backdrop-blur-sm hidden flex items-center justify-center pointer-events-none">
  <div class="bg-gray-800 border-2 border-dashed border-oc-500 rounded-2xl p-12 text-center">
    <p class="text-xl font-semibold text-oc-500">Drop file here</p>
    <p class="text-sm text-gray-400 mt-2">The AI will read and analyze your file</p>
  </div>
</div>

<!-- Input area -->
<div class="border-t border-gray-800 bg-gray-900/80 backdrop-blur flex-shrink-0">
  <div class="max-w-3xl mx-auto px-3 sm:px-4 py-3 sm:py-4">
    <!-- Pending files -->
    <div id="pending-files" class="hidden mb-2 flex flex-wrap gap-2"></div>
    <div class="flex gap-2 sm:gap-3 items-end">
      <button onclick="toggleVoiceMode()" class="flex-shrink-0 text-gray-500 hover:text-oc-500 p-2 rounded-lg hover:bg-gray-800 transition-colors" id="voice-mode-btn" title="Voice mode">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/><circle cx="12" cy="12" r="10" stroke-dasharray="4 4" opacity="0.3"/></svg>
      </button>
      <label class="flex-shrink-0 cursor-pointer text-gray-500 hover:text-gray-300 p-2 rounded-lg hover:bg-gray-800 transition-colors" title="Attach file">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/></svg>
        <input type="file" class="hidden" id="file-input" onchange="handleFileSelect(event)" multiple>
      </label>

      <div class="flex-1 relative">
        <textarea id="input" rows="1" class="w-full bg-gray-800 border border-gray-700 hover:border-gray-600 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 rounded-2xl px-4 sm:px-5 py-2.5 sm:py-3 pr-20 text-white placeholder-gray-400 outline-none resize-none text-sm leading-relaxed" placeholder="Message Syntaur... (drop files here)" onkeydown="handleKey(event)" oninput="autoGrow(this)" style="max-height:200px"></textarea>
        <div class="absolute right-10 bottom-1 flex items-center">
          <div id="mic-menu" class="hidden absolute bottom-10 right-0 bg-gray-900 border border-gray-700 rounded-xl shadow-xl p-3 w-56 z-50">
            <div class="text-xs text-gray-500 font-medium mb-2">Audio source</div>
            <div id="mic-devices" class="space-y-1 mb-2"></div>
            <div class="border-t border-gray-800 pt-2 mt-1">
              <button onclick="showPhoneQr()" class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-xs text-gray-400 hover:bg-gray-800 hover:text-white transition-colors">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="5" y="2" width="14" height="20" rx="2"/><line x1="12" y1="18" x2="12" y2="18.01"/></svg>
                Use phone as mic
              </button>
            </div>
            <div id="phone-qr-area" class="hidden mt-3 text-center border-t border-gray-800 pt-3">
              <img src="" id="phone-qr-img" class="inline-block w-32 h-32 bg-white p-2 rounded-lg">
              <p class="text-xs text-gray-500 mt-2">Scan to open Syntaur Voice</p>
              <a href="" id="phone-qr-link" target="_blank" class="text-xs text-oc-500 hover:text-oc-400">or tap to open</a>
            </div>
          </div>
          <button onmousedown="startVoice(event)" onmouseup="stopVoice(event)" ontouchstart="startVoice(event)" ontouchend="stopVoice(event)" onclick="toggleMicMenu(event)" class="text-gray-500 hover:text-oc-500 rounded-lg p-1.5 transition-colors" id="mic-btn" title="Hold to speak, click to change mic">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
          </button>
        </div>
        <button onclick="sendFromInput()" class="absolute right-2 bottom-2 bg-oc-600 hover:bg-oc-700 disabled:opacity-40 text-white rounded-lg p-1.5 transition-colors" id="send-btn">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M5 12h14M12 5l7 7-7 7"/></svg>
        </button>
      </div>
    </div>
    <p class="text-xs text-gray-600 text-center mt-2 hidden sm:block">Drop files to attach &middot; Your data stays on your machine</p>
  </div>
</div>

<script>
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }
let sending = false;
let voiceInitiated = false;
let pendingFiles = [];
let currentAgent = 'main';
let agents = [];
let conversationId = null;

// Load agents and initialize
(async () => {
  try {
    const health = await (await fetch('/health', { headers: { 'Authorization': 'Bearer ' + token } })).json();
    const rawAgents = health.agents || [];

    // Health now returns {id, name} objects
    window._agentIdMap = {};
    agents = rawAgents.map(a => {
      if (typeof a === 'object') {
        window._agentIdMap[a.name] = a.id;
        return a.name;
      }
      return a; // fallback for old format
    });

    if (agents.length > 0) {
      currentAgent = agents[0];
      updateAgentDisplay();
      buildAgentMenu();
    }

    // Resume last conversation for this agent, or start fresh
    await loadLastConversation();

    // When page becomes visible again (user navigated back), reload messages
    document.addEventListener('visibilitychange', async () => {
      if (document.visibilityState === 'visible' && conversationId && !sending) {
        await reloadCurrentConversation();
      }
    });
  } catch(e) {
    console.error('init:', e);
  }
})();

function updateAgentDisplay() {
  document.getElementById('agent-name').textContent = currentAgent;
  document.getElementById('greeting').textContent = `Hey! I'm ${currentAgent}. How can I help you today?`;
  document.title = `${currentAgent} — Syntaur`;
}

function buildAgentMenu() {
  const menu = document.getElementById('agent-menu');
  menu.innerHTML = agents.map(a => `
    <button onclick="switchAgent('${a}')" class="w-full text-left px-4 py-2 text-sm hover:bg-gray-700 transition-colors ${a === currentAgent ? 'text-oc-500 font-medium' : 'text-gray-300'}">
      ${esc(a)}
    </button>
  `).join('');
}

function toggleAgentMenu() {
  const menu = document.getElementById('agent-menu');
  menu.classList.toggle('hidden');
  // Close on click outside
  if (!menu.classList.contains('hidden')) {
    setTimeout(() => {
      document.addEventListener('click', function close(e) {
        if (!menu.contains(e.target) && !e.target.closest('#agent-switcher')) {
          menu.classList.add('hidden');
        }
        document.removeEventListener('click', close);
      });
    }, 10);
  }
}

async function switchAgent(name) {
  document.getElementById('agent-menu').classList.add('hidden');
  if (name === currentAgent) return;
  currentAgent = name;
  updateAgentDisplay();
  buildAgentMenu();
  conversationId = null;
  clearMessagesUI();
  await loadLastConversation();
}

async function loadLastConversation() {
  try {
    const agentId = getAgentId(currentAgent);
    // Try agent ID first, then display name (for conversations created before ID fix)
    let convs = [];
    for (const name of [agentId, currentAgent]) {
      const resp = await fetch(`/api/conversations?token=${token}&agent=${encodeURIComponent(name)}&limit=1`);
      const data = await resp.json();
      convs = data.conversations || [];
      if (convs.length > 0) break;
    }
    if (convs.length > 0) {
      conversationId = convs[0].id;
      await loadConversationMessages(conversationId);
    }
  } catch(e) {
    console.log('No conversation history:', e.message);
  }
}

// Map display name back to agent ID for API calls
function getAgentId(displayName) {
  // The health endpoint returns display names, but the API needs IDs.
  // Common mapping: "Felix" -> "main", others are their own IDs.
  // We store this from the setup status.
  return window._agentIdMap?.[displayName] || displayName.toLowerCase().replace(/\s+/g, '-');
}

async function loadConversationMessages(convId) {
  try {
    const resp = await fetch(`/api/conversations/${convId}?token=${token}`);
    const data = await resp.json();
    const messages = data.messages || [];
    if (messages.length === 0) return;

    // Hide welcome suggestions
    const welcome = document.getElementById('welcome-msg');
    if (welcome) {
      const chips = welcome.querySelector('.flex.flex-wrap');
      if (chips) chips.remove();
    }

    const container = document.getElementById('messages');

    for (const msg of messages) {
      if (msg.role === 'user') {
        const el = document.createElement('div');
        el.className = 'flex gap-4 justify-end';
        el.innerHTML = `<div class="max-w-[75%]">
          <div class="bg-oc-900/50 border border-oc-800/40 rounded-2xl rounded-br-md px-5 py-3 text-sm text-gray-200 leading-relaxed">${esc(msg.content)}</div></div>`;
        container.appendChild(el);
      } else if (msg.role === 'assistant' && msg.content) {
        const el = document.createElement('div');
        el.className = 'flex gap-4';
        el.innerHTML = `<img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
          <div class="flex-1 min-w-0">
            <div class="text-gray-300 leading-relaxed text-sm chat-md">${md(msg.content)}</div>
          </div>`;
        container.appendChild(el);
      }
    }

    // If last message is from user, the AI is still working — show indicator and poll
    if (messages.length > 0 && messages[messages.length - 1].role === 'user') {
      const thinkEl = document.createElement('div');
      thinkEl.className = 'flex gap-4';
      thinkEl.id = 'pending-response';
      thinkEl.innerHTML = `<img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 text-gray-500 text-sm py-2">
            <span class="flex gap-1">
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:0ms"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:150ms"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:300ms"></span>
            </span>
            <span class="text-xs">Working on it...</span>
          </div>
        </div>`;
      container.appendChild(thinkEl);
      pollForResponse(convId, messages.length);
    }

    const scroll = document.getElementById('messages-scroll');
    scroll.scrollTop = scroll.scrollHeight;
  } catch(e) {
    console.log('Failed to load messages:', e.message);
  }
}

// Poll conversation for new messages (AI still responding)
async function pollForResponse(convId, lastCount) {
  const poll = async () => {
    try {
      const resp = await fetch(`/api/conversations/${convId}?token=${token}`);
      const data = await resp.json();
      const messages = data.messages || [];
      if (messages.length > lastCount) {
        // New message arrived — remove thinking indicator and show response
        const pending = document.getElementById('pending-response');
        if (pending) pending.remove();
        const container = document.getElementById('messages');
        for (let i = lastCount; i < messages.length; i++) {
          const msg = messages[i];
          if (msg.role === 'assistant' && msg.content) {
            const el = document.createElement('div');
            el.className = 'flex gap-4';
            el.innerHTML = `<img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
              <div class="flex-1 min-w-0">
                <div class="text-gray-300 leading-relaxed text-sm chat-md">${md(msg.content)}</div>
              </div>`;
            container.appendChild(el);
          }
        }
        const scroll = document.getElementById('messages-scroll');
        scroll.scrollTop = scroll.scrollHeight;
        return; // Stop polling
      }
    } catch(e) {}
    // Keep polling every 2 seconds
    setTimeout(poll, 2000);
  };
  setTimeout(poll, 2000);
}

function clearMessagesUI() {
  const container = document.getElementById('messages');
  container.innerHTML = `
    <div class="flex gap-4" id="welcome-msg">
      <img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
      <div class="flex-1">
        <p class="text-gray-300 leading-relaxed" id="greeting">Hey! I'm ${esc(currentAgent)}. How can I help you today?</p>
        <div class="flex flex-wrap gap-2 mt-4">
          <button onclick="send('What can you do?')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">What can you do?</button>
          <button onclick="send('Search the web for today\\'s top tech news')" class="text-xs sm:text-sm bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-600 rounded-xl px-3 sm:px-4 py-1.5 sm:py-2 transition-all">Today's news</button>
        </div>
      </div>
    </div>`;
}

async function newConversation() {
  conversationId = null;
  clearMessagesUI();
  document.getElementById('input').focus();
}

async function reloadCurrentConversation() {
  if (!conversationId) return;
  // Clear and reload all messages for the current conversation
  const container = document.getElementById('messages');
  container.innerHTML = '';
  await loadConversationMessages(conversationId);
}

function handleKey(e) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendFromInput();
  }
}

function autoGrow(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, 200) + 'px';
}

function sendFromInput() {
  const el = document.getElementById('input');
  const msg = el.value.trim();
  if (!msg || sending) return;
  el.value = '';
  el.style.height = 'auto';
  send(msg);
}

async function send(msg) {
  if (sending) return;
  sending = true;
  const btn = document.getElementById('send-btn');
  btn.disabled = true;

  // Hide welcome suggestions after first message
  const welcome = document.getElementById('welcome-msg');
  if (welcome) {
    const chips = welcome.querySelector('.flex.flex-wrap');
    if (chips) chips.remove();
  }

  const container = document.getElementById('messages');
  const scroll = document.getElementById('messages-scroll');

  // User message
  const userEl = document.createElement('div');
  userEl.className = 'flex gap-4 justify-end';
  userEl.innerHTML = `
    <div class="max-w-[75%]">
      <div class="bg-oc-900/50 border border-oc-800/40 rounded-2xl rounded-br-md px-5 py-3 text-sm text-gray-200 leading-relaxed">${esc(msg)}</div>
    </div>`;
  container.appendChild(userEl);

  // AI thinking
  const aiEl = document.createElement('div');
  aiEl.className = 'flex gap-4';
  aiEl.innerHTML = `
    <img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
    <div class="flex-1 min-w-0" id="ai-response">
      <div class="flex items-center gap-2 text-gray-500 text-sm py-2">
        <span class="flex gap-1">
          <span class="w-1.5 h-1.5 rounded-full bg-gray-500 animate-bounce" style="animation-delay:0ms"></span>
          <span class="w-1.5 h-1.5 rounded-full bg-gray-500 animate-bounce" style="animation-delay:150ms"></span>
          <span class="w-1.5 h-1.5 rounded-full bg-gray-500 animate-bounce" style="animation-delay:300ms"></span>
        </span>
      </div>
    </div>`;
  container.appendChild(aiEl);
  scroll.scrollTop = scroll.scrollHeight;

  const responseEl = aiEl.querySelector('#ai-response');
  responseEl.removeAttribute('id');
  const t0 = Date.now();

  try {
    // Start the turn — get a turn_id for streaming
    // Create conversation if we don't have one
    if (!conversationId) {
      try {
        const cr = await fetch('/api/conversations', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ token, agent: getAgentId(currentAgent) })
        });
        const cd = await cr.json();
        if (cd.id) conversationId = cd.id;
      } catch(e) {}
    }

    const startResp = await fetch('/api/message/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: msg, agent: getAgentId(currentAgent), token, conversation_id: conversationId })
    });
    const startData = await startResp.json();
    const turnId = startData.turn_id;

    if (!turnId) {
      // Fallback to non-streaming
      const resp = await fetch('/api/message', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: msg, agent: getAgentId(currentAgent), token, conversation_id: conversationId })
      });
      const data = await resp.json();
      const secs = ((Date.now() - t0) / 1000).toFixed(1);
      let replyHtml = data.response
        ? `<div class="text-gray-300 leading-relaxed text-sm chat-md">${md(data.response)}</div>
           <div class="flex items-center gap-3 mt-2"><span class="text-xs text-gray-600">${secs}s</span>
           <button onclick="copy(this)" class="text-xs text-gray-600 hover:text-gray-400" data-t="${escAttr(data.response)}">Copy</button></div>`
        : `<p class="text-red-400 text-sm">${esc(data.error || 'No response')}</p>`;
      if (data.escalation) {
        const e = data.escalation;
        replyHtml += `<div class="mt-3 p-3 rounded-lg border border-sky-800/50 bg-sky-900/20">
          <p class="text-sky-300 text-sm mb-2">${esc(e.message)}</p>
          <div class="flex gap-2">
            <button onclick="acceptHandoff('${e.module}')"
              class="px-3 py-1 text-xs font-medium rounded bg-sky-600 hover:bg-sky-500 text-white transition-colors">
              Open ${esc(e.agent_name)} module</button>
            <button onclick="dismissEscalation('${e.module}')"
              class="px-3 py-1 text-xs font-medium rounded bg-gray-700 hover:bg-gray-600 text-gray-300 transition-colors">
              Keep going here</button>
          </div></div>`;
      }
      responseEl.innerHTML = replyHtml;
    } else {
      // Stream events via SSE
      const evtSource = new EventSource(`/api/message/${turnId}/stream?token=${token}`);
      let toolsUsed = [];

      // Split the response container into a persistent thought line + a
      // status area. `thinking` events only touch the thought line so
      // subsequent tool-call renders don't wipe them.
      responseEl.innerHTML = '<div class="persona-thought text-xs text-gray-500 italic mb-1.5 opacity-0 transition-opacity duration-300"></div><div class="status-area"></div>';
      const statusArea = responseEl.querySelector('.status-area');
      const thoughtEl = responseEl.querySelector('.persona-thought');

      evtSource.onmessage = (event) => {
        const ev = JSON.parse(event.data);
        switch (ev.event) {
          case 'started':
            statusArea.innerHTML = `<div class="flex items-center gap-2 text-gray-500 text-sm py-1">
              <span class="flex gap-1"><span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:150ms"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:300ms"></span></span>
              <span class="text-xs status-text">Thinking...</span></div>`;
            break;
          case 'llm_call_started':
            const st = statusArea.querySelector('.status-text');
            if (st) st.textContent = ev.round > 0 ? `Round ${ev.round + 1}...` : 'Thinking...';
            break;
          case 'thinking':
            // Persona-flavored grey-text thought while the model works.
            // Lives in its own .persona-thought element that survives
            // innerHTML swaps on .status-area.
            thoughtEl.style.opacity = '0';
            thoughtEl.textContent = ev.text || '';
            requestAnimationFrame(() => { thoughtEl.style.opacity = '0.75'; });
            break;
          case 'tool_call_started':
            toolsUsed.push(ev.tool_name);
            statusArea.innerHTML = `<div class="space-y-1">${toolsUsed.map(t =>
              `<div class="flex items-center gap-2 text-xs text-gray-500">
                <span class="w-1 h-1 rounded-full bg-oc-500 animate-pulse"></span>
                Using <span class="text-gray-400">${esc(t)}</span>...</div>`).join('')}</div>`;
            break;
          case 'tool_call_completed':
            statusArea.querySelectorAll('.text-xs').forEach(el => {
              if (el.textContent.includes(ev.tool_name)) {
                const dot = el.querySelector('.animate-pulse');
                if (dot) { dot.classList.remove('animate-pulse','bg-oc-500'); dot.classList.add(ev.success?'bg-green-500':'bg-red-500'); }
              }
            });
            break;
          case 'complete':
            evtSource.close();
            const secs = ((Date.now() - t0) / 1000).toFixed(1);
            const rounds = ev.rounds > 1 ? ` &middot; ${ev.rounds} rounds` : '';
            const tools = toolsUsed.length ? ` &middot; ${toolsUsed.join(', ')}` : '';
            // Strip the [HANDBACK] control marker from the user-visible text
            // — it's internal metadata used to route agent switching.
            const displayResp = (ev.response || '').replace(/\s*\[HANDBACK\]\s*/gi, '');
            responseEl.innerHTML = `<div class="text-gray-300 leading-relaxed text-sm chat-md">${md(displayResp)}</div>
              <div class="flex items-center gap-3 mt-2"><span class="text-xs text-gray-600">${secs}s${rounds}${tools}</span>
              <button onclick="copy(this)" class="text-xs text-gray-600 hover:text-gray-400" data-t="${escAttr(displayResp)}">Copy</button></div>`;
            scroll.scrollTop = scroll.scrollHeight;
            if (voiceInitiated) { playTts(ev.response); voiceInitiated = false; }
            sending = false; btn.disabled = false;
            document.getElementById('input').focus();
            return;
          case 'error':
            evtSource.close();
            responseEl.innerHTML = `<p class="text-red-400 text-sm">${esc(ev.message)}</p>`;
            sending = false; btn.disabled = false;
            return;
        }
        scroll.scrollTop = scroll.scrollHeight;
      };
      evtSource.onerror = () => {
        evtSource.close();
        if (responseEl.querySelector('.animate-bounce')) responseEl.innerHTML = '<p class="text-red-400 text-sm">Connection lost</p>';
        sending = false; btn.disabled = false;
      };
      return;
    }
  } catch(e) {
    responseEl.innerHTML = `<p class="text-red-400 text-sm">Connection error: ${esc(e.message)}</p>`;
  }

  scroll.scrollTop = scroll.scrollHeight;
  sending = false;
  btn.disabled = false;
  document.getElementById('input').focus();
}

function clearMessages() {
  newConversation();
}

function acceptHandoff(module) {
      // Navigate to the specialist module page
      const moduleUrls = {
        tax: '/tax', research: '/knowledge', music: '/music',
        scheduler: '/settings', coders: '/coders', journal: '/journal'
      };
      const url = moduleUrls[module] || '/' + module;
      window.location.href = url;
    }
    function dismissEscalation(module) {
      // Tell the backend to suppress this escalation
      const token = localStorage.getItem('syntaur_token') || '';
      fetch('/api/agents/dismiss_escalation', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token, conversation_id: conversationId || 'ephemeral', module })
      }).catch(() => {});
      // Remove the escalation UI from the current message
      event.target.closest('.border-sky-800\\/50')?.remove();
    }
    function copy(btn) {
  navigator.clipboard.writeText(btn.dataset.t).then(() => {
    btn.textContent = 'Copied!';
    setTimeout(() => btn.textContent = 'Copy', 1500);
  });
}

function esc(s) {
  return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\n/g,'<br>');
}
function escAttr(s) {
  return s.replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function md(text) {
  let h = esc(text);
  // Code blocks
  h = h.replace(/```(\w*)<br>([\s\S]*?)```/g, (_, lang, code) => {
    const l = lang ? `<span class="absolute top-2 right-3 text-xs text-gray-500">${lang}</span>` : '';
    return `<div class="relative my-3 group"><pre class="bg-gray-900 border border-gray-800 rounded-xl p-4 overflow-x-auto text-xs text-gray-300">${l}${code.replace(/<br>/g,'\n')}</pre><button onclick="navigator.clipboard.writeText(this.previousElementSibling.textContent)" class="absolute top-2 right-10 text-xs text-gray-600 hover:text-gray-400 opacity-0 group-hover:opacity-100 transition-opacity">Copy</button></div>`;
  });
  // Inline code
  h = h.replace(/`([^`]+)`/g, '<code class="bg-gray-800/80 px-1.5 py-0.5 rounded-md text-oc-400 text-xs">$1</code>');
  // Bold
  h = h.replace(/\*\*([^*]+)\*\*/g, '<strong class="text-white font-semibold">$1</strong>');
  // Italic
  h = h.replace(/\*([^*]+)\*/g, '<em class="text-gray-200">$1</em>');
  // Links
  h = h.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" class="text-oc-500 hover:text-oc-400 underline underline-offset-2">$1</a>');
  // Bullets
  h = h.replace(/(^|<br>)- (.+?)(?=<br>|$)/g, '$1<div class="flex gap-2 pl-1"><span class="text-gray-600 mt-0.5">&bull;</span><span>$2</span></div>');
  // Numbered
  h = h.replace(/(^|<br>)(\d+)\. (.+?)(?=<br>|$)/g, '$1<div class="flex gap-2 pl-1"><span class="text-gray-500 w-5 text-right flex-shrink-0">$2.</span><span>$3</span></div>');
  // H3
  h = h.replace(/(^|<br>)### (.+?)(?=<br>|$)/g, '$1<p class="font-semibold text-white mt-3 mb-1">$2</p>');
  // H2
  h = h.replace(/(^|<br>)## (.+?)(?=<br>|$)/g, '$1<p class="font-bold text-white text-base mt-3 mb-1">$2</p>');
  // Fix-options → clickable buttons (runs last, operates on rendered HTML)
  h = renderFixOptions(h);
  return h;
}

// Detect "Fix options — reply with a number" blocks emitted by tool failures
// (see src/tools/image_gen.rs format_image_failure) and turn the numbered
// items into clickable approval buttons. Clicking a button submits the
// number as the user's next message — the agent's STYLE.md teaches it to
// read the preceding options block and execute the chosen action.
function renderFixOptions(html) {
  // The marker is a bolded header containing "Fix options"; be lenient on
  // punctuation + surrounding tags so agents that rephrase still trigger.
  const markerRe = /<strong[^>]*>\s*Fix options[^<]*<\/strong>\s*(?:<br>)?/i;
  const m = html.match(markerRe);
  if (!m) return html;
  const prefix = html.slice(0, m.index + m[0].length);
  const after = html.slice(m.index + m[0].length);
  // Each numbered item rendered by the Numbered rule above is:
  //   <div class="flex gap-2 pl-1"><span class="text-gray-500 w-5 text-right flex-shrink-0">N.</span><span>INNER</span></div>
  const itemRe = /<div class="flex gap-2 pl-1"><span class="text-gray-500 w-5 text-right flex-shrink-0">(\d+)\.<\/span><span>([\s\S]*?)<\/span><\/div>/g;
  const items = [];
  let match, firstStart = -1, lastEnd = -1;
  while ((match = itemRe.exec(after)) !== null) {
    if (firstStart < 0) firstStart = match.index;
    lastEnd = match.index + match[0].length;
    items.push({ num: match[1], inner: match[2] });
  }
  if (items.length < 2) return html; // not enough options to warrant buttons
  // Build a horizontal button row to replace the numbered list.
  let btnHtml = '<div class="mt-2.5 mb-1 flex flex-wrap gap-2">';
  for (const it of items) {
    const labelMatch = it.inner.match(/<strong[^>]*>([^<]+)<\/strong>/);
    const label = labelMatch ? labelMatch[1].trim() : `Option ${it.num}`;
    const safeLabel = label.replace(/"/g, '&quot;');
    btnHtml += `<button onclick="pickFixOption(${it.num}, this)" data-label="${safeLabel}" ` +
      `class="px-3 py-1.5 bg-oc-600 hover:bg-oc-700 text-white rounded-lg text-sm font-medium transition-colors shadow-sm">` +
      esc(label) +
      `</button>`;
  }
  btnHtml += '</div>';
  // Keep the original numbered block so the agent can still read the full
  // details when it processes this message as context on its next turn.
  // Buttons appear ABOVE the numbered list.
  return prefix + btnHtml + after.slice(0, firstStart) +
    `<div class="text-xs text-gray-500 italic mt-1">Or reply with the number:</div>` +
    after.slice(firstStart, lastEnd) + after.slice(lastEnd);
}

// User clicked a fix-option button — submit the number as a user message,
// then visually mark all sibling buttons as disabled so they don't get
// clicked twice.
function pickFixOption(num, btn) {
  if (btn) {
    const row = btn.parentElement;
    if (row) {
      for (const b of row.children) {
        b.disabled = true;
        b.classList.remove('bg-oc-600','hover:bg-oc-700');
        b.classList.add('bg-gray-700','cursor-not-allowed','opacity-60');
      }
      btn.classList.remove('bg-gray-700','opacity-60');
      btn.classList.add('bg-oc-700','opacity-100');
    }
  }
  // "Let my debug specialist look at this" special-cases to a handoff:
  // switch the agent selector to Maurice and submit a context message.
  // Detect via label text so we don't need to smuggle a marker through the
  // failure-message format.
  const label = btn ? (btn.dataset.label || btn.textContent || '') : '';
  if (/\b(debug specialist|specialist look|get.*specialist)\b/i.test(label)
      || /\bmaurice\b/i.test(label)) {
    handoffToMaurice(btn);
    return;
  }
  const input = document.getElementById('input');
  if (!input) return;
  input.value = String(num);
  if (typeof sendMessage === 'function') sendMessage();
}

// Hand off to Maurice — navigate the user FROM /chat TO /coders, which is
// Maurice's dedicated module (terminal, debug context panel, code view).
// This is a full page transition, not a swap within the chat. When Maurice
// is done he asks the user whether to return; on yes, /coders posts the
// outcome back to the original conversation and navigates back here.
//
// URL params carried to /coders:
//   ?handoff=1 — puts the module in "someone sent you here" mode
//   &return_agent=<display_name> — who to hand back to
//   &conv_id=<id> — conversation to post the outcome into on return
//   &ctx=<base64 url-safe> — the failure message text so Maurice opens
//                            with the right context already loaded
function handoffToMaurice(btn) {
  const priorAgent = currentAgent || 'Peter';
  let ctx = '';
  let el = btn ? btn.closest('.chat-md, [data-ai-msg]') : null;
  if (!el) {
    const msgs = document.querySelectorAll('#messages .chat-md');
    if (msgs.length) el = msgs[msgs.length - 1];
  }
  if (el) ctx = (el.innerText || '').trim().slice(0, 2000);

  // base64url-encode the context so it survives URL transport cleanly
  let ctxEnc = '';
  try {
    ctxEnc = btoa(unescape(encodeURIComponent(ctx)))
      .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  } catch(e) { ctxEnc = ''; }

  const params = new URLSearchParams();
  params.set('handoff', '1');
  params.set('return_agent', priorAgent);
  if (conversationId) params.set('conv_id', conversationId);
  if (ctxEnc) params.set('ctx', ctxEnc);
  // Preserve the auth token so the /coders page can authenticate immediately.
  if (token) params.set('token', token);

  // Navigate — /coders takes over from here.
  location.href = '/coders?' + params.toString();
}

// Drag and drop
document.addEventListener('dragover', (e) => {
  e.preventDefault();
  document.getElementById('drop-overlay').classList.remove('hidden');
});
document.addEventListener('dragleave', (e) => {
  if (e.relatedTarget === null) document.getElementById('drop-overlay').classList.add('hidden');
});
document.addEventListener('drop', async (e) => {
  e.preventDefault();
  document.getElementById('drop-overlay').classList.add('hidden');
  for (const file of e.dataTransfer.files) await addPendingFile(file);
});

function handleFileSelect(event) {
  for (const file of event.target.files) addPendingFile(file);
  event.target.value = '';
}

async function addPendingFile(file) {
  // Check if this might be a financial document (image or PDF under 10MB)
  const isImage = file.type.startsWith('image/') || file.type === 'application/pdf';
  const couldBeFinancial = isImage && file.size < 10 * 1024 * 1024;

  if (couldBeFinancial && localStorage.getItem('syntaur_skip_doc_detect') !== '1') {
    // Show detection prompt — let user decide what to do
    showDocumentDetection(file);
    return;
  }

  // Regular file attachment
  attachFileToChat(file);
}

async function attachFileToChat(file) {
  const container = document.getElementById('pending-files');
  container.classList.remove('hidden');
  const sizeStr = file.size > 1048576 ? `${(file.size/1048576).toFixed(1)}MB` : `${(file.size/1024).toFixed(0)}KB`;

  const pill = document.createElement('div');
  pill.className = 'flex items-center gap-2 bg-gray-800 border border-gray-700 rounded-lg px-3 py-1.5 text-xs';
  pill.innerHTML = '<span class="text-gray-400 animate-pulse">Uploading...</span>';
  container.appendChild(pill);

  const formData = new FormData();
  formData.append('file', file);

  try {
    const resp = await fetch('/api/upload', { method: 'POST', body: formData });
    const data = await resp.json();
    if (data.success) {
      const idx = pendingFiles.length;
      pendingFiles.push({ filename: data.filename, path: data.path, preview: data.preview, size: sizeStr });
      pill.innerHTML = `<span class="text-oc-500">&#128206;</span><span class="text-gray-300">${esc(data.filename)}</span><span class="text-gray-600">${sizeStr}</span><button onclick="this.parentElement.remove();pendingFiles[${idx}]=null" class="text-gray-600 hover:text-red-400">&times;</button>`;
    } else {
      pill.innerHTML = '<span class="text-red-400">Failed</span>';
      setTimeout(() => pill.remove(), 2000);
    }
  } catch(e) {
    pill.innerHTML = '<span class="text-red-400">Error</span>';
    setTimeout(() => pill.remove(), 2000);
  }
}

// Smart document detection — classify then prompt user
async function showDocumentDetection(file) {
  const container = document.getElementById('messages');
  const scroll = document.getElementById('messages-scroll');
  const msgEl = document.createElement('div');
  msgEl.className = 'flex gap-4';

  const previewUrl = URL.createObjectURL(file);
  const isPdf = file.type === 'application/pdf';

  msgEl.innerHTML = `
    <img src="/agent-avatar/main" class="w-8 h-8 rounded-lg flex-shrink-0" alt="">
    <div class="flex-1 min-w-0">
      <div class="p-3 rounded-lg bg-gray-800 border border-gray-700">
        <div class="flex items-start gap-3">
          ${isPdf
            ? '<div class="w-20 h-20 rounded-lg bg-gray-900 flex items-center justify-center flex-shrink-0"><span class="text-2xl">&#128196;</span></div>'
            : `<img src="${previewUrl}" class="w-20 h-20 object-cover rounded-lg flex-shrink-0">`}
          <div class="flex-1" id="doc-detect-content">
            <p class="text-sm text-gray-300">Identifying document...</p>
            <div class="flex gap-1 mt-2">
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:150ms"></span>
              <span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:300ms"></span>
            </div>
          </div>
        </div>
      </div>
    </div>`;
  container.appendChild(msgEl);
  scroll.scrollTop = scroll.scrollHeight;

  const contentEl = msgEl.querySelector('#doc-detect-content');
  contentEl.removeAttribute('id');

  // Use smart upload to classify
  try {
    const resp = await fetch(`/api/tax/upload?token=${token}`, {
      method: 'POST',
      headers: { 'Content-Type': file.type },
      body: file
    });
    const data = await resp.json();
    const docType = data.doc_type || 'unknown';
    const routedTo = data.routed_to || 'unknown';
    const docId = data.id;

    const typeLabels = {
      'receipt': 'Receipt', 'invoice': 'Invoice',
      'w2': 'W-2 Tax Form', '1099_int': '1099-INT', '1099_div': '1099-DIV',
      '1099_b': '1099-B', '1099_misc': '1099-MISC', '1099_nec': '1099-NEC',
      '1095_c': '1095-C Health Coverage',
      'mortgage_statement': 'Mortgage Statement (1098)',
      'property_tax_statement': 'Property Tax Statement',
      'bank_statement': 'Bank Statement', 'credit_card_statement': 'Credit Card Statement',
      'insurance_policy': 'Insurance Document', 'settlement_statement': 'Settlement Statement',
      'other': 'Document',
    };
    const label = typeLabels[docType] || docType;

    const isFinancial = docType !== 'other' && docType !== 'unknown';
    const moduleLocked = data.module_locked === true;

    if (isFinancial && moduleLocked) {
      // Financial doc detected but module is locked — show classification + trial prompt
      const dismissCount = parseInt(localStorage.getItem('syntaur_doc_dismiss_count') || '0');
      contentEl.innerHTML = `
        <p class="text-sm text-gray-300">I noticed this looks like a <strong class="text-white">${esc(label)}</strong>.</p>
        <p class="text-xs text-gray-500 mt-1">File saved. Unlock the Tax Module to scan and track it automatically.</p>
        <div class="flex flex-wrap gap-2 mt-3">
          <button onclick="startTrialFromChat(this)" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded-lg transition-colors">
            Start Free 3-Day Trial
          </button>
          <button onclick="dismissDocDetect(this)" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1.5 rounded-lg transition-colors">
            No thanks
          </button>
          ${dismissCount >= 1 ? `<button onclick="disableDocDetect(this)" class="text-xs text-gray-600 hover:text-gray-400 px-2 py-1.5 transition-colors">Don't remind me again</button>` : ''}
        </div>`;
    } else if (isFinancial) {
      // Financial document detected + module unlocked — show prompt with options
      const isStatement = routedTo === 'statement';
      const isReceipt = routedTo === 'receipt';
      const processingMsg = isReceipt ? 'Scanning for vendor, amount, and category...'
        : isStatement ? 'Extracting transactions...'
        : 'Extracting fields...';

      const dismissCount2 = parseInt(localStorage.getItem('syntaur_doc_dismiss_count') || '0');
      contentEl.innerHTML = `
        <p class="text-sm text-gray-300">I noticed this looks like a <strong class="text-white">${esc(label)}</strong>.</p>
        <p class="text-xs text-gray-500 mt-1">${processingMsg}</p>
        <div class="flex flex-wrap gap-2 mt-3">
          <a href="/tax" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded-lg transition-colors inline-flex items-center gap-1">
            Open Tax Module &#8594;
          </a>
          <button onclick="dismissDocDetect(this)" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1.5 rounded-lg transition-colors">
            Got it
          </button>
          ${dismissCount2 >= 1 ? `<button onclick="disableDocDetect(this)" class="text-xs text-gray-600 hover:text-gray-400 px-2 py-1.5 transition-colors">Don't remind me again</button>` : ''}
        </div>`;

      // If it's a receipt, poll for scan results and update inline
      if (isReceipt && docId) {
        pollReceiptInline(docId, contentEl, previewUrl);
      }
    } else {
      // Not a financial document — attach as regular file instead
      contentEl.innerHTML = `
        <p class="text-sm text-gray-400">This doesn't look like a financial document. Attaching to chat instead.</p>`;
      // Remove the smart-upload copy and attach normally
      setTimeout(() => msgEl.remove(), 3000);
      attachFileToChat(file);
    }
  } catch(e) {
    // Classification failed — fall back to regular attachment
    contentEl.innerHTML = `<p class="text-sm text-gray-400">Attaching file to chat.</p>`;
    setTimeout(() => msgEl.remove(), 2000);
    attachFileToChat(file);
  }
}

async function pollReceiptInline(receiptId, el, previewUrl) {
  const poll = async () => {
    try {
      const resp = await fetch(`/api/tax/receipts?token=${token}`);
      const data = await resp.json();
      const receipt = (data.receipts || []).find(r => r.id === receiptId);
      if (receipt && receipt.status === 'scanned') {
        // Update the prompt with extracted data
        const existingButtons = el.querySelector('.flex.flex-wrap.gap-2');
        const resultsHtml = `
          <div class="grid grid-cols-2 gap-x-4 gap-y-1 mt-2 text-xs border-t border-gray-700 pt-2">
            <div><span class="text-gray-500">Vendor:</span> <span class="text-gray-300">${esc(receipt.vendor || '?')}</span></div>
            <div><span class="text-gray-500">Amount:</span> <span class="text-white font-medium">${receipt.amount_display || '?'}</span></div>
            <div><span class="text-gray-500">Date:</span> <span class="text-gray-300">${receipt.receipt_date || '?'}</span></div>
            <div><span class="text-gray-500">Category:</span> <span class="text-gray-300">${receipt.category || '?'}</span></div>
          </div>
          <p class="text-xs text-green-500 mt-1">&#10004; Expense saved</p>`;
        if (existingButtons) existingButtons.insertAdjacentHTML('beforebegin', resultsHtml);
        else el.insertAdjacentHTML('beforeend', resultsHtml);
        return;
      }
      if (receipt && receipt.status === 'tax_form') {
        el.querySelector('.text-gray-500')?.remove();
        el.insertAdjacentHTML('beforeend', '<p class="text-xs text-yellow-400 mt-1">This looks like a tax form, not a receipt. Saved as a tax document instead.</p>');
        return;
      }
    } catch(e) {}
    setTimeout(poll, 2500);
  };
  setTimeout(poll, 3000);
}

function dismissDocDetect(btn) {
  const count = parseInt(localStorage.getItem('syntaur_doc_dismiss_count') || '0') + 1;
  localStorage.setItem('syntaur_doc_dismiss_count', count);
  const msg = btn.closest('.flex.gap-4');
  if (msg) msg.remove();
}

function disableDocDetect(btn) {
  localStorage.setItem('syntaur_skip_doc_detect', '1');
  const msg = btn.closest('.flex.gap-4');
  if (msg) {
    const content = msg.querySelector('.flex-1');
    if (content) content.innerHTML = '<p class="text-xs text-gray-500 p-3">Document detection disabled. You can re-enable it in Settings.</p>';
    setTimeout(() => msg.remove(), 2000);
  }
}

async function startTrialFromChat(btn) {
  btn.textContent = 'Starting trial...';
  btn.disabled = true;
  try {
    const resp = await fetch('/api/modules/trial', {
      method: 'POST',
      headers: {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token},
      body: JSON.stringify({ token, module: 'tax' })
    });
    const data = await resp.json();
    if (data.granted) {
      const parent = btn.closest('.flex-1');
      if (parent) {
        parent.innerHTML = `
          <p class="text-sm text-green-400">Tax Module trial started! ${data.trial_days_left} days remaining.</p>
          <p class="text-xs text-gray-500 mt-1">Drop your document again to scan it, or <a href="/tax" class="text-oc-500 hover:text-oc-400">open the Tax Module</a>.</p>`;
      }
    } else {
      btn.textContent = 'Trial already used';
    }
  } catch(e) {
    btn.textContent = 'Error — try again';
    btn.disabled = false;
  }
}

// Override send to include file context
const _origSend = sendFromInput;
sendFromInput = function() {
  const el = document.getElementById('input');
  let msg = el.value.trim();
  const files = pendingFiles.filter(f => f !== null);
  if (files.length > 0) {
    const ctx = files.map(f => `\n\n--- File: ${f.filename} (${f.size}) ---\n${f.preview || '[binary]'}\n--- End ---`).join('');
    if (!msg) msg = 'Please analyze the attached file' + (files.length > 1 ? 's' : '') + ':';
    msg += ctx;
    pendingFiles = [];
    document.getElementById('pending-files').innerHTML = '';
    document.getElementById('pending-files').classList.add('hidden');
  }
  if (!msg || sending) return;
  el.value = '';
  el.style.height = 'auto';
  send(msg);
};

document.getElementById('input').focus();


// ── Voice input (mic button) ─────────────────────────────────────
let voiceWs = null;
let voiceRecording = false;
let voiceAudioCtx = null;
let voiceStream = null;
let micClickTime = 0;
let selectedMicId = localStorage.getItem('syntaur_mic_id') || '';

function connectVoiceWs() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  voiceWs = new WebSocket(`${proto}//${location.host}/ws/stt`);
  voiceWs.binaryType = 'arraybuffer';
  voiceWs.onclose = () => { voiceWs = null; };
  voiceWs.onerror = () => { if (voiceWs) voiceWs.close(); };
  voiceWs.onmessage = (e) => {
    try {
      const msg = JSON.parse(e.data);
      if (msg.type === 'transcript' && msg.text && msg.text.trim()) {
        voiceInitiated = true;
        const input = document.getElementById('input');
        input.value = msg.text.trim();
        autoGrow(input);
        // Auto-send voice messages
        sendFromInput();
      }
    } catch {}
  };
}

// Enumerate mic devices and populate menu
async function loadMicDevices() {
  try {
    const tempStream = await navigator.mediaDevices.getUserMedia({ audio: true });
    tempStream.getTracks().forEach(t => t.stop());

    const devices = await navigator.mediaDevices.enumerateDevices();
    const mics = devices.filter(d => d.kind === 'audioinput');
    const container = document.getElementById('mic-devices');
    container.innerHTML = '';
    mics.forEach((mic, i) => {
      const isActive = mic.deviceId === selectedMicId || (!selectedMicId && i === 0);
      const btn = document.createElement('button');
      btn.className = `w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-xs transition-colors ${isActive ? 'bg-oc-900/50 text-oc-400' : 'text-gray-400 hover:bg-gray-800 hover:text-white'}`;
      btn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 1a3 3 0 00-3 3v8a3 3 0 006 0V4a3 3 0 00-3-3z"/><path d="M19 10v2a7 7 0 01-14 0v-2"/></svg>
        <span class="truncate">${mic.label || 'Microphone ' + (i+1)}</span>
        ${isActive ? '<span class="ml-auto text-oc-500">&#10003;</span>' : ''}`;
      btn.onclick = () => { selectedMicId = mic.deviceId; localStorage.setItem('syntaur_mic_id', selectedMicId); loadMicDevices(); };
      container.appendChild(btn);
    });
  } catch {}
}

function showPhoneQr() {
  const area = document.getElementById('phone-qr-area');
  const img = document.getElementById('phone-qr-img');
  const link = document.getElementById('phone-qr-link');
  const setupUrl = `http://${location.hostname}:18803`;
  img.src = `http://${location.hostname}:18803/qr.svg`;
  link.href = setupUrl;
  link.textContent = setupUrl;
  area.classList.toggle('hidden');
}

function toggleMicMenu(e) {
  const elapsed = Date.now() - micClickTime;
  if (elapsed < 300) {
    const menu = document.getElementById('mic-menu');
    menu.classList.toggle('hidden');
    if (!menu.classList.contains('hidden')) loadMicDevices();
  }
}

// Close mic menu when clicking outside
document.addEventListener('click', (e) => {
  const menu = document.getElementById('mic-menu');
  if (menu && !menu.contains(e.target) && e.target.id !== 'mic-btn' && !e.target.closest('#mic-btn')) {
    menu.classList.add('hidden');
  }
});

async function startVoice(e) {
  e.preventDefault();
  micClickTime = Date.now();
  if (voiceRecording) return;

  if (!voiceWs || voiceWs.readyState !== 1) {
    connectVoiceWs();
    await new Promise(r => {
      const check = setInterval(() => {
        if (voiceWs && voiceWs.readyState === 1) { clearInterval(check); r(); }
      }, 50);
      setTimeout(() => { clearInterval(check); r(); }, 2000);
    });
  }
  if (!voiceWs || voiceWs.readyState !== 1) return;

  const audioConstraints = {
    sampleRate: 16000, channelCount: 1,
    echoCancellation: true, noiseSuppression: true, autoGainControl: true
  };
  if (selectedMicId) audioConstraints.deviceId = { exact: selectedMicId };

  try {
    voiceStream = await navigator.mediaDevices.getUserMedia({ audio: audioConstraints });
  } catch {
    // If exact device fails, try default
    try {
      delete audioConstraints.deviceId;
      voiceStream = await navigator.mediaDevices.getUserMedia({ audio: audioConstraints });
    } catch { return; }
  }

  voiceAudioCtx = new AudioContext({ sampleRate: 16000 });
  const source = voiceAudioCtx.createMediaStreamSource(voiceStream);
  const processor = voiceAudioCtx.createScriptProcessor(4096, 1, 1);
  processor.onaudioprocess = (ev) => {
    if (!voiceRecording || !voiceWs || voiceWs.readyState !== 1) return;
    const f = ev.inputBuffer.getChannelData(0);
    const pcm = new Int16Array(f.length);
    for (let i = 0; i < f.length; i++)
      pcm[i] = Math.max(-32768, Math.min(32767, Math.round(f[i] * 32767)));
    voiceWs.send(pcm.buffer);
  };
  source.connect(processor);
  processor.connect(voiceAudioCtx.destination);

  voiceWs.send(JSON.stringify({ type: 'start' }));
  voiceRecording = true;

  const btn = document.getElementById('mic-btn');
  btn.classList.add('text-red-500');
  btn.classList.remove('text-gray-500');
  // Hide mic menu while recording
  document.getElementById('mic-menu').classList.add('hidden');
}

function stopVoice(e) {
  e.preventDefault();
  if (!voiceRecording) return;
  voiceRecording = false;

  if (voiceWs && voiceWs.readyState === 1) {
    voiceWs.send(JSON.stringify({ type: 'stop' }));
  }
  if (voiceStream) { voiceStream.getTracks().forEach(t => t.stop()); voiceStream = null; }
  if (voiceAudioCtx) { voiceAudioCtx.close(); voiceAudioCtx = null; }

  const btn = document.getElementById('mic-btn');
  btn.classList.remove('text-red-500');
  btn.classList.add('text-gray-500');
}



// ── Voice Mode (continuous conversation) ─────────────────────────
const VM_SILENCE_THRESHOLD = 1200;   // RMS energy (browser mic is louder than satellite)
const VM_SILENCE_CHUNKS = 8;         // ~330ms at 4096 samples/chunk @ 16kHz
const VM_MAX_SILENCE_S = 30;         // exit voice mode after 30s total silence
const VM_ECHO_WORDS = new Set([
  'yeah','yes','yep','yup','no','nah','nope','ok','okay','sure','right',
  'mm','hmm','uh','huh','oh','ah','so','well','hey','hi',
  'thanks','cool','nice','good','great','alright',
  'yeah yeah','ok ok','all right','oh yeah','oh ok','sounds good',
  'got it','thank you','you good','for sure','no problem','of course'
]);

let vmActive = false;
let vmWs = null;
let vmAudioCtx = null;
let vmStream = null;
let vmState = 'idle'; // idle, listening, speaking, thinking, playing, cooldown
let vmSilenceCount = 0;
let vmHeardSpeech = false;
let vmLastTtsDuration = 3;
let vmCooldownTimer = null;
let vmIdleTimer = null;

function toggleVoiceMode() {
  if (vmActive) {
    stopVoiceMode();
  } else {
    startVoiceMode();
  }
}

async function startVoiceMode() {
  vmActive = true;
  document.getElementById('voice-mode-overlay').classList.remove('hidden');
  document.getElementById('voice-mode-btn').classList.add('text-oc-500');
  document.getElementById('voice-mode-btn').classList.remove('text-gray-500');

  // Connect STT WebSocket
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  vmWs = new WebSocket(`${proto}//${location.host}/ws/stt`);
  vmWs.binaryType = 'arraybuffer';
  vmWs.onclose = () => { if (vmActive) setTimeout(startVoiceMode, 1000); };
  vmWs.onerror = () => vmWs.close();
  vmWs.onmessage = handleVmTranscript;

  await new Promise(r => {
    const check = setInterval(() => {
      if (vmWs && vmWs.readyState === 1) { clearInterval(check); r(); }
    }, 50);
    setTimeout(() => { clearInterval(check); r(); }, 2000);
  });

  // Start mic with VAD
  const constraints = { sampleRate: 16000, channelCount: 1,
    echoCancellation: true, noiseSuppression: true, autoGainControl: true };
  if (selectedMicId) constraints.deviceId = { exact: selectedMicId };

  try {
    vmStream = await navigator.mediaDevices.getUserMedia({ audio: constraints });
  } catch {
    try {
      delete constraints.deviceId;
      vmStream = await navigator.mediaDevices.getUserMedia({ audio: constraints });
    } catch { stopVoiceMode(); return; }
  }

  vmAudioCtx = new AudioContext({ sampleRate: 16000 });
  const source = vmAudioCtx.createMediaStreamSource(vmStream);
  const processor = vmAudioCtx.createScriptProcessor(4096, 1, 1);
  processor.onaudioprocess = handleVmAudio;
  source.connect(processor);
  processor.connect(vmAudioCtx.destination);

  setVmState('listening');
  resetVmIdle();
}

function stopVoiceMode() {
  vmActive = false;
  if (vmWs) { vmWs.close(); vmWs = null; }
  if (vmStream) { vmStream.getTracks().forEach(t => t.stop()); vmStream = null; }
  if (vmAudioCtx) { vmAudioCtx.close(); vmAudioCtx = null; }
  if (vmCooldownTimer) { clearTimeout(vmCooldownTimer); vmCooldownTimer = null; }
  if (vmIdleTimer) { clearTimeout(vmIdleTimer); vmIdleTimer = null; }

  document.getElementById('voice-mode-overlay').classList.add('hidden');
  document.getElementById('voice-mode-btn').classList.remove('text-oc-500');
  document.getElementById('voice-mode-btn').classList.add('text-gray-500');
  setVmState('idle');
}

function setVmState(state) {
  vmState = state;
  const overlay = document.getElementById('voice-mode-overlay');
  overlay.className = overlay.className.replace(/vm-\S+/g, '');

  const status = document.getElementById('vm-status');
  switch (state) {
    case 'listening':
      overlay.classList.add('vm-listening');
      status.textContent = 'Listening...';
      break;
    case 'speaking':
      overlay.classList.add('vm-speaking');
      status.textContent = 'Hearing you...';
      break;
    case 'thinking':
      overlay.classList.add('vm-thinking');
      status.textContent = 'Thinking...';
      break;
    case 'playing':
      overlay.classList.add('vm-playing');
      status.textContent = 'Speaking...';
      break;
    case 'cooldown':
      overlay.classList.add('vm-listening');
      status.textContent = 'Listening...';
      break;
  }
}

function handleVmAudio(e) {
  if (!vmActive || vmState === 'thinking' || vmState === 'playing') return;

  const f = e.inputBuffer.getChannelData(0);
  // Calculate RMS
  let sum = 0;
  for (let i = 0; i < f.length; i++) sum += f[i] * f[i];
  const rms = Math.sqrt(sum / f.length) * 32767; // scale to int16 range

  if (rms >= VM_SILENCE_THRESHOLD) {
    // Speech detected
    if (!vmHeardSpeech) {
      // Start of new utterance
      vmHeardSpeech = true;
      vmSilenceCount = 0;
      setVmState('speaking');
      if (vmWs && vmWs.readyState === 1) {
        vmWs.send(JSON.stringify({ type: 'start' }));
      }
    }
    vmSilenceCount = 0;
    resetVmIdle();
  } else if (vmHeardSpeech) {
    vmSilenceCount++;
    if (vmSilenceCount >= VM_SILENCE_CHUNKS) {
      // End of utterance — send stop, wait for transcript
      vmHeardSpeech = false;
      vmSilenceCount = 0;
      if (vmWs && vmWs.readyState === 1) {
        vmWs.send(JSON.stringify({ type: 'stop' }));
      }
      setVmState('thinking');
    }
  }

  // Forward audio if speaking
  if (vmHeardSpeech && vmWs && vmWs.readyState === 1) {
    const pcm = new Int16Array(f.length);
    for (let i = 0; i < f.length; i++)
      pcm[i] = Math.max(-32768, Math.min(32767, Math.round(f[i] * 32767)));
    vmWs.send(pcm.buffer);
  }
}

async function handleVmTranscript(e) {
  try {
    const msg = JSON.parse(e.data);
    if (msg.type !== 'transcript' || !msg.text) return;
    const text = msg.text.trim();
    if (!text) { setVmState('listening'); return; }

    // Echo filter
    const lower = text.toLowerCase().replace(/[^a-z ]/g, '').trim();
    if (VM_ECHO_WORDS.has(lower)) {
      setVmState('listening');
      return;
    }

    // Show transcript
    document.getElementById('vm-transcript').textContent = '"' + text + '"';
    setVmState('thinking');

    // Send to agent (use existing send function, but intercept response for TTS)
    voiceInitiated = true;

    // Add user message to chat
    const container = document.getElementById('messages');
    const scroll = document.getElementById('messages-scroll');
    const userEl = document.createElement('div');
    userEl.className = 'flex gap-4 justify-end';
    userEl.innerHTML = `<div class="max-w-[75%]"><div class="bg-oc-900/50 border border-oc-800/40 rounded-2xl rounded-br-md px-5 py-3 text-sm text-gray-200 leading-relaxed">${esc(text)}</div></div>`;
    container.appendChild(userEl);
    scroll.scrollTop = scroll.scrollHeight;

    // Call agent
    const startResp = await fetch('/api/message/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: text, agent: 'main', token })
    });
    const startData = await startResp.json();
    const turnId = startData.turn_id;

    if (turnId) {
      const evtSource = new EventSource(`/api/message/${turnId}/stream?token=${token}`);
      evtSource.onmessage = async (event) => {
        const ev = JSON.parse(event.data);
        if (ev.event === 'complete') {
          evtSource.close();

          // Add response to chat
          const aiEl = document.createElement('div');
          aiEl.className = 'flex gap-4';
          aiEl.innerHTML = `<div class="w-8 h-8 rounded-lg bg-oc-600 flex items-center justify-center text-sm font-bold flex-shrink-0">&#9813;</div>
            <div class="flex-1"><div class="text-gray-300 leading-relaxed text-sm chat-md">${md(ev.response)}</div></div>`;
          container.appendChild(aiEl);
          scroll.scrollTop = scroll.scrollHeight;

          // Play TTS
          setVmState('playing');
          document.getElementById('vm-transcript').textContent = '';
          try {
            const ttsResp = await fetch('/api/tts', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ text: ev.response.substring(0, 500) })
            });
            const ttsData = await ttsResp.json();
            if (ttsData.audio_url) {
              const audio = new Audio(ttsData.audio_url);
              // Estimate duration from response length (~150ms per word)
              const words = ev.response.split(/\s+/).length;
              vmLastTtsDuration = Math.max(2, Math.round(words * 0.15));

              audio.onplay = () => {
                fetch('/api/music/duck', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({token: authToken, state: 'on', duration_secs: vmLastTtsDuration + 5}) }).catch(()=>{});
              };
              audio.onended = () => {
                fetch('/api/music/duck', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({token: authToken, state: 'off'}) }).catch(()=>{});
                // Enter cooldown, then resume listening
                setVmState('cooldown');
                vmCooldownTimer = setTimeout(() => {
                  if (vmActive) setVmState('listening');
                }, (vmLastTtsDuration + 1) * 1000);
              };
              audio.play().catch(() => {
                if (vmActive) setVmState('listening');
              });
            } else {
              if (vmActive) setVmState('listening');
            }
          } catch {
            if (vmActive) setVmState('listening');
          }
        } else if (ev.event === 'error') {
          evtSource.close();
          if (vmActive) setVmState('listening');
        }
      };
      evtSource.onerror = () => { evtSource.close(); if (vmActive) setVmState('listening'); };
    }
  } catch {
    if (vmActive) setVmState('listening');
  }
}

function resetVmIdle() {
  if (vmIdleTimer) clearTimeout(vmIdleTimer);
  vmIdleTimer = setTimeout(() => {
    if (vmActive && vmState === 'listening') {
      stopVoiceMode();
    }
  }, VM_MAX_SILENCE_S * 1000);
}

// TTS playback for voice-initiated responses
async function playTts(text) {
  try {
    const resp = await fetch('/api/tts', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: text.substring(0, 500) })
    });
    const data = await resp.json();
    if (data.audio_url) {
      const audio = new Audio(data.audio_url);
      const estDur = data.estimated_duration_secs || 10;
      audio.onplay = () => {
        fetch('/api/music/duck', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({token: authToken, state: 'on', duration_secs: estDur + 5}) }).catch(()=>{});
      };
      audio.onended = () => {
        fetch('/api/music/duck', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({token: authToken, state: 'off'}) }).catch(()=>{});
      };
      audio.play().catch(e => console.log('audio play blocked:', e));
    }
  } catch(e) { console.log('TTS error:', e); }
}
</script>
<script>
// Bug Report UI — injected on all authenticated pages (schema v10)
(function() {
  const BUG_TOKEN = sessionStorage.getItem('syntaur_token') || '';
  if (!BUG_TOKEN) return;
  const topBar = document.querySelector('.border-b.border-gray-800 .flex.items-center.justify-between');
  if (!topBar) return;
  const rightNav = topBar.lastElementChild;
  if (!rightNav) return;
  const bugBtn = document.createElement('button');
  bugBtn.title = 'Report a Bug';
  bugBtn.className = 'text-gray-500 hover:text-gray-300 transition-colors p-1 rounded';
  bugBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2l1.88 1.88M14.12 3.88L16 2M9 7.13v-1a3.003 3.003 0 116 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 014-4h4a4 4 0 014 4v3c0 3.3-2.7 6-6 6z"/><path d="M12 20v-9M6.53 9C4.6 8.8 3 7.1 3 5M6 13H2M6 17l-4 1M17.47 9c1.93-.2 3.53-1.9 3.53-4M18 13h4M18 17l4 1"/></svg>';
  bugBtn.onclick = function() { openBugModal(); };
  if (rightNav.tagName === 'DIV') { rightNav.prepend(bugBtn); }
  else { const w = document.createElement('div'); w.className='flex items-center gap-3'; topBar.replaceChild(w, rightNav); w.appendChild(bugBtn); w.appendChild(rightNav); }
  const overlay = document.createElement('div');
  overlay.id = 'bug-report-overlay';
  overlay.className = 'fixed inset-0 z-50 bg-black/60 backdrop-blur-sm hidden flex items-center justify-center';
  overlay.innerHTML = '<div class="bg-gray-800 border border-gray-700 rounded-2xl w-full max-w-lg mx-4 shadow-2xl"><div class="px-6 py-4 border-b border-gray-700 flex items-center justify-between"><h3 class="text-lg font-semibold text-white">Report a Bug</h3><button onclick="document.getElementById(\'bug-report-overlay\').classList.add(\'hidden\')" class="text-gray-400 hover:text-gray-200 text-xl leading-none">&times;</button></div><div class="px-6 py-5 space-y-4"><div><label class="block text-sm font-medium text-gray-300 mb-1.5">What went wrong?</label><textarea id="bug-desc" rows="4" class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-500 focus:border-sky-500 focus:ring-1 focus:ring-sky-500 outline-none resize-none text-sm" placeholder="Describe the bug..."></textarea></div><details class="text-sm"><summary class="text-gray-400 cursor-pointer hover:text-gray-300 select-none">System Info (auto-collected)</summary><pre id="bug-sysinfo" class="mt-2 bg-gray-900 border border-gray-700 rounded-lg p-3 text-xs text-gray-400 overflow-x-auto max-h-40 overflow-y-auto"></pre></details><div id="bug-feedback" class="hidden text-sm"></div></div><div class="px-6 py-4 border-t border-gray-700 flex justify-end gap-3"><button onclick="document.getElementById(\'bug-report-overlay\').classList.add(\'hidden\')" class="text-sm text-gray-400 hover:text-gray-300 px-4 py-2">Cancel</button><button id="bug-submit-btn" onclick="submitBugReport()" class="text-sm bg-sky-600 hover:bg-sky-700 text-white font-medium px-5 py-2 rounded-lg transition-colors">Submit</button></div></div>';
  document.body.appendChild(overlay);
  overlay.addEventListener('click', function(e) { if (e.target === overlay) overlay.classList.add('hidden'); });
  window.openBugModal = async function() {
    const ov = document.getElementById('bug-report-overlay');
    const desc = document.getElementById('bug-desc');
    const si = document.getElementById('bug-sysinfo');
    const fb = document.getElementById('bug-feedback');
    desc.value = ''; fb.classList.add('hidden');
    document.getElementById('bug-submit-btn').disabled = false;
    document.getElementById('bug-submit-btn').textContent = 'Submit';
    const info = { userAgent: navigator.userAgent, screen: screen.width+'x'+screen.height, window: innerWidth+'x'+innerHeight, page: location.href, time: new Date().toISOString() };
    try { const r = await fetch('/health'); if (r.ok) info.gateway = await r.json(); } catch(e) { info.gateway = 'unavailable'; }
    si.textContent = JSON.stringify(info, null, 2);
    ov.classList.remove('hidden'); desc.focus();
  };
  window.submitBugReport = async function() {
    const desc = document.getElementById('bug-desc').value.trim();
    const fb = document.getElementById('bug-feedback');
    const btn = document.getElementById('bug-submit-btn');
    if (!desc) { fb.className='text-sm text-red-400'; fb.textContent='Please describe the bug.'; fb.classList.remove('hidden'); return; }
    btn.disabled = true; btn.textContent = 'Submitting...'; fb.classList.add('hidden');
    const si = { userAgent: navigator.userAgent, screen: screen.width+'x'+screen.height, window: innerWidth+'x'+innerHeight, page: location.href, time: new Date().toISOString() };
    try { const r = await fetch('/health'); if (r.ok) si.gateway = await r.json(); } catch(e) {}
    try {
      const res = await fetch('/api/bug-reports', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({ token: BUG_TOKEN, description: desc, system_info: si, page_url: location.href }) });
      const data = await res.json();
      if (data.id) { fb.className='text-sm text-green-400'; fb.textContent='Bug report #'+data.id+' submitted. Thank you!'; fb.classList.remove('hidden'); btn.textContent='Submitted'; setTimeout(function(){ document.getElementById('bug-report-overlay').classList.add('hidden'); }, 2000); }
      else { fb.className='text-sm text-red-400'; fb.textContent=data.error||'Submission failed.'; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
    } catch(e) { fb.className='text-sm text-red-400'; fb.textContent='Network error: '+e.message; fb.classList.remove('hidden'); btn.disabled=false; btn.textContent='Submit'; }
  };
})();
</script>"##;
