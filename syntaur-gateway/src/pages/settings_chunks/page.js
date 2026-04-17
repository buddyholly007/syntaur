
const token = sessionStorage.getItem('syntaur_token') || '';
if (!token) { window.location.href = '/'; }
const esc = (s) => String(s || '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function authFetch(url, opts = {}) {
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;
  return fetch(url, opts);
}

function showTab(name) {
  document.querySelectorAll('.tab-content').forEach(el => el.classList.add('hidden'));
  document.querySelectorAll('.tab').forEach(el => el.classList.remove('active'));
  document.getElementById(`tab-${name}`).classList.remove('hidden');
  event.target.classList.add('active');
}

async function loadSettings() {
  try {
    const headers = { 'Authorization': 'Bearer ' + token };
    const [health, status, mods] = await Promise.all([
      fetch('/health', { headers }).then(r => r.json()),
      fetch('/api/setup/status', { headers }).then(r => r.json()),
      fetch('/api/setup/modules', { headers }).then(r => r.json()),
    ]);

    // General
    document.getElementById('set-agent-name').textContent = status.agent_name || 'main';
    document.getElementById('set-version').textContent = `v${health.version || '?'}`;

    // Agent avatars
    const agents = (health.agents || []).map(a => typeof a === 'object' ? a : { id: a, name: a });
    const avatarList = document.getElementById('avatar-list');
    if (avatarList) {
      avatarList.innerHTML = agents.map(a => {
        const id = a.id || a;
        const name = a.name || id;
        return `<div class="flex items-center gap-3 p-2 rounded-lg bg-gray-900">
          <img src="/agent-avatar/${id}?t=${Date.now()}" class="w-10 h-10 rounded-lg" alt="" id="avatar-img-${id}">
          <div class="flex-1">
            <p class="text-sm font-medium text-gray-300">${name}</p>
            <p class="text-xs text-gray-600">${id}</p>
          </div>
          <label class="text-xs text-oc-500 hover:text-oc-400 cursor-pointer px-3 py-1.5 rounded-lg bg-gray-800 hover:bg-gray-700 transition-colors">
            Change
            <input type="file" class="hidden" accept="image/*" onchange="uploadAvatar('${id}', this)">
          </label>
        </div>`;
      }).join('');
    }
    document.getElementById('set-port').textContent = `${location.port || '18789'}`;

    // Uptime
    const u = health.uptime_secs || 0;
    const h = Math.floor(u / 3600);
    const m = Math.floor((u % 3600) / 60);
    document.getElementById('sys-uptime').textContent = h > 0 ? `${h}h ${m}m` : `${m}m`;
    document.getElementById('sys-agents').textContent = (health.agents || []).join(', ');

    // Modules
    const coreEnabled = mods.core_modules.filter(m => m.enabled).length;
    const extEnabled = mods.extension_modules.filter(m => m.enabled).length;
    document.getElementById('sys-core-mods').textContent = `${coreEnabled} / ${mods.core_modules.length}`;
    document.getElementById('sys-ext-mods').textContent = `${extEnabled} / ${mods.extension_modules.length}`;

    // LLM providers
    const providers = health.providers || [];
    document.getElementById('llm-providers-list').innerHTML = providers.map((p, i) => {
      const circuitOk = p.circuit_state === 'Closed';
      const badgeClass = circuitOk ? 'badge-green' : 'badge-red';
      const badgeText = circuitOk ? 'Active' : p.circuit_state;
      const latency = p.avg_latency_ms > 0 ? `${Math.round(p.avg_latency_ms)}ms avg` : '';
      return `
      <div class="flex items-center justify-between p-3 rounded-lg bg-gray-900">
        <div>
          <p class="text-sm font-medium">${p.name}</p>
          <p class="text-xs text-gray-500">${i === 0 ? 'Primary' : 'Fallback ' + i} &middot; ${p.model_id} ${latency ? '&middot; ' + latency : ''}</p>
        </div>
        <div class="flex items-center gap-2">
          <span class="text-xs text-gray-600">${p.total_requests} req</span>
          <span class="badge ${badgeClass}">${badgeText}</span>
        </div>
      </div>`;
    }).join('') || '<p class="text-gray-500 text-sm">No providers configured. Use Quick Setup below to add one.</p>';

    // Token
    if (token) {
      document.getElementById('api-token-display').value = token;
    }

  } catch(e) {
    console.error('Failed to load settings:', e);
  }
}

