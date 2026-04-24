//! /setup/tailscale — Tailscale Serve connection wizard.
//!
//! Single-page form. User pastes OAuth client credentials (preferred, auto-
//! rotating) OR a manual auth key (quick, expires in 90 days). Either way
//! the sidecar registers on the tailnet and starts serving HTTPS within a
//! few seconds.
//!
//! Design principle: zero ACL/scope/tag clicks. Syntaur auto-manages the
//! tailnet ACL via the OAuth client's `acl:write` scope. Users see one
//! input, one button, one success URL.

use axum::response::Html;
use maud::{html, PreEscaped, DOCTYPE};

pub async fn render() -> Html<String> {
    let m = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Remote access — Syntaur" }
                link rel="icon" href="/favicon.ico";
                script src="/tailwind.js" {}
                style { (PreEscaped(STYLES)) }
            }
            body class="bg-gray-950 text-gray-100 min-h-screen" {
                div class="max-w-2xl mx-auto px-4 py-12" {
                    div class="mb-10" {
                        h1 class="text-2xl font-semibold mb-2" { "Connect Syntaur to your Tailscale network" }
                        p class="text-gray-400 text-sm leading-relaxed" {
                            "One paste gives your Syntaur a trusted HTTPS URL you can open from any device on your Tailscale network — including your phone on cellular. No port forwarding, no cert warnings, no red padlocks."
                        }
                    }

                    // Status card — live state of the connection.
                    div id="ts-status" class="mb-6 p-4 rounded-lg border border-gray-800 bg-gray-900/50 text-sm" {
                        "Loading current status…"
                    }

                    // Paste OAuth creds (primary path).
                    div class="mb-6 p-5 rounded-lg border border-gray-800 bg-gray-900/30" {
                        div class="flex items-start gap-3 mb-3" {
                            div class="flex-shrink-0 w-8 h-8 rounded-full bg-emerald-500/20 flex items-center justify-center text-emerald-400 font-bold text-xs" { "1" }
                            div class="flex-1" {
                                h2 class="font-medium text-gray-100" { "Generate OAuth credentials" }
                                p class="text-xs text-gray-500 mt-1" {
                                    "At "
                                    a class="text-emerald-400 underline" href="https://login.tailscale.com/admin/settings/oauth" target="_blank" { "login.tailscale.com/admin/settings/oauth" }
                                    " → Generate OAuth client. Check BOTH scopes: "
                                    strong { "auth_keys (Write)" }
                                    " and "
                                    strong { "acl (Write)" }
                                    ". Copy the Client ID and Secret, paste below."
                                }
                            }
                        }
                        form id="ts-oauth-form" class="space-y-3" {
                            input type="text" id="ts-client-id" placeholder="Client ID (e.g. abc123CNTRL)" class="w-full px-3 py-2 rounded bg-gray-800 border border-gray-700 text-sm font-mono" autocomplete="off";
                            input type="password" id="ts-client-secret" placeholder="Client Secret (starts with tskey-client-...)" class="w-full px-3 py-2 rounded bg-gray-800 border border-gray-700 text-sm font-mono" autocomplete="off";
                            div class="flex items-center gap-3" {
                                input type="text" id="ts-hostname" placeholder="Hostname (default: syntaur)" class="flex-1 px-3 py-2 rounded bg-gray-800 border border-gray-700 text-sm font-mono" autocomplete="off" value="syntaur";
                                button type="submit" class="px-4 py-2 rounded bg-emerald-600 hover:bg-emerald-500 text-white text-sm font-medium" {
                                    "Connect"
                                }
                            }
                        }
                        div id="ts-oauth-msg" class="mt-3 text-xs" {}
                    }

                    // Fallback: auth key paste.
                    details class="mb-6" {
                        summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-300" {
                            "Don't want OAuth? Paste a regular auth key instead (expires in 90 days)"
                        }
                        div class="mt-3 p-4 rounded-lg border border-gray-800 bg-gray-900/20 space-y-3" {
                            p class="text-xs text-gray-500" {
                                "Generate one at "
                                a class="text-emerald-400 underline" href="https://login.tailscale.com/admin/settings/keys" target="_blank" { "login.tailscale.com/admin/settings/keys" }
                                " (enable Reusable + Pre-approved for a smooth first boot)."
                            }
                            form id="ts-authkey-form" class="flex gap-2" {
                                input type="password" id="ts-authkey" placeholder="tskey-auth-..." class="flex-1 px-3 py-2 rounded bg-gray-800 border border-gray-700 text-sm font-mono" autocomplete="off";
                                button type="submit" class="px-4 py-2 rounded bg-gray-700 hover:bg-gray-600 text-gray-100 text-sm" { "Connect" }
                            }
                            div id="ts-authkey-msg" class="mt-2 text-xs" {}
                        }
                    }

                    // Disconnect (shown only when connected).
                    div id="ts-disconnect-wrap" class="hidden text-center" {
                        button id="ts-disconnect-btn" class="text-xs text-gray-600 hover:text-red-400 underline" {
                            "Disconnect from Tailscale"
                        }
                    }
                }

                script { (PreEscaped(PAGE_JS)) }
            }
        }
    };
    Html(m.into_string())
}

