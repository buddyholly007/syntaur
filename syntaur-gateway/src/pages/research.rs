//! /research — deep-research workflow UI.
//!
//! Thin shell over the existing `/api/research/*` endpoints:
//!   1. POST /api/research/clarify  → optional clarifying questions
//!   2. POST /api/research/start     → kicks off plan → subtasks → report
//!   3. GET  /api/research/{id}/stream → SSE of phase events
//!   4. GET  /api/research/{id}      → final report + citations

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Research",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (top_bar_standard("Research"))
        (page_body())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        div class="max-w-5xl mx-auto px-4 py-6 space-y-6" {
            div {
                h1 class="text-2xl font-bold" { "Research" }
                p class="text-gray-400 mt-1 text-sm" {
                    "Plan → investigate → synthesize. Subtasks query the local knowledge "
                    "index and the web, then a final report is written with citations."
                }
            }

            // ── Query card ────────────────────────────────────────────
            div class="card p-5" id="query-card" {
                label class="block text-sm font-medium text-gray-300 mb-2" for="research-q" {
                    "What do you want researched?"
                }
                textarea id="research-q" rows="3"
                    class="w-full bg-gray-900 border border-gray-700 rounded-lg px-4 py-2.5 text-white placeholder-gray-500 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none resize-y text-sm"
                    placeholder="e.g. How did our crypto bot perform in Q1 vs the leveraged bot, and what drove the difference?" {}
                div class="flex items-center justify-between mt-3 gap-3" {
                    div class="flex items-center gap-3 text-xs text-gray-500" {
                        label { "Agent: " select id="agent-select" class="bg-gray-900 border border-gray-700 rounded px-2 py-1 text-gray-300" {
                            option value="main" { "main (Felix)" }
                        } }
                        label { "Time budget: " select id="time-budget" class="bg-gray-900 border border-gray-700 rounded px-2 py-1 text-gray-300" {
                            option value="60" { "1 min" }
                            option value="180" selected { "3 min" }
                            option value="300" { "5 min" }
                            option value="600" { "10 min" }
                        } }
                    }
                    button id="start-btn" onclick="startFlow()"
                        class="bg-oc-600 hover:bg-oc-700 text-white font-medium px-5 py-2 rounded-lg transition-colors" {
                        "Research"
                    }
                }
            }

            // ── Clarify card (hidden until needed) ────────────────────
            div class="card p-5 hidden" id="clarify-card" {
                h2 class="text-sm font-medium text-gray-300 mb-2" { "A few quick questions" }
                p class="text-xs text-gray-500 mb-3" {
                    "These help the planner scope the research. Answer briefly or skip."
                }
                div id="clarify-questions" class="space-y-3" {}
                div class="flex justify-end gap-2 mt-4" {
                    button onclick="skipClarify()" class="text-sm text-gray-400 hover:text-gray-300 px-3 py-2" {
                        "Skip"
                    }
                    button onclick="submitClarify()" class="bg-oc-600 hover:bg-oc-700 text-white text-sm font-medium px-4 py-2 rounded-lg" {
                        "Continue"
                    }
                }
            }

            // ── Timeline + Report (hidden until running) ──────────────
            div class="hidden" id="run-card" {
                div class="card p-5" {
                    div class="flex items-center justify-between mb-3" {
                        h2 class="text-sm font-medium text-gray-300" { "Progress" }
                        span id="run-status" class="badge badge-blue" { "running…" }
                    }
                    div id="timeline" class="space-y-2" {}
                }

                div class="card p-5 mt-4 hidden" id="report-card" {
                    h2 class="text-sm font-medium text-gray-300 mb-3" { "Report" }
                    div id="report-body" class="prose prose-invert max-w-none text-sm" {}
                    div class="mt-4 pt-4 border-t border-gray-800" {
                        h3 class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-2" { "Citations" }
                        div id="citations-list" class="space-y-1 text-xs" {}
                    }
                }
            }

            // ── Recent sessions ───────────────────────────────────────
            div class="card p-5" {
                div class="flex items-center justify-between mb-3" {
                    h2 class="text-sm font-medium text-gray-300" { "Recent research" }
                    button onclick="loadRecent()" class="text-xs text-gray-500 hover:text-gray-300" { "Refresh" }
                }
                div id="recent-list" class="space-y-2" {}
            }
        }
    }
}