async function testConnection() {
  const url = document.getElementById('test-url').value;
  const key = document.getElementById('test-key').value;
  const result = document.getElementById('test-result');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Testing...';

  try {
    const resp = await fetch('/api/setup/test-llm', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ base_url: url, api_key: key || null })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm text-green-400';
      result.textContent = `Connected! ${data.models.length} models, ${data.latency_ms}ms`;
    } else {
      result.className = 'text-sm text-red-400';
      result.textContent = data.error || 'Failed';
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Network error';
  }
}

function toggleTokenVisibility() {
  const input = document.getElementById('api-token-display');
  input.type = input.type === 'password' ? 'text' : 'password';
}

function copyToken() {
  const t = document.getElementById('api-token-display').value;
  navigator.clipboard.writeText(t).then(() => {
    const btn = event.target;
    btn.textContent = 'Copied!';
    setTimeout(() => btn.textContent = 'Copy', 1500);
  });
}

async function loadLicense() {
  try {
    const lic = await (await fetch('/api/license/status')).json();
    const info = document.getElementById('license-info');
    if (lic.mode === 'licensed') {
      info.innerHTML = `
        <div class="flex items-center gap-2 mb-2">
          <span class="badge bg-green-900/50 text-green-400">${lic.license_tier || 'Licensed'}</span>
        </div>
        <p class="text-sm text-gray-400">Licensed to: <span class="text-gray-300">${lic.license_holder}</span></p>
        <p class="text-sm text-green-400 mt-1">Full access — all modules and features unlocked.</p>`;
    } else if (lic.mode === 'demo') {
      const days = Math.ceil((lic.demo_remaining_secs || 0) / 86400);
      const hours = Math.floor((lic.demo_remaining_secs || 0) / 3600);
      info.innerHTML = `
        <div class="flex items-center gap-2 mb-2">
          <span class="badge bg-yellow-900/50 text-yellow-400">Demo Mode</span>
          <span class="text-sm text-yellow-400">${days} day${days !== 1 ? 's' : ''} remaining</span>
        </div>
        <p class="text-sm text-gray-400">Full access during demo period. Enter a license key below to keep using Syntaur after the trial.</p>
        <div class="mt-2 w-full bg-gray-700 rounded-full h-1.5">
          <div class="bg-yellow-500 h-1.5 rounded-full" style="width: ${Math.min(100, ((259200 - (lic.demo_remaining_secs || 0)) / 259200) * 100)}%"></div>
        </div>`;
    } else {
      info.innerHTML = `
        <div class="flex items-center gap-2 mb-2">
          <span class="badge bg-red-900/50 text-red-400">Demo Expired</span>
        </div>
        <p class="text-sm text-gray-400">Your demo has ended. You can still use core features (${lic.daily_limit} conversations/day).</p>
        <p class="text-sm text-gray-400 mt-1">Enter a license key below to unlock full access.</p>`;
    }
  } catch(e) {}
}

async function activateLicense() {
  const key = document.getElementById('license-key-input').value.trim();
  if (!key) return;
  const result = document.getElementById('license-result');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Activating...';

  try {
    const resp = await fetch('/api/license/activate', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ key: key })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm text-green-400';
      result.textContent = data.message;
      loadLicense();
    } else {
      result.className = 'text-sm text-red-400';
      result.textContent = data.error || 'Activation failed';
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Network error';
  }
}

