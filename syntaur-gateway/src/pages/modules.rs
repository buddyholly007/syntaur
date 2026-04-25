//! /modules — module management page. Lists core + extension modules
//! and lets the user toggle them.

use axum::response::Html;
use maud::{html, Markup, PreEscaped};

use super::shared::{shell, top_bar_standard, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Modules",
        authed: true,
        extra_style: None,
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        (page_body())
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

fn page_body() -> Markup {
    html! {
        div class="max-w-5xl mx-auto px-4 py-6" {
            div class="flex items-center justify-between mb-6" {
                div {
                    h1 class="text-2xl font-bold" { "Modules" }
                    p class="text-gray-400 mt-1" id="module-summary" { "Loading..." }
                }
            }

            div id="restart-banner"
                class="hidden mb-4 p-4 rounded-xl bg-yellow-900/20 border border-yellow-800 flex items-center justify-between" {
                div class="flex items-center gap-2" {
                    span class="text-yellow-400" { "⚠" }
                    p class="text-sm text-yellow-300" {
                        "Module changes require a gateway restart to take effect."
                    }
                }
                span class="badge badge-gray text-xs" {
                    "Restart via: systemctl --user restart syntaur"
                }
            }

            div class="mb-8" {
                h2 class="text-sm font-medium text-gray-500 uppercase tracking-wider mb-3" {
                    "Core Modules"
                }
                p class="text-xs text-gray-600 mb-3" {
                    "Built into the gateway. Always compiled, can be disabled at runtime."
                }
                div class="space-y-2" id="core-list" {}
            }

            div {
                h2 class="text-sm font-medium text-gray-500 uppercase tracking-wider mb-3" {
                    "Extension Modules"
                }
                p class="text-xs text-gray-600 mb-3" {
                    "Separate binaries communicating via MCP protocol. Auto-discovered from ~/.syntaur/modules/"
                }
                div class="space-y-2" id="ext-list" {}
            }
        }
    }
}

const PAGE_JS: &str = r#"
const token = sessionStorage.getItem('syntaur_token') || '';
// Client-side token-gate removed 2026-04-24 — it bounced cookie-
// authenticated users back to `/` whenever storage was empty,
// causing the module-reset bug. 401 handler below covers the
// actual-bad-session case.
let pendingChanges = false;

async function loadModules() {
  try {
    const resp = await fetch('/api/setup/modules', {
      headers: { 'Authorization': 'Bearer ' + token }
    });
    // 2026-04-25: stop bouncing on widget 401. Cookie-authed users
    // hit this when sessionStorage is empty + cookie carries auth;
    // bouncing made the module-reset bug. Just return — UI shows
    // empty state. syntaurLogout() (top bar) is the path back to /.
    if (resp.status === 401) { return; }
    const data = await resp.json();
    const allMods = [...data.core_modules, ...data.extension_modules];
    const enabled = allMods.filter(m => m.enabled);
    const totalTools = allMods.reduce((s, m) => s + m.tool_count, 0);
    document.getElementById('module-summary').textContent =
      `${enabled.length} of ${allMods.length} modules enabled — ${totalTools} total tools`;
    renderList('core-list', data.core_modules);
    renderList('ext-list', data.extension_modules);
  } catch(e) {
    document.getElementById('module-summary').textContent = 'Error loading modules';
  }
}

function renderList(containerId, modules) {
  const container = document.getElementById(containerId);
  if (modules.length === 0) {
    container.innerHTML = '<p class="text-sm text-gray-500 py-4">No modules found.</p>';
    return;
  }
  container.innerHTML = modules.map(m => `
    <div class="card p-4">
      <div class="flex items-start justify-between gap-4">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 flex-wrap">
            <h3 class="font-medium">${m.name}</h3>
            <span class="badge ${m.tier === 'core' ? 'badge-blue' : 'badge-gray'}">${m.tier}</span>
            <span class="badge badge-gray">${m.tool_count} tools</span>
          </div>
          <p class="text-sm text-gray-400 mt-1">${m.description}</p>
        </div>
        <button class="toggle ${m.enabled ? 'bg-oc-600' : 'bg-gray-600'} flex-shrink-0 mt-1"
                onclick="toggleModule('${m.id}', ${!m.enabled}, this)"
                data-id="${m.id}">
          <span class="toggle-dot ${m.enabled ? 'translate-x-6' : 'translate-x-1'}"></span>
        </button>
      </div>
    </div>
  `).join('');
}

async function toggleModule(id, enable, btn) {
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${enable ? 'bg-oc-600' : 'bg-gray-600'} flex-shrink-0 mt-1`;
  dot.className = `toggle-dot ${enable ? 'translate-x-6' : 'translate-x-1'}`;
  try {
    const resp = await fetch('/api/modules/toggle', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token },
      body: JSON.stringify({ token, module_id: id, enabled: enable })
    });
    const data = await resp.json();
    if (data.restart_required) {
      document.getElementById('restart-banner').classList.remove('hidden');
      pendingChanges = true;
    }
  } catch(e) {
    btn.className = `toggle ${!enable ? 'bg-oc-600' : 'bg-gray-600'} flex-shrink-0 mt-1`;
    dot.className = `toggle-dot ${!enable ? 'translate-x-6' : 'translate-x-1'}`;
  }
}

loadModules();
"#;