const STYLES: &str = r##"
body { font-family: -apple-system, BlinkMacSystemFont, 'Inter', sans-serif; }
input:focus, button:focus { outline: 2px solid #10b981; outline-offset: 1px; }
"##;

const PAGE_JS: &str = r##"
const token = sessionStorage.getItem('syntaur_token') || '';
// Client-side token-gate removed 2026-04-24 (module-reset bug fix).
const AUTH_H = () => ({ 'Authorization': 'Bearer ' + token });
const JSON_AUTH_H = () => ({ 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token });

async function loadStatus() {
  try {
    const r = await fetch('/api/setup/tailscale/status', { headers: AUTH_H() });
    const data = await r.json();
    renderStatus(data);
  } catch(e) {
    document.getElementById('ts-status').innerHTML = '<span class="text-red-400">Status check failed: ' + e.message + '</span>';
  }
}

function renderStatus(s) {
  const el = document.getElementById('ts-status');
  if (!s.enabled) {
    el.innerHTML = '<span class="text-gray-400">Not connected. Paste credentials below to get started.</span>';
    document.getElementById('ts-disconnect-wrap').classList.add('hidden');
    return;
  }
  const mode = s.auth_mode === 'oauth' ? 'OAuth (auto-rotating)' : 'auth key';
  const status = s.connected ? '<span class="text-emerald-400">● Connected</span>' : '<span class="text-yellow-400">● Starting up…</span>';
  const url = s.tailnet_url
    ? '<a class="text-emerald-400 underline break-all" href="' + s.tailnet_url + '" target="_blank">' + s.tailnet_url + '</a>'
    : '<span class="text-gray-500">(URL appears once the sidecar finishes registering)</span>';
  let html = status + ' via ' + mode + '<br><span class="text-xs text-gray-500">Access at: </span>' + url;

  // Phase 4.1 polish: if the sidecar reported a one-click-fix error, show
  // the action URL as a prominent button the user can click to finish
  // setup. Most commonly this is the Tailscale-Serve-not-enabled case.
  if (s.last_error) {
    html += '<div class="mt-3 p-3 rounded border border-amber-600/40 bg-amber-900/20">'
         +   '<p class="text-sm text-amber-200 mb-2">' + s.last_error + '</p>';
    if (s.action_url) {
      html +=   '<a href="' + s.action_url + '" target="_blank" '
             +    'class="inline-block px-3 py-1.5 rounded bg-amber-600 hover:bg-amber-500 text-white text-sm font-medium">'
             +    'Finish setup in Tailscale →'
             + '</a>'
             + '<p class="text-xs text-amber-300/70 mt-2">Opens in a new tab. Come back here when done — we\'ll detect the change within seconds.</p>';
    }
    html += '</div>';
  }

  el.innerHTML = html;
  document.getElementById('ts-disconnect-wrap').classList.remove('hidden');
}

document.getElementById('ts-oauth-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const msg = document.getElementById('ts-oauth-msg');
  msg.innerHTML = '<span class="text-gray-400">Connecting…</span>';
  const client_id = document.getElementById('ts-client-id').value.trim();
  const client_secret = document.getElementById('ts-client-secret').value.trim();
  const hostname = document.getElementById('ts-hostname').value.trim() || 'syntaur';
  try {
    const r = await fetch('/api/setup/tailscale/connect_oauth', {
      method: 'POST', headers: JSON_AUTH_H(),
      body: JSON.stringify({ client_id, client_secret, hostname })
    });
    const data = await r.json();
    if (data.ok) {
      msg.innerHTML = '<span class="text-emerald-400">✓ ' + (data.note || 'Connected') + '</span>';
      document.getElementById('ts-client-secret').value = '';
      setTimeout(loadStatus, 2000);
    } else {
      msg.innerHTML = '<span class="text-red-400">' + (data.error || 'Connection failed') + '</span>';
    }
  } catch(e) { msg.innerHTML = '<span class="text-red-400">' + e.message + '</span>'; }
});

document.getElementById('ts-authkey-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const msg = document.getElementById('ts-authkey-msg');
  msg.innerHTML = '<span class="text-gray-400">Connecting…</span>';
  const auth_key = document.getElementById('ts-authkey').value.trim();
  try {
    const r = await fetch('/api/setup/tailscale/connect', {
      method: 'POST', headers: JSON_AUTH_H(),
      body: JSON.stringify({ auth_key, hostname: 'syntaur' })
    });
    const data = await r.json();
    if (data.ok) {
      msg.innerHTML = '<span class="text-emerald-400">✓ ' + (data.note || 'Connected') + '</span>';
      document.getElementById('ts-authkey').value = '';
      setTimeout(loadStatus, 2000);
    } else {
      msg.innerHTML = '<span class="text-red-400">' + (data.error || 'Connection failed') + '</span>';
    }
  } catch(e) { msg.innerHTML = '<span class="text-red-400">' + e.message + '</span>'; }
});

document.getElementById('ts-disconnect-btn').addEventListener('click', async () => {
  if (!confirm('Disconnect Syntaur from your Tailscale network? Syntaur itself keeps running — only the tailnet HTTPS URL goes away.')) return;
  try {
    await fetch('/api/tailscale/disconnect', { method: 'POST', headers: JSON_AUTH_H() });
    loadStatus();
  } catch(e) { alert(e.message); }
});

loadStatus();
setInterval(loadStatus, 10000);
"##;
