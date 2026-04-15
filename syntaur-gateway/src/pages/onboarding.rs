//! /onboarding — new user setup wizard (create agent + personalize AI).

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Welcome to Syntaur",
        authed: true,
        extra_style: None,
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
                            button onclick="skipToFinish()"
                                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-medium py-2.5 rounded-lg transition-colors" { "Skip" }
                        }
                    }
                }

                // Step 3: Done
                div id="step-3" class="hidden text-center" {
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
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, agent_id: agentId, display_name: name, base_agent: 'main', system_prompt: prompt || null })
    });
    const data = await resp.json();
    if (data.ok) {
      createdAgentId = agentId;
      document.getElementById('step-1').classList.add('hidden');
      document.getElementById('step-2').classList.remove('hidden');
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
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, agent_id: aid, doc_type: 'bio', title: 'About Me', content: bio })
    });
  }
  if (prefs) {
    await fetch('/api/me/personality', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, agent_id: aid, doc_type: 'preferences', title: 'Communication Preferences', content: prefs })
    });
  }
  skipToFinish();
}

async function skipToFinish() {
  await fetch('/api/me/onboarding/complete?token=' + encodeURIComponent(token), { method: 'POST' });
  document.getElementById('step-2').classList.add('hidden');
  document.getElementById('step-3').classList.remove('hidden');
}

// Check if already onboarded
(async function() {
  const data = await (await fetch('/api/me/onboarding?token=' + encodeURIComponent(token))).json();
  if (data.complete) { window.location.href = '/'; }
})();
"##;