async function checkForUpdates() {
  const btn = document.getElementById('btn-check-update');
  btn.textContent = 'Checking...';
  try {
    const resp = await fetch(`/api/updates/check?token=${token}`, { headers: { 'Authorization': 'Bearer ' + token } });
    const data = await resp.json();
    document.getElementById('update-version-text').textContent = `Current: v${data.current_version}${data.latest_version ? ' | Latest: v' + data.latest_version : ''}`;
    if (data.update_available) {
      document.getElementById('update-available').classList.remove('hidden');
      document.getElementById('update-notes').textContent = data.release_notes || '';
      document.getElementById('update-download-link').href = data.download_url || '#';
      document.getElementById('update-status-text').textContent = 'A newer version is available.';
    } else if (data.error) {
      document.getElementById('update-status-text').textContent = data.error;
    } else {
      document.getElementById('update-status-text').textContent = 'You are up to date.';
    }
  } catch(e) {
    document.getElementById('update-status-text').textContent = 'Could not check for updates.';
  }
  btn.textContent = 'Check Now';

  // Also check tax brackets
  try {
    const resp = await fetch(`/api/tax/brackets/status?token=${token}`, { headers: { 'Authorization': 'Bearer ' + token } });
    const data = await resp.json();
    const el = document.getElementById('bracket-status-text');
    if (data.stale) {
      el.innerHTML = `<span class="text-yellow-400">${data.warning}</span>`;
    } else {
      el.textContent = `Up to date (last updated ${data.last_updated || 'unknown'}, years: ${(data.available_years || []).join(', ')})`;
    }
  } catch(e) {}
}

// Auto-check on page load
setTimeout(checkForUpdates, 1000);

async function uploadAvatar(agentId, input) {
  const file = input.files[0];
  if (!file) return;
  try {
    const resp = await fetch(`/api/agent-avatar/${agentId}`, {
      method: 'POST',
      headers: { 'Content-Type': file.type, 'Authorization': 'Bearer ' + token },
      body: file
    });
    const data = await resp.json();
    if (data.success) {
      const img = document.getElementById(`avatar-img-${agentId}`);
      if (img) img.src = `/agent-avatar/${agentId}?t=${Date.now()}`;
    }
  } catch(e) { console.log('upload error:', e); }
  input.value = '';
}

// ── Users tab ────────────────────────────────────────────────────────────
async function loadUsers() {
  try {
    const data = await authFetch('/api/admin/users').then(r => r.json());
    const users = data.users || [];
    const list = document.getElementById('users-list');
    if (users.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-500">No users.</p>';
      return;
    }
    list.innerHTML = users.map(u => `
      <div class="flex items-center justify-between bg-gray-800 rounded-lg p-3">
        <div>
          <span class="font-medium text-sm">${esc(u.name)}</span>
          <span class="ml-2 text-xs px-2 py-0.5 rounded-full ${u.role === 'admin' ? 'bg-sky-900 text-sky-300' : 'bg-gray-700 text-gray-400'}">${esc(u.role)}</span>
          ${u.disabled ? '<span class="ml-1 text-xs text-red-400">(disabled)</span>' : ''}
        </div>
        <div class="flex gap-2 text-xs">
          ${u.id !== 1 ? `
            <button onclick="toggleUser(${u.id}, ${u.disabled})" class="px-2 py-1 rounded bg-gray-700 hover:bg-gray-600">${u.disabled ? 'Enable' : 'Disable'}</button>
            <button onclick="deleteUser(${u.id}, '${esc(u.name)}')" class="px-2 py-1 rounded bg-red-900/30 hover:bg-red-900/50 text-red-400">Delete</button>
          ` : ''}
        </div>
      </div>
    `).join('');
  } catch(e) { console.error('loadUsers', e); }
}

async function toggleUser(id, currentlyDisabled) {
  await authFetch(`/api/admin/users/${id}`, {
    method: 'PUT',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({ token, disabled: !currentlyDisabled })
  });
  loadUsers();
}

async function deleteUser(id, name) {
  if (!confirm(`Delete user "${name}"? This cannot be undone.`)) return;
  await authFetch(`/api/admin/users/${id}?token=${encodeURIComponent(token)}`, { method: 'DELETE' });
  loadUsers();
}

let inviteSharingOptions = [];

function showInviteDialog() {
  document.getElementById('invite-result').classList.add('hidden');
  document.getElementById('invite-step1').classList.remove('hidden');
  document.getElementById('invite-step2').classList.add('hidden');
  document.getElementById('invite-dialog').classList.remove('hidden');
}

