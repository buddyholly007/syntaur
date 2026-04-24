//! /onboarding — new user setup wizard (create agent + personalize AI).

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Welcome to Syntaur",
        authed: true,
        extra_style: None,
        body_class: None,
        head_boot: None,
    };
    let body = html! {
        div class="min-h-screen flex items-center justify-center px-4 py-8" {
            div class="w-full max-w-lg" {
                // Step 1: Create your agent
                div id="step-1" {
                    div class="text-center mb-6" {
                        h1 class="text-2xl font-bold" { "Welcome to Syntaur" }
                        p class="text-gray-400 text-sm mt-2" { "Let's set up your personal AI agent." }
                    }
                    div class="bg-gray-800 rounded-xl border border-gray-700 p-6 space-y-4" {
                        h2 class="font-medium text-lg" { "Step 1: Create Your Agent" }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Agent Name" }
                            input type="text" id="ob-agent-name"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 outline-none"
                                placeholder="e.g., My Assistant";
                        }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Personality (optional)" }
                            textarea id="ob-agent-prompt" rows="3"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 outline-none resize-none text-sm"
                                placeholder="Describe how you want your AI to behave — tone, expertise, personality..." {}
                        }
                        button onclick="createAgent()"
                            class="w-full bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 rounded-lg transition-colors"
                            id="ob-create-btn" { "Create Agent" }
                        p id="ob-create-error" class="text-sm text-red-400 hidden" {}
                    }
                }

                // Step 2: Personalize (shown after agent creation)
                div id="step-team" class="hidden" {
                    div class="text-center mb-6" {
                        h1 class="text-2xl font-bold" { "Meet Your Team" }
                        p class="text-gray-400 text-sm mt-2" {
                            "These are your AI specialists. Each handles a different area. "
                            "Rename any of them — or keep the defaults."
                        }
                    }
                    div class="bg-gray-800 rounded-xl border border-gray-700 p-6 space-y-3" id="team-list" {
                        p class="text-gray-500 text-sm animate-pulse" { "Loading your team..." }
                    }
                    div class="mt-4 flex gap-3 justify-end" {
                        button onclick="saveTeamNames()"
                            class="bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-8 rounded-lg transition-colors"
                        { "Continue" }
                    }
                }

                div id="step-2" class="hidden" {
                    div class="text-center mb-6" {
                        h1 class="text-2xl font-bold" { "Personalize Your AI" }
                        p class="text-gray-400 text-sm mt-2" { "Help your agent understand you better. All optional." }
                    }
                    div class="bg-gray-800 rounded-xl border border-gray-700 p-6 space-y-4" {
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "About You" }
                            textarea id="ob-bio" rows="3"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 outline-none resize-none text-sm"
                                placeholder="Your background, role, interests — anything that helps the AI understand you..." {}
                        }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Communication Preferences" }
                            textarea id="ob-prefs" rows="2"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 outline-none resize-none text-sm"
                                placeholder="How should the AI communicate? Casual or formal? Brief or detailed?" {}
                        }
                        div class="flex gap-3" {
                            button onclick="savePersonality()"
                                class="flex-1 bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 rounded-lg transition-colors"
                                id="ob-save-btn" { "Save & Continue" }
                            button onclick="goToDataStep()"
                                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-medium py-2.5 rounded-lg transition-colors" { "Skip" }
                        }
                    }
                }

                // Step 3: Data location
                div id="step-3" class="hidden" {
                    div class="text-center mb-6" {
                        h1 class="text-2xl font-bold" { "Where to Save Your Data" }
                        p class="text-gray-400 text-sm mt-2" { "Choose where documents, uploads, and agent files are stored." }
                    }
                    div class="bg-gray-800 rounded-xl border border-gray-700 p-6 space-y-4" {
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Data Directory" }
                            input type="text" id="ob-data-dir"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 outline-none"
                                placeholder="Leave blank for default (~/.syntaur)";
                            p class="text-xs text-gray-500 mt-1" {
                                "Must be an absolute path. Your agent workspaces and uploaded documents will be stored here."
                            }
                        }
                        div class="flex gap-3" {
                            button onclick="saveDataLocation()"
                                class="flex-1 bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 rounded-lg transition-colors" { "Save & Finish" }
                            button onclick="skipToFinal()"
                                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-medium py-2.5 rounded-lg transition-colors" { "Use Default" }
                        }
                    }
                }

                // Step 4: Done
                div id="step-4" class="hidden text-center" {
                    div class="text-5xl mb-4" { "✓" }
                    h1 class="text-2xl font-bold" { "You're all set!" }
                    p class="text-gray-400 text-sm mt-2 mb-6" { "Your agent is ready. Start chatting on the dashboard." }
                    a href="/" class="inline-block bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-8 rounded-lg transition-colors" {
                        "Go to Dashboard"
                    }
                }
            }
        }
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

const PAGE_JS: &str = r##"
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }
// Phase 1.1: every fetch carries Authorization: Bearer <token>; no ?token= in URLs.
const AUTH_H = () => ({ 'Authorization': 'Bearer ' + token });
const JSON_AUTH_H = () => ({ 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token });
let createdAgentId = null;

