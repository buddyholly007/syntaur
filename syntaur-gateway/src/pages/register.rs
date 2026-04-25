//! /register — invite-based account registration page.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Create Account",
        authed: false,
        extra_style: None,
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        div class="min-h-screen flex items-center justify-center bg-gray-950 px-4" {
            div class="w-full max-w-sm" {
                div class="text-center mb-8" {
                    img src="/app-icon.jpg" class="w-16 h-16 rounded-2xl mx-auto" alt="Syntaur";
                    h1 class="text-2xl font-bold mt-2" { "Create Account" }
                    p class="text-gray-400 text-sm mt-1" { "Enter your invite code to get started" }
                }
                div class="bg-gray-800 rounded-xl border border-gray-700 p-6" {
                    div class="space-y-4" {
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Invite Code" }
                            input type="text" id="reg-code"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
                                placeholder="Paste your invite code" autocomplete="off";
                        }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Name" }
                            input type="text" id="reg-name"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
                                placeholder="Your name" autocomplete="name";
                        }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Password" }
                            input type="password" id="reg-pass"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
                                placeholder="At least 8 characters" autocomplete="new-password";
                        }
                        div {
                            label class="block text-sm font-medium text-gray-300 mb-1.5" { "Confirm Password" }
                            input type="password" id="reg-pass2"
                                class="w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none"
                                placeholder="Confirm password" autocomplete="new-password"
                                onkeydown="if(event.key==='Enter')doRegister()";
                        }
                        button onclick="doRegister()"
                            class="w-full bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-6 rounded-lg transition-colors"
                            id="reg-btn" { "Create Account" }
                        p id="reg-error" class="text-sm text-red-400 hidden" {}
                        p class="text-center text-xs text-gray-500" {
                            "Already have an account? "
                            a href="/" class="text-oc-400 hover:text-oc-300" { "Sign in" }
                        }
                    }
                }
            }
        }
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

const PAGE_JS: &str = r#"
// Pre-fill invite code from URL
const params = new URLSearchParams(window.location.search);
if (params.get('code')) {
  document.getElementById('reg-code').value = params.get('code');
  document.getElementById('reg-name').focus();
}

async function doRegister() {
  const code = document.getElementById('reg-code').value.trim();
  const name = document.getElementById('reg-name').value.trim();
  const pass = document.getElementById('reg-pass').value;
  const pass2 = document.getElementById('reg-pass2').value;
  const errEl = document.getElementById('reg-error');
  errEl.classList.add('hidden');

  if (!code) { showErr('Invite code is required'); return; }
  if (!name) { showErr('Name is required'); return; }
  if (pass.length < 8) { showErr('Password must be at least 8 characters'); return; }
  if (pass !== pass2) { showErr('Passwords do not match'); return; }

  document.getElementById('reg-btn').textContent = 'Creating...';
  try {
    const resp = await fetch('/api/auth/register', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code, name, password: pass })
    });
    const data = await resp.json();
    if (data.ok && data.token) {
      sessionStorage.setItem('syntaur_token', data.token);
      window.location.href = data.needs_onboarding ? '/onboarding' : '/';
    } else {
      showErr(data.error || 'Registration failed');
    }
  } catch(e) {
    showErr('Connection error: ' + e.message);
  }
  document.getElementById('reg-btn').textContent = 'Create Account';
}

function showErr(msg) {
  const el = document.getElementById('reg-error');
  el.textContent = msg;
  el.classList.remove('hidden');
}
"#;