function showInviteStep1() {
  document.getElementById('invite-step1').classList.remove('hidden');
  document.getElementById('invite-step2').classList.add('hidden');
}

async function showInviteStep2() {
  document.getElementById('invite-step1').classList.add('hidden');
  document.getElementById('invite-step2').classList.remove('hidden');
  // Load sharing options
  try {
    const data = await authFetch('/api/admin/sharing/options?token=' + encodeURIComponent(token)).then(r => r.json());
    inviteSharingOptions = data.resource_types || [];
    const container = document.getElementById('invite-sharing-cats');
    container.innerHTML = inviteSharingOptions.map(rt => {
      const isOauth = rt.type === 'oauth';
      const warn = isOauth ? '<span class="text-amber-400 text-xs ml-1">(uses your connected account)</span>' : '';
      return `<label class="flex items-center gap-3 p-3 rounded-lg bg-gray-900 cursor-pointer hover:bg-gray-800">
        <input type="checkbox" class="sharing-check accent-sky-500" data-type="${esc(rt.type)}" data-id="*">
        <div>
          <span class="text-sm text-gray-300">${esc(rt.label)}</span>${warn}
        </div>
      </label>`;
    }).join('');
  } catch {}
}

async function sendInvite() {
  const btn = document.getElementById('invite-btn');
  btn.textContent = 'Creating...';
  // Build sharing preset from checkboxes
  const checks = document.querySelectorAll('.sharing-check:checked');
  const preset = Array.from(checks).map(c => ({
    resource_type: c.dataset.type,
    resource_id: c.dataset.id || '*'
  }));
  try {
    const data = await authFetch('/api/admin/invites', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({
        token,
        name_hint: document.getElementById('invite-name').value.trim() || null,
        role: document.getElementById('invite-role').value,
        sharing_preset: preset.length > 0 ? JSON.stringify(preset) : null,
      })
    }).then(r => r.json());
    if (data.ok) {
      const url = window.location.origin + data.register_url;
      document.getElementById('invite-result').innerHTML = `
        <p class="text-green-400 mb-1">Invite created!</p>
        <input type="text" value="${esc(url)}" class="w-full bg-gray-900 border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-300" readonly onclick="this.select()">
        <p class="text-xs text-gray-500 mt-1">Share this link. Expires in 72 hours.</p>
      `;
      document.getElementById('invite-result').classList.remove('hidden');
      loadInvites();
    } else {
      document.getElementById('invite-result').innerHTML = `<p class="text-red-400">${esc(data.error)}</p>`;
      document.getElementById('invite-result').classList.remove('hidden');
    }
  } catch(e) {
    document.getElementById('invite-result').innerHTML = `<p class="text-red-400">${e.message}</p>`;
    document.getElementById('invite-result').classList.remove('hidden');
  }
  btn.textContent = 'Create Invite';
}

async function loadInvites() {
  try {
    const data = await authFetch('/api/admin/invites?token=' + encodeURIComponent(token)).then(r => r.json());
    const invites = data.invites || [];
    const list = document.getElementById('invites-list');
    const active = invites.filter(i => !i.consumed_at && (i.expires_at * 1000) > Date.now());
    if (active.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-500">No pending invites.</p>';
      return;
    }
    list.innerHTML = active.map(i => `
      <div class="flex items-center justify-between bg-gray-800 rounded-lg p-3 text-sm">
        <div>
          <span class="text-gray-300">${esc(i.name_hint || 'No name')}</span>
          <span class="ml-2 text-xs text-gray-500">${esc(i.role)}</span>
        </div>
        <span class="text-xs text-gray-500">expires ${new Date(i.expires_at * 1000).toLocaleDateString()}</span>
      </div>
    `).join('');
  } catch(e) { console.error('loadInvites', e); }
}

async function loadSharingMode() {
  try {
    const data = await authFetch('/api/admin/sharing?token=' + encodeURIComponent(token)).then(r => r.json());
    const mode = data.mode || 'shared';
    document.querySelectorAll('input[name="sharing"]').forEach(r => { r.checked = r.value === mode; });
  } catch(e) { console.error('loadSharingMode', e); }
}