async function createAgent() {
  const name = document.getElementById('ob-agent-name').value.trim();
  const prompt = document.getElementById('ob-agent-prompt').value.trim();
  const errEl = document.getElementById('ob-create-error');
  errEl.classList.add('hidden');

  if (!name) { errEl.textContent = 'Please enter a name for your agent'; errEl.classList.remove('hidden'); return; }

  const agentId = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 30) || 'my-agent';
  document.getElementById('ob-create-btn').textContent = 'Creating...';

  try {
    const resp = await fetch('/api/me/agents', {
      method: 'POST',
      headers: JSON_AUTH_H(),
      body: JSON.stringify({ agent_id: agentId, display_name: name, base_agent: 'main', system_prompt: prompt || null })
    });
    const data = await resp.json();
    if (data.ok) {
      createdAgentId = agentId;
      document.getElementById('step-1').classList.add('hidden');
      document.getElementById('step-team').classList.remove('hidden');
      showTeamStep();
    } else {
      errEl.textContent = data.error || 'Failed to create agent';
      errEl.classList.remove('hidden');
    }
  } catch(e) {
    errEl.textContent = 'Connection error: ' + e.message;
    errEl.classList.remove('hidden');
  }
  document.getElementById('ob-create-btn').textContent = 'Create Agent';
}

async function savePersonality() {
  const bio = document.getElementById('ob-bio').value.trim();
  const prefs = document.getElementById('ob-prefs').value.trim();
  document.getElementById('ob-save-btn').textContent = 'Saving...';

  const aid = createdAgentId || 'main';
  if (bio) {
    await fetch('/api/me/personality', {
      method: 'POST',
      headers: JSON_AUTH_H(),
      body: JSON.stringify({ agent_id: aid, doc_type: 'bio', title: 'About Me', content: bio })
    });
  }
  if (prefs) {
    await fetch('/api/me/personality', {
      method: 'POST',
      headers: JSON_AUTH_H(),
      body: JSON.stringify({ agent_id: aid, doc_type: 'preferences', title: 'Communication Preferences', content: prefs })
    });
  }
  goToDataStep();
}

function goToDataStep() {
  document.getElementById('step-2').classList.add('hidden');
  document.getElementById('step-3').classList.remove('hidden');
}

async function saveDataLocation() {
  const dir = document.getElementById('ob-data-dir').value.trim();
  if (dir) {
    await fetch('/api/me/data-location', {
      method: 'PUT',
      headers: JSON_AUTH_H(),
      body: JSON.stringify({ path: dir })
    });
  }
  skipToFinal();
}

async function skipToFinal() {
  await fetch('/api/me/onboarding/complete', { method: 'POST', headers: AUTH_H() });
  document.getElementById('step-3').classList.add('hidden');
  document.getElementById('step-4').classList.remove('hidden');
}


const AGENT_ROLES = {
  main: { emoji: '🤖', role: 'Your main assistant', accent: '#0ea5e9' },
  tax: { emoji: '📊', role: 'Tax specialist', accent: '#3b82f6' },
  research: { emoji: '🔍', role: 'Research analyst', accent: '#d4a574' },
  music: { emoji: '🎵', role: 'Music curator', accent: '#d946ef' },
  scheduler: { emoji: '📅', role: 'Calendar & todos', accent: '#b8860b' },
  coders: { emoji: '💻', role: 'Pair programmer', accent: '#22c55e' },
  journal: { emoji: '📓', role: 'Journal companion', accent: '#8fbc8f' },
};

async function showTeamStep() {
  // Seed defaults first
  await fetch('/api/agents/seed_defaults', { method: 'POST', headers: AUTH_H() });
  const data = await (await fetch('/api/agents/list', { headers: AUTH_H() })).json();
  const agents = data.agents || [];
  const listEl = document.getElementById('team-list');

  if (agents.length === 0) {
    listEl.innerHTML = '<p class="text-red-400">No agents found. Try refreshing.</p>';
    return;
  }

  listEl.innerHTML = agents.map(a => {
    const info = AGENT_ROLES[a.agent_id] || { emoji: '✨', role: 'Specialist', accent: '#6b7280' };
    return `<div class="flex items-center gap-3 p-3 rounded-lg hover:bg-gray-700/50 transition-colors">
      <span class="text-2xl">${info.emoji}</span>
      <div class="flex-1">
        <input type="text" value="${a.display_name}" data-agent="${a.agent_id}"
          class="bg-transparent border-b border-gray-600 text-white font-medium text-sm px-1 py-0.5 w-full max-w-[200px]
                 focus:border-sky-400 outline-none transition-colors" />
        <p class="text-xs text-gray-500 mt-0.5">${info.role}</p>
      </div>
      <span class="text-xs px-2 py-0.5 rounded-full border" style="border-color:${info.accent}40;color:${info.accent}">${a.agent_id}</span>
    </div>`;
  }).join('');
}

async function saveTeamNames() {
  const inputs = document.querySelectorAll('#team-list input[data-agent]');
  for (const inp of inputs) {
    const agentId = inp.dataset.agent;
    const newName = inp.value.trim();
    if (newName && newName.length <= 50) {
      await fetch('/api/agents/rename', {
        method: 'PUT',
        headers: JSON_AUTH_H(),
        body: JSON.stringify({ agent_id: agentId, name: newName })
      });
    }
  }
  document.getElementById('step-team').classList.add('hidden');
  document.getElementById('step-2').classList.remove('hidden');
}

// Check if already onboarded
(async function() {
  const data = await (await fetch('/api/me/onboarding', { headers: AUTH_H() })).json();
  if (data.complete) { window.location.href = '/'; }
})();
"##;