const EXTRA_STYLE: &str = r#"
.prose h1, .prose h2, .prose h3 { color: #e5e7eb; font-weight: 600; margin-top: 1.25em; margin-bottom: 0.5em; }
.prose h1 { font-size: 1.25rem; }
.prose h2 { font-size: 1.125rem; }
.prose h3 { font-size: 1rem; }
.prose p { margin: 0.6em 0; color: #d1d5db; }
.prose ul, .prose ol { margin: 0.6em 0 0.6em 1.5em; color: #d1d5db; }
.prose li { margin: 0.2em 0; }
.prose code { background: #111827; padding: 0.1em 0.3em; border-radius: 3px; font-size: 0.85em; }
.prose pre { background: #030712; border: 1px solid #1f2937; padding: 0.75em; border-radius: 8px; overflow-x: auto; }
.prose a { color: #38bdf8; }
.tl-icon { width: 1.25rem; height: 1.25rem; border-radius: 9999px; display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0; font-size: 0.7rem; }
"#;

const PAGE_JS: &str = r#"
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }

const q = (sel) => document.querySelector(sel);
const esc = (s) => String(s || '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

let pendingQuery = '';
let pendingAgent = 'main';
let pendingTimeBudget = 180;
let clarifyQuestions = [];
let activeStream = null;

async function apiGet(path) {
  const url = path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token);
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

// ── Agent list ────────────────────────────────────────────────────────
(async function loadAgents() {
  try {
    const data = await apiGet('/health');
    if (!data || !data.agents) return;
    const sel = q('#agent-select');
    sel.innerHTML = data.agents.map(a => `<option value="${esc(a.id)}">${esc(a.name || a.id)}</option>`).join('');
  } catch(e) {}
})();

// ── Flow ──────────────────────────────────────────────────────────────
async function startFlow() {
  const text = q('#research-q').value.trim();
  if (!text) { q('#research-q').focus(); return; }
  pendingQuery = text;
  pendingAgent = q('#agent-select').value || 'main';
  pendingTimeBudget = parseInt(q('#time-budget').value || '180', 10);
  const btn = q('#start-btn');
  btn.disabled = true;
  btn.textContent = 'Thinking…';
  q('#clarify-card').classList.add('hidden');
  q('#run-card').classList.add('hidden');
  q('#report-card').classList.add('hidden');
  q('#timeline').innerHTML = '';

  try {
    const clar = await apiPost('/api/research/clarify', {
      agent: pendingAgent,
      query: pendingQuery,
    });
    if (clar && clar.status === 'needs_clarification' && clar.questions && clar.questions.length > 0) {
      clarifyQuestions = clar.questions;
      renderClarify(clar.questions);
      btn.disabled = false;
      btn.textContent = 'Research';
      return;
    }
  } catch(e) {
    console.error('clarify:', e);
  }
  // Ready — start immediately
  btn.disabled = false;
  btn.textContent = 'Research';
  launchResearch('');
}

function renderClarify(questions) {
  const box = q('#clarify-questions');
  box.innerHTML = questions.map((qq, i) => `
    <div>
      <label class="block text-xs text-gray-400 mb-1">${i + 1}. ${esc(qq)}</label>
      <input type="text" data-idx="${i}"
        class="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
        placeholder="Your answer…"/>
    </div>
  `).join('');
  q('#clarify-card').classList.remove('hidden');
  box.querySelector('input')?.focus();
}

function skipClarify() {
  q('#clarify-card').classList.add('hidden');
  launchResearch('');
}

function submitClarify() {
  const inputs = q('#clarify-questions').querySelectorAll('input[data-idx]');
  const answers = [];
  inputs.forEach(inp => {
    const i = parseInt(inp.dataset.idx, 10);
    const v = inp.value.trim();
    if (v) answers.push(`${clarifyQuestions[i]}\n  ${v}`);
  });
  q('#clarify-card').classList.add('hidden');
  launchResearch(answers.join('\n\n'));
}

async function launchResearch(clarification_answers) {
  const runCard = q('#run-card');
  runCard.classList.remove('hidden');
  q('#run-status').textContent = 'starting…';
  q('#timeline').innerHTML = '';
  addTimelineRow('started', 'Starting research…');

  const r = await apiPost('/api/research/start', {
    agent: pendingAgent,
    query: pendingQuery,
    time_budget_secs: pendingTimeBudget,
    clarification_answers: clarification_answers || null,
  });
  if (r.error || !r.session_id) {
    addTimelineRow('error', r.error || 'could not start session');
    q('#run-status').textContent = 'error';
    q('#run-status').className = 'badge badge-red';
    return;
  }
  streamSession(r.session_id);
  loadRecent();
}

function streamSession(sessionId) {
  if (activeStream) { try { activeStream.close(); } catch(e) {} }
  const url = `/api/research/${encodeURIComponent(sessionId)}/stream?token=${encodeURIComponent(token)}`;
  const es = new EventSource(url);
  activeStream = es;
  es.onmessage = (ev) => {
    try {
      const data = JSON.parse(ev.data);
      handleEvent(data, sessionId);
    } catch(e) { console.error('parse event:', e, ev.data); }
  };
  es.onerror = () => {
    try { es.close(); } catch(e) {}
    activeStream = null;
  };
}

function handleEvent(ev, sessionId) {
  switch (ev.event) {
    case 'started':
      addTimelineRow('started', `Session ${sessionId.slice(0, 8)} started.`);
      break;
    case 'cache_hit':
      addTimelineRow('cache', `Cache hit (age ${ev.cached_age_secs}s) — loading report…`);
      loadReport(sessionId);
      break;
    case 'plan_generated':
      addTimelineRow('plan', `Plan: ${ev.steps} step${ev.steps === 1 ? '' : 's'}`);
      (ev.plan_titles || []).forEach((t, i) => addTimelineRow('plan-step', `  ${i + 1}. ${t}`, true));
      break;
    case 'subtask_started':
      addTimelineRow('subtask', `Step ${ev.step_index + 1} running: ${ev.task}`);
      break;
    case 'subtask_completed':
      const note = ev.error
        ? `✗ Step ${ev.step_index + 1} failed: ${ev.error}`
        : `✓ Step ${ev.step_index + 1} done (${ev.citations} citation${ev.citations === 1 ? '' : 's'}, ${(ev.duration_ms / 1000).toFixed(1)}s)`;
      addTimelineRow('subtask-done', note);
      break;
    case 'report_started':
      addTimelineRow('report', 'Writing report…');
      break;
    case 'complete':
      addTimelineRow('complete', `Complete (${(ev.duration_ms / 1000).toFixed(1)}s)`);
      q('#run-status').textContent = 'complete';
      q('#run-status').className = 'badge badge-green';
      loadReport(sessionId);
      break;
    case 'error':
      addTimelineRow('error', ev.message || 'error');
      q('#run-status').textContent = 'error';
      q('#run-status').className = 'badge badge-red';
      break;
    default:
      addTimelineRow('info', JSON.stringify(ev));
  }
}

function addTimelineRow(kind, text, muted) {
  const icon = {
    started: ['🔵', 'bg-blue-900/40 text-blue-400'],
    cache: ['💾', 'bg-purple-900/40 text-purple-400'],
    plan: ['📋', 'bg-indigo-900/40 text-indigo-400'],
    'plan-step': ['·', 'bg-gray-800 text-gray-500'],
    subtask: ['▶', 'bg-cyan-900/40 text-cyan-400'],
    'subtask-done': ['✓', 'bg-emerald-900/40 text-emerald-400'],
    report: ['✎', 'bg-sky-900/40 text-sky-400'],
    complete: ['✓', 'bg-green-900/40 text-green-400'],
    error: ['✗', 'bg-red-900/40 text-red-400'],
    info: ['·', 'bg-gray-800 text-gray-400'],
  }[kind] || ['·', 'bg-gray-800 text-gray-400'];
  const row = document.createElement('div');
  row.className = 'flex items-start gap-2' + (muted ? ' ml-5 text-xs text-gray-500' : ' text-sm');
  row.innerHTML = `<span class="tl-icon ${icon[1]}">${icon[0]}</span><span>${esc(text)}</span>`;
  q('#timeline').appendChild(row);
}

async function loadReport(sessionId) {
  const data = await apiGet('/api/research/' + encodeURIComponent(sessionId));
  if (!data || data.error) return;
  const body = data.report_text || data.summary || '(no report text)';
  q('#report-body').innerHTML = renderMarkdown(body);
  const cites = data.evidence?.flatMap(e => e.citations || []) || data.citations || [];
  renderCitations(cites);
  q('#report-card').classList.remove('hidden');
}

function renderCitations(cites) {
  const box = q('#citations-list');
  if (!cites || cites.length === 0) {
    box.innerHTML = '<p class="text-gray-600">No citations recorded.</p>';
    return;
  }
  const seen = new Set();
  const deduped = [];
  for (const c of cites) {
    const key = `${c.source || ''}::${c.external_id || c.url || c.title || ''}`;
    if (seen.has(key)) continue;
    seen.add(key);
    deduped.push(c);
  }
  box.innerHTML = deduped.map((c, i) => `
    <div class="flex items-start gap-2">
      <span class="text-gray-500 flex-shrink-0">[${i + 1}]</span>
      <div class="min-w-0">
        <div class="text-gray-300 truncate">${esc(c.title || c.external_id || c.url || 'source')}</div>
        ${c.source ? `<div class="text-gray-600">${esc(c.source)}${c.external_id ? ' · ' + esc(c.external_id) : ''}</div>` : ''}
        ${c.snippet ? `<div class="text-gray-500 mt-0.5">${esc(c.snippet.slice(0, 200))}${c.snippet.length > 200 ? '…' : ''}</div>` : ''}
      </div>
    </div>
  `).join('');
}

// Tiny, safe-ish markdown renderer (headings, bold/italic, code, lists, links).
// Not full CommonMark — enough for the LLM-produced reports.
function renderMarkdown(src) {
  let s = esc(src);
  s = s.replace(/```(\w+)?\n([\s\S]*?)```/g, (_m, _lang, body) => `<pre><code>${body}</code></pre>`);
  s = s.replace(/`([^`]+)`/g, '<code>$1</code>');
  s = s.replace(/^### (.+)$/gm, '<h3>$1</h3>');
  s = s.replace(/^## (.+)$/gm, '<h2>$1</h2>');
  s = s.replace(/^# (.+)$/gm, '<h1>$1</h1>');
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/\*([^*]+)\*/g, '<em>$1</em>');
  s = s.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  // Lists: consecutive lines starting with "- " become <ul><li>…</li></ul>.
  s = s.replace(/(^|\n)((?:- .+\n?)+)/g, (_m, pre, block) => {
    const items = block.trim().split(/\n/).map(l => `<li>${l.replace(/^- /, '')}</li>`).join('');
    return `${pre}<ul>${items}</ul>`;
  });
  // Paragraphs: split by blank lines.
  s = s.split(/\n{2,}/).map(p => {
    const t = p.trim();
    if (!t) return '';
    if (t.startsWith('<h') || t.startsWith('<ul') || t.startsWith('<pre')) return t;
    return `<p>${t.replace(/\n/g, '<br>')}</p>`;
  }).join('\n');
  return s;
}

async function loadRecent() {
  try {
    const data = await apiGet('/api/research/recent');
    if (!data) return;
    const rows = data.sessions || [];
    const box = q('#recent-list');
    if (rows.length === 0) {
      box.innerHTML = '<p class="text-sm text-gray-500">No prior research yet.</p>';
      return;
    }
    box.innerHTML = rows.map(r => `
      <div class="flex items-center justify-between bg-gray-900 rounded-lg p-3 cursor-pointer hover:bg-gray-800"
           onclick="reopenSession('${esc(r.id)}')">
        <div class="min-w-0 flex-1">
          <div class="text-sm truncate">${esc(r.query)}</div>
          <div class="text-xs text-gray-500 mt-0.5">
            <span class="badge ${r.status === 'complete' ? 'badge-green' : (r.status === 'error' ? 'badge-red' : 'badge-gray')}">${esc(r.status)}</span>
            · ${esc(r.agent)} · ${esc(r.created_at || '')}
          </div>
        </div>
      </div>
    `).join('');
  } catch(e) { console.error('recent:', e); }
}

async function reopenSession(id) {
  q('#run-card').classList.remove('hidden');
  q('#timeline').innerHTML = '';
  addTimelineRow('info', 'Loading prior session…');
  q('#run-status').textContent = 'loading';
  q('#run-status').className = 'badge badge-gray';
  await loadReport(id);
  q('#run-status').textContent = 'complete';
  q('#run-status').className = 'badge badge-green';
}

loadRecent();
"#;