async function setSharingMode(mode) {
  await authFetch('/api/admin/sharing', {
    method: 'PUT',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({ token, mode })
  });
}

async function changePassword() {
  const btn = document.getElementById('pw-btn');
  const status = document.getElementById('pw-status');
  btn.textContent = 'Updating...';
  status.classList.add('hidden');
  try {
    const data = await authFetch('/api/me/password', {
      method: 'PUT',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({
        token,
        current_password: document.getElementById('pw-current').value || null,
        new_password: document.getElementById('pw-new').value,
      })
    }).then(r => r.json());
    if (data.ok) {
      status.textContent = 'Password updated';
      status.className = 'text-xs text-green-400';
    } else {
      status.textContent = data.error || 'Failed';
      status.className = 'text-xs text-red-400';
    }
    status.classList.remove('hidden');
    document.getElementById('pw-current').value = '';
    document.getElementById('pw-new').value = '';
  } catch(e) {
    status.textContent = e.message;
    status.className = 'text-xs text-red-400';
    status.classList.remove('hidden');
  }
  btn.textContent = 'Update Password';
}

// Check if user is admin, show/hide users tab
(async function() {
  try {
    const data = await authFetch('/api/me?token=' + encodeURIComponent(token)).then(r => r.json());
    if (data.role === 'admin') {
      document.getElementById('users-tab-btn').style.display = '';
      loadUsers();
      loadInvites();
      loadSharingMode();
    } else {
      document.getElementById('users-tab-btn').style.display = 'none';
    }
  } catch {}
})();

loadSettings();
loadLicense();

// ── Media Bridge panel ────────────────────────────────────────────────
const MB_URL = 'http://127.0.0.1:18790';
async function probeMediaBridge() {
  const badge = document.getElementById('mb-status-badge');
  const details = document.getElementById('mb-details');
  const install = document.getElementById('mb-install');
  if (!badge) return;
  try {
    const s = await (await fetch(MB_URL + '/status')).json();
    badge.className = 'badge badge-green';
    badge.textContent = 'running';
    details.classList.remove('hidden');
    install.classList.add('hidden');
    document.getElementById('mb-version').textContent = 'v' + s.version;
    document.getElementById('mb-backend').textContent = s.audio_backend;
    const authed = s.authed_providers || [];
    document.getElementById('mb-authed').innerHTML = authed.length
      ? authed.map(p => `<span class="badge badge-green mr-1">${p}</span>`).join('')
      : '<span class="text-gray-500">none yet — log in to a service below</span>';
  } catch (e) {
    badge.className = 'badge badge-red';
    badge.textContent = 'not running';
    details.classList.add('hidden');
    install.classList.remove('hidden');
  }
}
function copyAuthCmd(provider) {
  const cmd = `syntaur-media-bridge --auth-setup --auth-provider ${provider}`;
  navigator.clipboard?.writeText(cmd).then(() => {
    document.getElementById('mb-copy-hint').textContent =
      `Copied: ${cmd} — run in a terminal on this computer.`;
  }).catch(() => {
    document.getElementById('mb-copy-hint').textContent =
      `Run in a terminal on this computer: ${cmd}`;
  });
}
probeMediaBridge();
setInterval(probeMediaBridge, 10000);

async function installShortcut(target) {
  const btn = document.getElementById(target === 'desktop' ? 'btn-shortcut-desktop' : 'btn-shortcut-menu');
  const status = document.getElementById('shortcut-status');
  btn.disabled = true;
  btn.textContent = 'Installing...';
  status.textContent = '';
  try {
    const resp = await fetch('/api/settings/install-shortcut', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token },
      body: JSON.stringify({ target: target })
    });
    const data = await resp.json();
    if (data.success) {
      status.className = 'text-xs text-green-400';
      status.textContent = data.message;
    } else {
      status.className = 'text-xs text-red-400';
      status.textContent = data.message || 'Failed to install shortcut';
    }
  } catch(e) {
    status.className = 'text-xs text-red-400';
    status.textContent = 'Connection error';
  }
  btn.disabled = false;
  btn.textContent = target === 'desktop' ? 'Add to Desktop' : 'Install App Shortcut';
}
