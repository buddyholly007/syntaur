//! /setup — migrated from static/setup.html. Structural markup and
//! embedded scripts live as raw-string consts below so their bytes
//! count as Rust and the file compiles type-checked through maud.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Setup",
        authed: false,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! { (PreEscaped(BODY_HTML)) };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
  .step { display: none; }
  .step.active { display: block; }
  .fade-in { animation: fadeIn 0.3s ease-in; }
  @keyframes fadeIn { from { opacity: 0; transform: translateY(10px); } to { opacity: 1; transform: translateY(0); } }
  .card { @apply bg-gray-800 rounded-xl border border-gray-700 p-6; }
  .btn-primary { @apply bg-oc-600 hover:bg-oc-700 text-white font-medium py-2.5 px-6 rounded-lg transition-colors; }
  .btn-secondary { @apply bg-gray-700 hover:bg-gray-600 text-gray-200 font-medium py-2.5 px-6 rounded-lg transition-colors; }
  .input { @apply w-full bg-gray-900 border border-gray-600 rounded-lg px-4 py-2.5 text-white placeholder-gray-400 focus:border-oc-500 focus:ring-1 focus:ring-oc-500 outline-none; }
  .label { @apply block text-sm font-medium text-gray-300 mb-1.5; }
  .badge { @apply inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium; }
  .badge-green { @apply bg-green-900 text-green-300; }
  .badge-yellow { @apply bg-yellow-900 text-yellow-300; }
  .badge-red { @apply bg-red-900 text-red-300; }
  .badge-blue { @apply bg-blue-900 text-blue-300; }
  .toggle { @apply relative inline-flex h-6 w-11 items-center rounded-full transition-colors cursor-pointer; }
  .toggle-dot { @apply inline-block h-4 w-4 transform rounded-full bg-white transition-transform; }"##;

const BODY_HTML: &str = r##"<div class="max-w-2xl mx-auto px-4 py-8">

  <!-- Header -->
  <div class="text-center mb-8">
    <h1 class="text-3xl font-bold text-white mb-2">Syntaur</h1>
    <p class="text-gray-400">Your personal AI platform</p>
    <!-- Centaur emblem placeholder -->
    <img src="/app-icon.jpg" class="w-12 h-12 rounded-xl mx-auto mt-2" alt="">
    <!-- Progress bar (hidden until server mode chosen) -->
    <div class="mt-6 flex items-center gap-1 hidden" id="progress">
      <div class="h-1 rounded-full flex-1 bg-oc-600 transition-all" id="prog-1"></div>
      <div class="h-1 rounded-full flex-1 bg-gray-700 transition-all" id="prog-2"></div>
      <div class="h-1 rounded-full flex-1 bg-gray-700 transition-all" id="prog-3"></div>
      <div class="h-1 rounded-full flex-1 bg-gray-700 transition-all" id="prog-4"></div>
      <div class="h-1 rounded-full flex-1 bg-gray-700 transition-all" id="prog-5"></div>
      <div class="h-1 rounded-full flex-1 bg-gray-700 transition-all" id="prog-6"></div>
    </div>
  </div>

  <!-- Step 0: Server vs Connect -->
  <div class="step active fade-in" id="step-0">
    <div class="card text-center py-8">
      <h2 class="text-2xl font-semibold mb-2">Welcome to Syntaur</h2>
      <p class="text-gray-400 mb-8">Your personal AI platform</p>

      <div class="space-y-4 max-w-md mx-auto text-left">
        <button onclick="chooseServer()" class="w-full p-5 rounded-xl bg-gray-900 hover:bg-gray-800 border border-gray-700 hover:border-oc-600 transition-all text-left group">
          <div class="flex items-start gap-4">
            <div class="text-2xl mt-1">&#9881;</div>
            <div>
              <p class="font-semibold text-white group-hover:text-oc-400 transition-colors">Set up Syntaur on this computer</p>
              <p class="text-sm text-gray-400 mt-1">This computer will run your AI. You can access it from your phone, laptop, or any other device on your network.</p>
              <p class="text-xs text-yellow-400/70 mt-2">&#9888; This computer needs to stay on for Syntaur to work.</p>
            </div>
          </div>
        </button>

        <button onclick="chooseConnect()" class="w-full p-5 rounded-xl bg-gray-900 hover:bg-gray-800 border border-gray-700 hover:border-oc-600 transition-all text-left group">
          <div class="flex items-start gap-4">
            <div class="text-2xl mt-1">&#128279;</div>
            <div>
              <p class="font-semibold text-white group-hover:text-oc-400 transition-colors">Connect to my Syntaur server</p>
              <p class="text-sm text-gray-400 mt-1">Syntaur is already running on another computer. Connect this device to it.</p>
            </div>
          </div>
        </button>
      </div>

      <p class="text-xs text-gray-600 mt-6">For the best experience, run Syntaur on a computer that stays on — a NAS, mini PC, or desktop.</p>
    </div>
  </div>

  <!-- Step Connect: Enter server URL -->
  <div class="step fade-in" id="step-connect">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Connect to your Syntaur server</h2>
      <p class="text-gray-400 mb-6">Enter the address of the computer running Syntaur. You can find this on the server's dashboard or in its startup log.</p>

      <div class="space-y-4">
        <div>
          <label class="label">Server address</label>
          <input type="text" id="connect-url" class="input" placeholder="e.g. 192.168.1.50:18789 or my-server.tail1234.ts.net:18789">
          <p class="text-xs text-gray-500 mt-1">Local IP, Tailscale hostname, or domain name. Port is usually 18789.</p>
        </div>

        <div id="connect-status" class="hidden p-3 rounded-lg text-sm"></div>

        <div class="flex justify-between">
          <button class="btn-secondary" onclick="goStep(0)">Back</button>
          <button class="btn-primary" onclick="testConnect()" id="btn-connect">Connect</button>
        </div>
      </div>
    </div>
  </div>

  <!-- Step Connect Done -->
  <div class="step fade-in" id="step-connect-done">
    <div class="card text-center py-12">
      <div class="text-5xl mb-4">&#10003;</div>
      <h2 class="text-2xl font-semibold mb-2">Connected!</h2>
      <p class="text-gray-400 mb-4">This device is linked to your Syntaur server at <strong class="text-white" id="connect-done-url"></strong></p>
      <p class="text-gray-400 mb-8">Your conversations, settings, and AI follow you across every device.</p>

      <div class="flex gap-4 justify-center">
        <a id="connect-done-link" href="/" class="btn-primary inline-block">Open Dashboard</a>
      </div>

      <div class="mt-8 p-4 rounded-lg bg-gray-900 text-left max-w-md mx-auto">
        <p class="text-sm font-medium text-gray-300 mb-2">How to open Syntaur later</p>
        <p class="text-xs text-gray-500">A <strong class="text-gray-400">Syntaur</strong> shortcut was added to your app launcher. You can also bookmark this page or go to:</p>
        <p class="text-xs text-sky-400 mt-1 font-mono" id="connect-done-bookmark"></p>
        <p class="text-xs text-gray-500 mt-2">Your conversations sync automatically — pick up where you left off on any device.</p>
      </div>
    </div>
  </div>

  <!-- Step 1: Welcome + Hardware Scan -->
  <div class="step fade-in" id="step-1">
    <div class="card">
      <h2 class="text-xl font-semibold mb-4">Welcome to Syntaur</h2>
      <p class="text-gray-400 mb-6">Let's set up your personal AI assistant. This will only take a few minutes.</p>
      <div id="scan-status" class="mb-6">
        <div class="flex items-center gap-3 text-gray-400">
          <svg class="animate-spin h-5 w-5" viewBox="0 0 24 24"><circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" fill="none"></circle><path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path></svg>
          Scanning your hardware...
        </div>
      </div>
      <div id="scan-results" class="hidden space-y-3">
        <div id="hw-cpu" class="flex justify-between"><span class="text-gray-400">CPU</span><span id="hw-cpu-val"></span></div>
        <div id="hw-ram" class="flex justify-between"><span class="text-gray-400">RAM</span><span id="hw-ram-val"></span></div>
        <div id="hw-gpu" class="flex justify-between"><span class="text-gray-400">GPU</span><span id="hw-gpu-val"></span></div>
        <div id="hw-disk" class="flex justify-between"><span class="text-gray-400">Disk</span><span id="hw-disk-val"></span></div>
        <div id="hw-tier" class="mt-4 p-3 rounded-lg bg-gray-900">
          <span class="text-sm text-gray-400">Hardware tier: </span>
          <span id="hw-tier-badge" class="badge"></span>
        </div>
        <!-- Firewall blocked banner -->
      <div id="gpu-blocked-banner" class="hidden mt-3 p-4 rounded-lg bg-yellow-900/20 border border-yellow-800 space-y-3">
        <div class="flex items-start gap-2">
          <span class="text-yellow-400 mt-0.5">&#9888;</span>
          <div>
            <p class="text-sm font-medium text-yellow-300">We found computers on your network but couldn't check them for GPUs</p>
            <p class="text-sm text-gray-400 mt-1">If you have a GPU in another computer (like a gaming PC or workstation), Syntaur can use it over your network. We just need to set up a secure connection first.</p>
          </div>
        </div>

        <div class="space-y-2">
          <!-- Option 1: Guided setup -->
          <button onclick="showGpuGuide()" class="w-full text-left p-3 rounded-lg bg-gray-800 hover:bg-gray-700 border border-gray-700 transition-colors">
            <p class="text-sm font-medium text-gray-200">I have a GPU on another computer — help me connect it</p>
            <p class="text-xs text-gray-500">Guided setup, takes about 2 minutes. We'll walk you through each step.</p>
          </button>

          <!-- Option 2: Skip -->
          <button onclick="document.getElementById('gpu-blocked-banner').classList.add('hidden')" class="w-full text-left p-3 rounded-lg bg-gray-800 hover:bg-gray-700 border border-gray-700 transition-colors">
            <p class="text-sm font-medium text-gray-400">I don't have a GPU on another computer</p>
            <p class="text-xs text-gray-500">No problem — you can use a cloud AI service instead (free options available).</p>
          </button>
        </div>

        <!-- Guided GPU connection setup (hidden until clicked) -->
        <div id="gpu-guide" class="hidden space-y-4 mt-2">
          <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-3">
            <p class="text-gray-300 font-medium">What is this?</p>
            <p class="text-gray-400">Syntaur needs a way to securely connect to your other computer to check if it has a GPU and use it for AI processing. This is done using <strong>SSH</strong> — a standard, secure protocol that all computers support. Think of it like giving Syntaur a key to your other computer's door.</p>
          </div>

          <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-3">
            <p class="text-gray-300 font-medium">Step 1: Find this computer's key</p>
            <p class="text-gray-400">Syntaur generated a key when you installed it. Here it is:</p>
            <div class="bg-gray-900 rounded-lg p-3 font-mono text-xs text-oc-500 break-all select-all" id="gpu-guide-pubkey">Loading...</div>
            <p class="text-gray-400">Copy this entire line — you'll paste it on your GPU computer in Step 2.</p>
            <button onclick="copyGpuKey()" class="text-xs text-oc-500 hover:text-oc-400 font-medium">Copy Key</button>
          </div>

          <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-3">
            <p class="text-gray-300 font-medium">Step 2: Add the key to your GPU computer</p>
            <p class="text-gray-400">Go to the computer that has the GPU and open a terminal (command prompt), then run this command:</p>

            <div>
              <p class="text-xs text-gray-500 mb-1"><strong class="text-gray-400">Linux / Mac:</strong> Open Terminal and paste:</p>
              <div class="bg-gray-900 rounded-lg p-2 font-mono text-xs text-gray-300 break-all">
                echo "<span id="gpu-guide-key-linux">KEY</span>" >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
              </div>
            </div>

            <div>
              <p class="text-xs text-gray-500 mb-1"><strong class="text-gray-400">Windows:</strong> Open PowerShell and paste:</p>
              <div class="bg-gray-900 rounded-lg p-2 font-mono text-xs text-gray-300 break-all">
                Add-Content "$env:USERPROFILE\.ssh\authorized_keys" "<span id="gpu-guide-key-windows">KEY</span>"
              </div>
            </div>

            <p class="text-xs text-gray-500">
              <strong>What does this do?</strong> It adds Syntaur's key to the list of trusted keys on your GPU computer. This lets Syntaur connect securely without needing a password. You can remove it anytime by editing the <code class="bg-gray-900 px-1 rounded">authorized_keys</code> file.
            </p>

            <details class="text-xs text-gray-600">
              <summary class="cursor-pointer hover:text-gray-400">Don't have SSH set up on the GPU computer?</summary>
              <div class="mt-2 space-y-1 text-gray-500">
                <p><strong>Linux:</strong> SSH is usually pre-installed. If not: <code class="bg-gray-900 px-1 rounded">sudo apt install openssh-server</code></p>
                <p><strong>Mac:</strong> Go to System Settings → General → Sharing → enable "Remote Login"</p>
                <p><strong>Windows:</strong> Go to Settings → Apps → Optional Features → add "OpenSSH Server", then start it: <code class="bg-gray-900 px-1 rounded">Start-Service sshd</code></p>
              </div>
            </details>
          </div>

          <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-3">
            <p class="text-gray-300 font-medium">Step 3: Enter the GPU computer's address</p>
            <p class="text-gray-400">What is the IP address or hostname of the computer with the GPU?</p>
            <p class="text-xs text-gray-500">Not sure? On the GPU computer, run: <code class="bg-gray-900 px-1 rounded">hostname -I</code> (Linux/Mac) or <code class="bg-gray-900 px-1 rounded">ipconfig</code> (Windows, look for IPv4 Address)</p>
            <div class="flex gap-2">
              <input class="input flex-1" id="gpu-guide-ip" placeholder="192.168.1.xxx">
              <select class="input w-32" id="gpu-guide-user">
                <option value="">Username</option>
              </select>
            </div>
            <div>
              <label class="label">Username on the GPU computer</label>
              <input class="input" id="gpu-guide-username" placeholder="Your login name on the GPU computer (e.g. sean, admin)">
              <p class="text-xs text-gray-500 mt-1">This is the username you use to log into that computer.</p>
            </div>
            <button onclick="testGpuConnection()" class="bg-oc-600 hover:bg-oc-700 text-white font-medium py-2 px-4 rounded-lg transition-colors text-sm">Test Connection &amp; Scan for GPU</button>
            <div id="gpu-guide-result" class="hidden text-sm mt-2"></div>
          </div>
        </div>

        <div id="firewall-result" class="hidden text-sm"></div>
      </div>

      <!-- GPU Role Assignment (multi-GPU or large single GPU) -->
      <div id="gpu-roles" class="hidden mt-3 p-4 rounded-lg bg-gray-900 border border-gray-700 space-y-3">
        <div>
          <p class="text-sm font-medium text-gray-300">Assign GPU roles</p>
          <p class="text-xs text-gray-500">Different AI tasks need different amounts of GPU memory. Assign each GPU to a role for the best performance.</p>
        </div>
        <div id="gpu-role-list" class="space-y-2"></div>
        <div id="gpu-role-suggestion" class="text-xs text-gray-500 p-2 rounded bg-gray-800"></div>
      </div>

      <div id="hw-network" class="hidden mt-3 p-3 rounded-lg bg-gray-900 space-y-1">
          <p class="text-sm font-medium text-green-400">&#10003; Network LLM services found:</p>
          <div id="hw-network-list" class="space-y-1"></div>
        </div>
      </div>
    </div>
    <div class="flex justify-end mt-6">
      <button class="btn-primary" onclick="goStep(2)" id="btn-next-1" disabled>Next</button>
    </div>
  </div>

  <!-- Step 2: LLM Backend -->
  <div class="step fade-in" id="step-2">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Choose your AI brain</h2>
      <p class="text-gray-400 mb-6" id="llm-recommendation"></p>

      <div class="space-y-3">
        <label class="block p-4 rounded-lg border-2 cursor-pointer transition-colors border-gray-700 hover:border-oc-600" id="llm-opt-local">
          <input type="radio" name="llm" value="local" class="hidden" onchange="selectLlm('local')">
          <div class="flex items-center justify-between">
            <div>
              <span class="font-medium">Local (Ollama)</span>
              <p class="text-sm text-gray-400 mt-1" id="llm-local-desc">Run a model on your hardware — private, free, no internet needed</p>
              <a href="https://ollama.com/download" target="_blank" class="text-xs text-oc-500 hover:text-oc-400" onclick="event.stopPropagation()">Download Ollama &#8599;</a>
            </div>
            <span class="badge badge-green">Private</span>
          </div>
        </label>

        <label class="block p-4 rounded-lg border-2 cursor-pointer transition-colors border-gray-700 hover:border-oc-600" id="llm-opt-network">
          <input type="radio" name="llm" value="network" class="hidden" onchange="selectLlm('network')">
          <div class="flex items-center justify-between">
            <div><span class="font-medium">Network LLM</span><p class="text-sm text-gray-400 mt-1" id="llm-network-desc">Use a service on your local network</p></div>
            <span class="badge badge-blue">LAN</span>
          </div>
        </label>

        <label class="block p-4 rounded-lg border-2 cursor-pointer transition-colors border-gray-700 hover:border-oc-600" id="llm-opt-cloud">
          <input type="radio" name="llm" value="cloud" class="hidden" onchange="selectLlm('cloud')">
          <div class="flex items-center justify-between">
            <div><span class="font-medium">Cloud API</span><p class="text-sm text-gray-400 mt-1">Free: OpenRouter, Groq, Cerebras. Paid: OpenAI, Anthropic</p></div>
            <span class="badge badge-yellow">Cloud</span>
          </div>
        </label>
      </div>

      <!-- Cloud API key input (shown when cloud selected) -->
      <div id="cloud-config" class="hidden mt-4 space-y-3">
        <div>
          <label class="label">Provider</label>
          <select id="cloud-provider" class="input" onchange="updateCloudProvider()">
            <option value="openrouter">OpenRouter (free, tool-capable)</option>
            <option value="groq">Groq (free, ~250 tok/s)</option>
            <option value="cerebras">Cerebras (free, ~2000 tok/s — fastest)</option>
            <option value="openai">OpenAI ($5-15/mo)</option>
            <option value="anthropic">Anthropic ($10-30/mo)</option>
          </select>
        </div>
        <div id="provider-help" class="p-3 rounded-lg bg-gray-900 text-sm">
          <p class="text-gray-300 mb-2">Get your free API key in under a minute:</p>
          <a id="provider-signup-link" href="https://openrouter.ai/settings/keys" target="_blank" class="inline-flex items-center gap-1.5 text-oc-500 hover:text-oc-400 font-medium">
            Open OpenRouter Keys Page &#8599;
          </a>
          <p class="text-gray-500 mt-1.5" id="provider-help-detail">Free tier includes tool-capable models. No credit card required.</p>
        </div>
        <div>
          <label class="label">API Key</label>
          <input type="password" id="cloud-key" class="input" placeholder="sk-...">
          <p class="text-xs text-gray-500 mt-1">Paste the key you just created above</p>
        </div>
        <div class="flex items-center gap-3">
          <button class="btn-secondary text-sm" onclick="testLlm()">Test Connection</button>
          <div id="llm-test-result" class="hidden text-sm"></div>
        </div>
      </div>

      <!-- Fallback section -->
      <div class="mt-6 p-4 rounded-lg bg-gray-900 border border-gray-700">
        <div class="flex items-center justify-between mb-3">
          <div class="flex items-start gap-2">
            <span class="text-yellow-400 mt-0.5">&#9888;</span>
            <div>
              <p class="text-sm font-medium text-gray-200">Set up a backup LLM</p>
              <p class="text-sm text-gray-400 mt-1">If your primary goes down, Syntaur automatically switches to the fallback. Recommended but not required.</p>
            </div>
          </div>
        </div>

        <div id="fallback-config" class="space-y-3">
          <div>
            <label class="label">Fallback provider</label>
            <select id="fallback-provider" class="input" onchange="updateFallbackProvider()">
              <option value="none">Skip — no fallback</option>
              <option value="openrouter">OpenRouter (free tier)</option>
              <option value="groq">Groq (free, fast)</option>
              <option value="cerebras">Cerebras (free, fastest)</option>
              <option value="ollama">Local model (Ollama)</option>
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
            </select>
          </div>

          <div id="fallback-details" class="hidden space-y-3">
            <div id="fallback-help" class="p-3 rounded-lg bg-gray-800 text-sm">
              <p class="text-gray-400" id="fallback-help-text"></p>
              <a id="fallback-help-link" href="#" target="_blank" class="inline-flex items-center gap-1 text-xs text-oc-500 hover:text-oc-400 font-medium mt-1"></a>
            </div>
            <div id="fallback-key-row">
              <label class="label">API Key</label>
              <input type="password" id="fallback-key" class="input" placeholder="sk-...">
            </div>
          </div>
        </div>

        <p class="text-xs text-gray-600 mt-3" id="fallback-skip-note">You can always add a fallback later in Settings.</p>
      </div>

      <!-- Image generation (free-first) -->
      <div class="mt-6 p-4 rounded-lg bg-gray-900 border border-gray-700">
        <div class="mb-3">
          <p class="text-sm font-medium text-gray-200">Image generation</p>
          <p class="text-xs text-gray-500 mt-1">How should Syntaur generate images when you ask for one? All options have free paths — paid is opt-in only.</p>
        </div>
        <div class="space-y-2">
          <label class="block p-3 rounded-lg border-2 cursor-pointer transition-colors border-oc-600" id="img-opt-pollinations">
            <input type="radio" name="image-provider" value="pollinations" class="hidden" checked onchange="selectImageProvider('pollinations')">
            <div class="flex items-start gap-3">
              <span class="badge badge-green text-xs flex-shrink-0 mt-0.5">Free · No signup</span>
              <div class="flex-1">
                <p class="text-sm font-medium">Pollinations.ai (recommended)</p>
                <p class="text-xs text-gray-500 mt-0.5">Free public image API, no key, no account. Uses FLUX.1 under the hood. Anonymous and instant — zero config. Optional watermark disabled. <a href="https://pollinations.ai" target="_blank" class="text-oc-500 hover:text-oc-400">Learn more &#8599;</a></p>
              </div>
            </div>
          </label>
          <label class="block p-3 rounded-lg border-2 cursor-pointer transition-colors border-gray-700 hover:border-oc-600" id="img-opt-local">
            <input type="radio" name="image-provider" value="local" class="hidden" onchange="selectImageProvider('local')">
            <div class="flex items-start gap-3">
              <span class="badge badge-blue text-xs flex-shrink-0 mt-0.5">Free · Local GPU</span>
              <div class="flex-1">
                <p class="text-sm font-medium">Local Stable Diffusion</p>
                <p class="text-xs text-gray-500 mt-0.5">Run ComfyUI / Automatic1111 / SD.Next on your own GPU. Fully private, free forever, usually 3-8s per image on a modern card. Best quality since you pick the model. <a href="https://github.com/comfyanonymous/ComfyUI" target="_blank" class="text-oc-500 hover:text-oc-400">Install ComfyUI &#8599;</a></p>
                <div id="img-local-config" class="hidden mt-2 space-y-2">
                  <input type="text" id="img-local-url" placeholder="http://192.168.1.69:7860" class="input text-xs" value="http://127.0.0.1:7860">
                  <p class="text-[11px] text-gray-600">Enter your SD server URL. Leave blank to skip — falls back to Pollinations until you wire it up in Settings.</p>
                </div>
              </div>
            </div>
          </label>
          <label class="block p-3 rounded-lg border-2 cursor-pointer transition-colors border-gray-700 hover:border-oc-600" id="img-opt-openrouter">
            <input type="radio" name="image-provider" value="openrouter" class="hidden" onchange="selectImageProvider('openrouter')">
            <div class="flex items-start gap-3">
              <span class="badge bg-yellow-900/50 text-yellow-400 text-xs flex-shrink-0 mt-0.5">Pay-per-use</span>
              <div class="flex-1">
                <p class="text-sm font-medium">OpenRouter (paid)</p>
                <p class="text-xs text-gray-500 mt-0.5">Uses your OpenRouter account — charges per image. Best cheap model is <code class="text-gray-400">google/gemini-2.5-flash-image</code> at ~$0.001/image. Reuses the OpenRouter key you set above. Opt-in only.</p>
                <div id="img-openrouter-config" class="hidden mt-2">
                  <input type="text" id="img-openrouter-model" placeholder="google/gemini-2.5-flash-image" class="input text-xs" value="google/gemini-2.5-flash-image">
                </div>
              </div>
            </div>
          </label>
        </div>
      </div>
    </div>
    <div class="flex justify-between mt-6">
      <button class="btn-secondary" onclick="goStep(1)">Back</button>
      <button class="btn-primary" onclick="goStep(3)" id="btn-next-2">Next</button>
    </div>
  </div>

  <!-- Step 3: Name & Voice -->
  <div class="step fade-in" id="step-3">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Name your AI</h2>
      <p class="text-gray-400 mb-6">Give your assistant a personality. You can change this anytime.</p>
      <div class="mb-6">
        <label class="label">Assistant name</label>
        <input type="text" id="agent-name" class="input" value="Claw" placeholder="Atlas, Nova, Sage...">
        <p class="text-xs text-gray-500 mt-1">This is how your AI will introduce itself</p>
      </div>
      <div class="mb-6">
        <label class="label">Your name</label>
        <input type="text" id="user-name" class="input" placeholder="What should it call you?">
      </div>
      <div class="border-t border-gray-700 pt-4">
        <div class="flex items-center justify-between mb-4">
          <div>
            <p class="font-medium">Enable voice</p>
            <p class="text-sm text-gray-400">Talk to your AI using your microphone</p>
          </div>
          <button id="voice-toggle" class="toggle bg-gray-600" onclick="toggleVoice()">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div id="voice-options" class="hidden mt-3 pt-3 border-t border-gray-700 space-y-4">

          <!-- How to talk -->
          <div>
            <label class="label">How will you talk to your AI?</label>
            <div class="space-y-2">
              <label class="flex items-start gap-3 p-3 rounded-lg bg-gray-900 cursor-pointer hover:bg-gray-800">
                <input type="radio" name="voice-input" value="browser" checked class="mt-1">
                <div>
                  <p class="text-sm font-medium text-gray-300">Browser microphone</p>
                  <p class="text-xs text-gray-500">Push-to-talk button in the chat. Works on any device with a mic.</p>
                </div>
              </label>
              <label class="flex items-start gap-3 p-3 rounded-lg bg-gray-900 cursor-pointer hover:bg-gray-800">
                <input type="radio" name="voice-input" value="satellite" class="mt-1">
                <div>
                  <p class="text-sm font-medium text-gray-300">Room speaker (voice satellite)</p>
                  <p class="text-xs text-gray-500">Hands-free with an ESP32-S3 device. Always listening with wake word.</p>
                  <a href="https://futureproofhomes.net/products" target="_blank" class="text-xs text-oc-500 hover:text-oc-400" onclick="event.stopPropagation()">Get a voice satellite &#8599;</a>
                </div>
              </label>
              <label class="flex items-start gap-3 p-3 rounded-lg bg-gray-900 cursor-pointer hover:bg-gray-800">
                <input type="radio" name="voice-input" value="telegram" class="mt-1">
                <div>
                  <p class="text-sm font-medium text-gray-300">Telegram voice messages</p>
                  <p class="text-xs text-gray-500">Send voice notes from your phone. Requires Telegram setup in the next step.</p>
                </div>
              </label>
            </div>
          </div>

          <!-- TTS voice -->
          <div>
            <label class="label">AI voice</label>
            <select id="tts-choice" class="input" onchange="updateTtsChoice()">
              <option value="piper">Piper (local, free, runs on CPU — good quality)</option>
              <option value="orpheus">Orpheus (local, free, needs GPU — very natural)</option>
              <option value="elevenlabs">ElevenLabs (cloud, paid — best quality)</option>
            </select>
            <div id="tts-help" class="mt-2 text-xs text-gray-500">
              Piper runs locally on your CPU with no internet required. Good quality for most uses.
            </div>
            <div id="tts-elevenlabs-config" class="hidden mt-2">
              <label class="label">ElevenLabs API Key</label>
              <input type="password" class="input" id="elevenlabs-key" placeholder="Paste from elevenlabs.io/app/settings/api-keys">
              <a href="https://elevenlabs.io/app/settings/api-keys" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 mt-1 inline-block">Get ElevenLabs API Key &#8599;</a>
            </div>
          </div>

          <!-- STT engine -->
          <div>
            <label class="label">Speech recognition</label>
            <select id="stt-choice" class="input">
              <option value="whisper">Whisper (local, free — works on CPU, faster with GPU)</option>
              <option value="deepgram">Deepgram (cloud, paid — fastest and most accurate)</option>
            </select>
            <p class="text-xs text-gray-500 mt-1">Local Whisper keeps all audio on your machine. Cloud is faster but sends audio to a server.</p>
          </div>

        </div>
      </div>
    </div>
    <div class="flex justify-between mt-6">
      <button class="btn-secondary" onclick="goStep(2)">Back</button>
      <button class="btn-primary" onclick="goStep(4)">Next</button>
    </div>
  </div>

  <!-- Step 4: Communication -->
  <div class="step fade-in" id="step-4">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Stay connected</h2>
      <p class="text-gray-400 mb-6">Let's set up how you'll access Syntaur. Web chat is included — set up Tailscale and/or Telegram now for the best experience, or skip and configure later in Settings.</p>

      <div class="p-4 rounded-lg bg-gray-900 mb-4">
        <div class="flex items-center gap-3">
          <span class="text-2xl">&#127760;</span>
          <div>
            <p class="font-medium">Web Chat</p>
            <p class="text-sm text-gray-400">Available on your local network at <code class="text-oc-500">localhost:18789</code></p>
          </div>
          <span class="badge badge-green ml-auto">Included</span>
        </div>
        <!-- Tailscale remote access -->
        <div class="mt-3 pt-3 border-t border-gray-800">
          <div class="flex items-center gap-2 mb-3">
            <p class="text-sm font-medium text-gray-300">Remote access with Tailscale</p>
            <span class="badge badge-blue">Recommended</span>
          </div>
          <div id="tailscale-config" class="space-y-3">
            <!-- Explainer -->
            <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-2">
              <p class="text-gray-300 font-medium">What is Tailscale?</p>
              <p class="text-gray-400">Tailscale is a free app that creates a secure, private connection between your devices. Once installed, your phone can reach your Syntaur machine as if they were on the same network — even from a coffee shop or work.</p>
              <p class="text-gray-400">No port forwarding, no complicated networking. Just install the app on both devices and they can talk to each other securely.</p>
            </div>

            <!-- How it works -->
            <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-2">
              <p class="text-gray-300 font-medium">How to set it up (5 minutes):</p>
              <p class="text-gray-400"><strong>Step 1:</strong> Install Tailscale on <strong>this machine</strong> (where Syntaur runs):</p>
              <div class="flex flex-wrap gap-2 mt-1 mb-2">
                <a href="https://tailscale.com/download/linux" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">Linux &#8599;</a>
                <a href="https://tailscale.com/download/mac" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">macOS &#8599;</a>
                <a href="https://tailscale.com/download/windows" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">Windows &#8599;</a>
              </div>
              <p class="text-gray-400"><strong>Step 2:</strong> Install Tailscale on <strong>your phone or laptop</strong> (the device you want to access Syntaur from):</p>
              <div class="flex flex-wrap gap-2 mt-1 mb-2">
                <a href="https://tailscale.com/download/ios" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">iPhone / iPad &#8599;</a>
                <a href="https://tailscale.com/download/android" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">Android &#8599;</a>
                <a href="https://tailscale.com/download" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-900 rounded">Other devices &#8599;</a>
              </div>
              <p class="text-gray-400"><strong>Step 3:</strong> Sign in with the <strong>same account</strong> on both devices (Google, Microsoft, or email).</p>
              <p class="text-gray-400"><strong>That's it.</strong> Both devices are now on your private network. Open the Tailscale URL below from your phone to access Syntaur anywhere.</p>
            </div>

            <!-- Status check -->
            <div class="p-3 rounded-lg bg-gray-800 text-sm space-y-2" id="tailscale-status">
              <p class="text-gray-400" id="tailscale-detect">Checking if Tailscale is installed on this machine...</p>
            </div>

            <p class="text-xs text-gray-600">Tailscale is free for personal use (up to 100 devices). Your data goes directly between your devices — Tailscale never sees your traffic.</p>
          </div>
        </div>
      </div>

      <div class="p-4 rounded-lg border border-gray-700 mb-4">
        <div class="mb-3">
          <div class="flex items-center gap-3 mb-2">
            <span class="text-2xl">&#9992;</span>
            <div class="flex items-center gap-2">
              <p class="font-medium">Telegram</p>
              <span class="badge badge-blue">Recommended</span>
            </div>
          </div>
          <p class="text-sm text-gray-400">Chat from your phone, get push notifications when tasks complete, and approve or deny AI actions remotely. Works alongside web chat — no Tailscale needed.</p>
        </div>
        <div class="flex items-center justify-between">
          <p class="text-sm text-gray-300">Set up Telegram now?</p>
          <button id="tg-toggle" class="toggle bg-gray-600" onclick="toggleTelegram()">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div id="tg-config" class="hidden space-y-3 mt-3 pt-3 border-t border-gray-700">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-2">
            <p class="text-gray-300">Create a Telegram bot in 3 steps:</p>
            <p class="text-gray-400">1. <a href="https://t.me/BotFather" target="_blank" class="text-oc-500 hover:text-oc-400 font-medium">Open @BotFather in Telegram &#8599;</a></p>
            <p class="text-gray-400">2. Send <code class="bg-gray-800 px-1.5 py-0.5 rounded">/newbot</code> and follow the prompts</p>
            <p class="text-gray-400">3. Copy the bot token and paste it below</p>
          </div>
          <div>
            <label class="label">Bot Token</label>
            <input type="text" id="tg-token" class="input" placeholder="123456789:ABCdef...">
          </div>
          <div class="flex items-center gap-3">
            <button class="btn-secondary text-sm" onclick="testTelegram()">Verify Bot</button>
            <div id="tg-result" class="hidden text-sm"></div>
          </div>
        </div>
      </div>
      <p class="text-xs text-gray-600 mt-3 text-center">Don't want to set these up right now? Click Next — you can configure both in Settings anytime.</p>
    </div>
    <div class="flex justify-between mt-6">
      <button class="btn-secondary" onclick="goStep(3)">Back</button>
      <button class="btn-primary" onclick="goStep(5)">Next</button>
    </div>
  </div>

  <!-- Step 5: Modules -->
  <div class="step fade-in" id="step-5">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Choose capabilities</h2>
      <p class="text-gray-400 mb-6">Enable what you need. Modules that require external accounts show setup instructions when enabled. You can change all of this later from the dashboard.</p>
    </div>

    <!-- Always-on modules (no toggle) -->
    <div class="card mt-3">
      <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Always Available</p>
      <div class="space-y-2 text-sm">
        <div class="flex justify-between"><span class="text-gray-300">Files &amp; Memory</span><span class="text-gray-500">Read, write, search your files</span></div>
        <div class="flex justify-between"><span class="text-gray-300">Shell &amp; Code</span><span class="text-gray-500">Run commands, execute code</span></div>
        <div class="flex justify-between"><span class="text-gray-300">Web Search</span><span class="text-gray-500">Search the internet, fetch pages</span></div>
      </div>
    </div>

    <!-- Configurable modules -->
    <div class="space-y-3 mt-3" id="module-cards">

      <!-- Email -->
      <div class="card" id="mod-card-email">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#9993;</span>
            <div>
              <p class="font-medium">Email</p>
              <p class="text-sm text-gray-400">Read and send emails through your AI. Ask it to check your inbox, draft replies, or send messages on your behalf.</p>
            </div>
          </div>
          <button class="toggle bg-oc-600" onclick="toggleMod(this,'email')" data-enabled="true">
            <span class="toggle-dot translate-x-6"></span>
          </button>
        </div>
        <div class="mod-config mt-3 pt-3 border-t border-gray-700" id="mod-config-email">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-2">
            <p class="text-gray-300">Connect your email account:</p>
            <p class="text-gray-400">Syntaur reads emails via <strong>IMAP</strong> and sends via <strong>SMTP</strong> — standard protocols supported by all email providers. Your email stays on your machine, never sent through our servers.</p>
            <p class="text-gray-400">Most providers require an <strong>app password</strong> instead of your normal password. This is a separate password just for Syntaur that you can revoke anytime.</p>
          </div>
          <div class="mt-3 space-y-3">
            <div>
              <label class="label">Email address</label>
              <input class="input" id="email-addr" placeholder="you@gmail.com" oninput="detectEmailProvider()">
            </div>
            <div id="email-provider-help" class="hidden p-3 rounded-lg bg-gray-900 text-sm space-y-1">
              <p class="text-gray-300" id="email-provider-name"></p>
              <p class="text-gray-400" id="email-provider-steps"></p>
              <a id="email-provider-link" href="#" target="_blank" class="inline-flex items-center gap-1 text-xs text-oc-500 hover:text-oc-400 font-medium"></a>
            </div>
            <div>
              <label class="label">App password</label>
              <input type="password" class="input" id="email-pass" placeholder="xxxx xxxx xxxx xxxx">
            </div>
          </div>
          <p class="text-xs text-gray-500 mt-2">IMAP/SMTP servers will be auto-detected from your email address. You can also configure this later in the dashboard.</p>
        </div>
      </div>

      <!-- SMS -->
      <div class="card" id="mod-card-sms">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128172;</span>
            <div>
              <p class="font-medium">SMS <span class="badge badge-yellow ml-2">Advanced Setup</span></p>
              <p class="text-sm text-gray-400">Send and receive text messages. Useful for automated verification codes, alerts, or two-factor authentication flows.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'sms')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-sms">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-2">
            <p class="text-gray-400">SMS requires additional setup that involves creating accounts and configuring phone numbers. A guided setup wizard is available in the dashboard after installation.</p>
            <div class="flex flex-wrap gap-2 mt-1">
              <span class="text-xs text-gray-500 px-2 py-1 bg-gray-800 rounded">Google Voice (free, US only)</span>
              <span class="text-xs text-gray-500 px-2 py-1 bg-gray-800 rounded">Twilio (~$1/mo + per-message)</span>
            </div>
            <p class="text-xs text-yellow-400/80 mt-1">&#9733; Full guided setup available in Dashboard &rarr; Modules &rarr; SMS after install</p>
          </div>
        </div>
      </div>

      <!-- CAPTCHA -->
      <div class="card" id="mod-card-captcha">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128274;</span>
            <div>
              <p class="font-medium">CAPTCHA Solving</p>
              <p class="text-sm text-gray-400">Automatically solve CAPTCHAs during web automation. Your AI can sign into websites, fill forms, and complete tasks that are blocked by CAPTCHAs.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'captcha')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-captcha">
          <div class="p-3 rounded-lg bg-gray-900 text-sm">
            <p class="text-gray-400">Uses <strong>2Captcha</strong>, a service that solves CAPTCHAs for ~$3 per 1,000 solves. You load a small balance and it deducts per solve.</p>
            <a href="https://2captcha.com/enterpage" target="_blank" class="inline-flex items-center gap-1 text-xs text-oc-500 hover:text-oc-400 font-medium mt-2">Create 2Captcha Account &#8599;</a>
          </div>
          <div class="mt-3">
            <label class="label">2Captcha API Key</label>
            <input class="input" id="captcha-key" placeholder="Paste your API key from 2captcha.com/setting">
          </div>
        </div>
      </div>

      <!-- Office -->
      <div class="card" id="mod-card-office">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128196;</span>
            <div>
              <p class="font-medium">Office Documents</p>
              <p class="text-sm text-gray-400">Create and edit Excel spreadsheets, Word documents, and PowerPoint presentations. Ask your AI to build reports, invoices, or fill templates.</p>
            </div>
          </div>
          <button class="toggle bg-oc-600" onclick="toggleMod(this,'office')" data-enabled="true">
            <span class="toggle-dot translate-x-6"></span>
          </button>
        </div>
        <div class="mod-config mt-1 text-xs text-gray-500 pl-10" id="mod-config-office">
          No external accounts needed. Works entirely on your machine.
        </div>
      </div>

      <!-- Browser -->
      <div class="card" id="mod-card-browser">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#127760;</span>
            <div>
              <p class="font-medium">Browser Automation</p>
              <p class="text-sm text-gray-400">Your AI can open websites, fill forms, click buttons, take screenshots, and automate web tasks — like having a virtual assistant that can use a browser.</p>
            </div>
          </div>
          <button class="toggle bg-oc-600" onclick="toggleMod(this,'browser')" data-enabled="true">
            <span class="toggle-dot translate-x-6"></span>
          </button>
        </div>
        <div class="mod-config mt-1 text-xs text-gray-500 pl-10" id="mod-config-browser">
          Uses Chromium on your machine. No external accounts needed.
        </div>
      </div>

      <!-- Social Media -->
      <div class="card" id="mod-card-social">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128227;</span>
            <div>
              <p class="font-medium">Social Media <span class="badge badge-yellow ml-2">Advanced Setup</span></p>
              <p class="text-sm text-gray-400">Post to Bluesky, Threads, and YouTube. Your AI can draft content, engage with followers, and manage your social presence on a schedule.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'social')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-social">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-3">
            <p class="text-gray-400">Each platform has different setup requirements. Bluesky is quick, YouTube and Threads need multi-step OAuth setup.</p>

            <!-- Bluesky - simple -->
            <div class="p-2 rounded bg-gray-800">
              <div class="flex items-center justify-between">
                <span class="text-gray-300 text-xs font-medium">Bluesky</span>
                <span class="badge badge-green text-xs">Quick setup</span>
              </div>
              <p class="text-xs text-gray-500 mt-1">Just needs an app password from your Bluesky settings.</p>
              <a href="https://bsky.app/settings/app-passwords" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium">Create App Password &#8599;</a>
            </div>

            <!-- YouTube - complex -->
            <div class="p-2 rounded bg-gray-800">
              <div class="flex items-center justify-between">
                <span class="text-gray-300 text-xs font-medium">YouTube</span>
                <span class="badge badge-yellow text-xs">Multi-step</span>
              </div>
              <p class="text-xs text-gray-500 mt-1">Requires creating a Google Cloud project, enabling the YouTube API, and completing an OAuth flow. ~10 minutes with our guided wizard.</p>
              <p class="text-xs text-yellow-400/80 mt-1">&#9733; Step-by-step wizard in Dashboard &rarr; Modules &rarr; Social &rarr; YouTube</p>
            </div>

            <!-- Threads - complex -->
            <div class="p-2 rounded bg-gray-800">
              <div class="flex items-center justify-between">
                <span class="text-gray-300 text-xs font-medium">Threads / Instagram</span>
                <span class="badge badge-yellow text-xs">Multi-step</span>
              </div>
              <p class="text-xs text-gray-500 mt-1">Requires a Meta developer account, creating an app, and configuring Threads API access. ~15 minutes with our guided wizard.</p>
              <p class="text-xs text-yellow-400/80 mt-1">&#9733; Step-by-step wizard in Dashboard &rarr; Modules &rarr; Social &rarr; Threads</p>
            </div>
          </div>
        </div>
      </div>

      <!-- Smart Home -->
      <div class="card" id="mod-card-home">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#127968;</span>
            <div>
              <p class="font-medium">Smart Home</p>
              <p class="text-sm text-gray-400">Control lights, thermostats, locks, and media through conversation. Works with Home Assistant to manage your entire smart home.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'home')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-home">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-2">
            <p class="text-gray-400">Requires <strong>Home Assistant</strong> running on your network. Syntaur connects to it using a long-lived access token.</p>
            <p class="text-gray-400">Don't have Home Assistant? <a href="https://www.home-assistant.io/installation/" target="_blank" class="text-oc-500 hover:text-oc-400 font-medium">Install it free &#8599;</a></p>
          </div>
          <div class="grid grid-cols-1 gap-3 mt-3">
            <div><label class="label">Home Assistant URL</label><input class="input" id="ha-url" placeholder="http://192.168.1.x:8123"></div>
            <div>
              <label class="label">Long-Lived Access Token</label>
              <input type="password" class="input" id="ha-token" placeholder="Paste token from HA profile page">
              <div class="p-2 mt-1 rounded bg-gray-800 text-xs text-gray-400 space-y-1">
                <p class="text-gray-300 font-medium">How to get this token (1 minute):</p>
                <p>1. Open your Home Assistant dashboard in a browser</p>
                <p>2. Click your profile icon (bottom-left corner)</p>
                <p>3. Scroll to <strong>Long-Lived Access Tokens</strong></p>
                <p>4. Click <strong>Create Token</strong>, name it "Syntaur", copy the token</p>
                <p>5. Paste it here &mdash; the token is only shown once!</p>
                <a href="https://www.home-assistant.io/docs/authentication/#your-account-profile" target="_blank" class="text-oc-500 hover:text-oc-400 font-medium">HA docs: Creating access tokens &#8599;</a>
              </div>
            </div>
            <button class="btn-secondary text-sm w-fit" onclick="testHa()">Test Connection</button>
            <div id="ha-result" class="hidden text-sm"></div>
          </div>
        </div>
      </div>

      <!-- Camera / Security -->
      <div class="card" id="mod-card-camera">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128247;</span>
            <div>
              <p class="font-medium">Security Cameras <span class="badge badge-yellow ml-2">Advanced Setup</span></p>
              <p class="text-sm text-gray-400">View camera feeds, search for events, and get notified about motion or people. Works with Frigate NVR for AI-powered object detection.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'camera')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-camera">
          <div class="p-3 rounded-lg bg-gray-900 text-sm space-y-2">
            <p class="text-gray-400">Requires <strong>Frigate NVR</strong> already running on your network. Frigate is a separate project that connects to your IP cameras and does AI-powered detection (people, cars, animals, license plates).</p>
            <p class="text-gray-400">If you don't have Frigate yet, it needs its own setup (cameras, RTSP streams, a Coral/GPU for detection). Our dashboard has a connection wizard once Frigate is running.</p>
            <a href="https://docs.frigate.video/guides/getting_started/" target="_blank" class="inline-flex items-center gap-1 text-xs text-oc-500 hover:text-oc-400 font-medium">Frigate Getting Started Guide &#8599;</a>
            <p class="text-xs text-yellow-400/80">&#9733; Connection wizard in Dashboard &rarr; Modules &rarr; Cameras after install</p>
          </div>
        </div>
      </div>

      <!-- Finance -->
      <div class="card" id="mod-card-finance">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#128176;</span>
            <div>
              <p class="font-medium">Finance</p>
              <p class="text-sm text-gray-400">Track expenses, manage a ledger, monitor investments, and handle tax records. Connect to your brokerage for automated portfolio tracking.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'finance')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-finance">
          <div class="p-3 rounded-lg bg-gray-900 text-sm">
            <p class="text-gray-400">The ledger works locally with no accounts. For investment tracking, connect your brokerage:</p>
            <div class="flex flex-wrap gap-2 mt-2">
              <a href="https://app.alpaca.markets/brokerage/dashboard/overview" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-800 rounded">Alpaca (stocks) &#8599;</a>
              <a href="https://www.coinbase.com/settings/api" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 font-medium px-2 py-1 bg-gray-800 rounded">Coinbase (crypto) &#8599;</a>
            </div>
          </div>
        </div>
      </div>

      <!-- Voice (if not already enabled in step 3) -->
      <div class="card" id="mod-card-voice">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <span class="text-xl">&#127908;</span>
            <div>
              <p class="font-medium">Voice Assistant</p>
              <p class="text-sm text-gray-400">Talk to your AI out loud with wake word detection, speech-to-text, and natural text-to-speech. Like having your own Alexa that's actually smart.</p>
            </div>
          </div>
          <button class="toggle bg-gray-600" onclick="toggleMod(this,'voice')" data-enabled="false">
            <span class="toggle-dot translate-x-1"></span>
          </button>
        </div>
        <div class="mod-config hidden mt-3 pt-3 border-t border-gray-700" id="mod-config-voice">
          <div class="p-3 rounded-lg bg-gray-900 text-sm">
            <p class="text-gray-400">Voice runs locally on your hardware by default (free, private). For the most natural-sounding voice, you can optionally use ElevenLabs (cloud, paid).</p>
            <a href="https://elevenlabs.io/app/settings/api-keys" target="_blank" class="inline-flex items-center gap-1 text-xs text-oc-500 hover:text-oc-400 font-medium mt-2">ElevenLabs API Keys (optional) &#8599;</a>
          </div>
        </div>
      </div>

    </div>

    <div class="flex justify-between mt-6">
      <button class="btn-secondary" onclick="goStep(4)">Back</button>
      <button class="btn-primary" onclick="goStep(6)">Next</button>
    </div>
  </div>

  <!-- Step 6: Account + Finish -->
  <div class="step fade-in" id="step-6">
    <div class="card">
      <h2 class="text-xl font-semibold mb-2">Secure your installation</h2>
      <p class="text-gray-400 mb-6">Set a password for the dashboard. You'll use this to log in from a browser.</p>
      <div class="space-y-4">
        <div>
          <label class="label">Dashboard password</label>
          <input type="password" id="admin-pass" class="input" placeholder="Choose a strong password">
        </div>
        <div>
          <label class="label">Confirm password</label>
          <input type="password" id="admin-pass-confirm" class="input" placeholder="Type it again">
        </div>
      </div>
    </div>

    <div class="card mt-4">
      <h3 class="font-medium mb-3">Setup Summary</h3>
      <div id="summary" class="space-y-2 text-sm text-gray-400">
        <!-- Populated by JS -->
      </div>
    </div>

    <div class="flex justify-between mt-6">
      <button class="btn-secondary" onclick="goStep(5)">Back</button>
      <button class="btn-primary" onclick="finishSetup()" id="btn-finish">Complete Setup</button>
    </div>
  </div>

  <!-- Step 7: Remote Access -->
  <div class="step fade-in" id="step-7">
    <div class="card">
      <div class="text-center mb-6">
        <div class="text-4xl mb-2">&#10003;</div>
        <h2 class="text-xl font-semibold">Syntaur is running!</h2>
        <p class="text-gray-400 mt-1">Your AI assistant <strong id="done-name" class="text-white"></strong> is ready on this computer.</p>
      </div>

      <div class="p-4 rounded-lg bg-gray-900 mb-6">
        <h3 class="font-medium text-gray-300 mb-3">Access from your phone and other computers</h3>
        <p class="text-sm text-gray-400 mb-4">Syntaur runs on this computer. To use it from your phone, laptop, or tablet, those devices just need to connect to this computer over your network.</p>

        <div class="space-y-3">
          <div class="p-3 rounded-lg bg-gray-800">
            <p class="text-sm font-medium text-gray-300 mb-1">Same Wi-Fi / local network</p>
            <p class="text-xs text-gray-500 mb-2">If your other devices are on the same network, use this address:</p>
            <p class="text-sm text-sky-400 font-mono" id="done-local-url">Detecting...</p>
            <p class="text-xs text-gray-500 mt-1">Open this URL on your phone's browser and tap "Add to Home Screen" for an app-like experience.</p>
          </div>

          <div class="p-3 rounded-lg bg-gray-800" id="done-tailscale-section">
            <p class="text-sm font-medium text-gray-300 mb-1">From anywhere — HTTPS over Tailscale</p>
            <p class="text-xs text-gray-500 mb-2">Syntaur publishes itself on your Tailscale network with a real trusted HTTPS certificate. Works from your phone on cellular, no VPN toggle, no port forwarding, no browser warnings. Auto-rotating keys via your Tailscale OAuth client — paste credentials once, never come back.</p>
            <div id="done-tailscale-status" class="text-xs text-gray-400 mb-2">Checking…</div>
            <div id="done-tailscale-cta">
              <a href="/setup/tailscale" class="btn-primary inline-block text-sm">Enable HTTPS + remote access →</a>
              <p class="text-xs text-gray-500 mt-2">Optional — you can enable it anytime from Settings → Connect.</p>
            </div>
          </div>
        </div>
      </div>

      <p class="text-sm text-gray-500 mt-6" id="done-telegram"></p>

      <div class="flex justify-between mt-6">
        <button class="btn-secondary" onclick="goStep(6)">Back</button>
        <a href="/" class="btn-primary inline-block">Open Dashboard</a>
      </div>

      <div class="mt-6 p-4 rounded-lg bg-gray-900 text-left">
        <p class="text-sm font-medium text-gray-300 mb-2">How to open Syntaur later</p>
        <p class="text-xs text-gray-500">Use the <strong class="text-gray-400">Syntaur</strong> shortcut in your app launcher, or go to:</p>
        <p class="text-xs text-sky-400 mt-1 font-mono">http://localhost:18789</p>
        <p class="text-xs text-gray-500 mt-2">Your conversations follow you across every device — pick up where you left off anywhere.</p>
      </div>
    </div>
  </div>

</div>

<script>
// State
let currentStep = 0;
let installMode = 'server'; // 'server' or 'connect'
let hwScan = null;
let llmChoice = 'cloud';
let voiceEnabled = false;
let telegramEnabled = false;
let modules = [];

// Step 0: Mode selection
function chooseServer() {
  installMode = 'server';
  document.getElementById('progress').classList.remove('hidden');
  goStep(1);
  runScan();
}

function chooseConnect() {
  installMode = 'connect';
  document.getElementById('progress').classList.add('hidden');
  goStep('connect');
}

// Connect mode: test server URL
async function testConnect() {
  const raw = document.getElementById('connect-url').value.trim();
  if (!raw) return;

  const btn = document.getElementById('btn-connect');
  const status = document.getElementById('connect-status');
  btn.disabled = true;
  btn.textContent = 'Connecting...';
  status.classList.remove('hidden');
  status.className = 'p-3 rounded-lg text-sm bg-gray-900 text-gray-400';
  status.textContent = 'Testing connection...';

  // Normalize URL
  let url = raw;
  if (!url.startsWith('http')) url = 'http://' + url;
  if (!url.includes(':')) url += ':18789';

  try {
    const resp = await fetch(url + '/api/setup/status', { mode: 'cors', signal: AbortSignal.timeout(10000) });
    const data = await resp.json();

    if (data.agent_name || data.setup_complete) {
      status.className = 'p-3 rounded-lg text-sm bg-green-900/30 text-green-300';
      status.innerHTML = `Connected to Syntaur server` +
        (data.agent_name ? ` (agent: <strong>${data.agent_name}</strong>)` : '') +
        `. Version ${data.version || 'unknown'}.`;

      // Show success screen
      document.getElementById('connect-done-url').textContent = url.replace('http://', '');
      document.getElementById('connect-done-bookmark').textContent = url;
      document.getElementById('connect-done-link').href = url;

      // Install shortcut pointing to remote URL
      try {
        await fetch('/api/settings/install-shortcut', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ target: 'menu' })
        });
      } catch(e) {}

      setTimeout(() => goStep('connect-done'), 1000);
    } else {
      status.className = 'p-3 rounded-lg text-sm bg-yellow-900/30 text-yellow-300';
      status.textContent = 'Server found but setup is not complete. Complete setup on the server first.';
    }
  } catch(e) {
    status.className = 'p-3 rounded-lg text-sm bg-red-900/30 text-red-300';
    status.innerHTML = `Can't reach <strong>${url}</strong>. Make sure:<br>` +
      '&bull; The server computer is turned on<br>' +
      '&bull; Syntaur is running on it<br>' +
      '&bull; Both devices are on the same network (or connected via Tailscale)<br>' +
      '&bull; The address and port are correct';
  }

  btn.disabled = false;
  btn.textContent = 'Connect';
}

// Step navigation
function goStep(n) {
  const current = document.getElementById(`step-${currentStep}`);
  if (current) current.classList.remove('active');
  const next = document.getElementById(`step-${n}`);
  if (next) next.classList.add('active');
  currentStep = n;
  // Update progress (server mode only, steps 1-6)
  if (typeof n === 'number' && n >= 1) {
    for (let i = 1; i <= 6; i++) {
      const el = document.getElementById(`prog-${i}`);
      if (el) el.className = `h-1 rounded-full flex-1 transition-all ${i <= n ? 'bg-oc-600' : 'bg-gray-700'}`;
    }
  }
  if (n === 4) checkTailscale();
  if (n === 5) loadModules();
  if (n === 6) buildSummary();
  if (n === 7) detectLocalUrl();
}

// Detect local network URL for remote access step
async function detectLocalUrl() {
  try {
    const resp = await fetch('/api/setup/scan');
    const data = await resp.json();
    const ip = data.local_ip || location.hostname;
    const port = location.port || '18789';
    const url = `http://${ip}:${port}`;
    document.getElementById('done-local-url').textContent = url;

    // Check the sidecar-based Tailscale integration (Phase 4.1). Admin-
    // session tokens are stored in sessionStorage after login; during
    // initial setup the /api/setup/tailscale/status call may return 401
    // because the admin isn't logged in yet — fall through to the
    // "not-connected" CTA in that case.
    const tsTok = sessionStorage.getItem('syntaur_token') || '';
    try {
      const tsResp = await fetch('/api/setup/tailscale/status', {
        headers: tsTok ? { 'Authorization': 'Bearer ' + tsTok } : {}
      });
      if (tsResp.ok) {
        const ts = await tsResp.json();
        if (ts.connected && ts.tailnet_url) {
          document.getElementById('done-tailscale-status').innerHTML =
            `<p class="text-sm text-green-400 mb-1">&#10003; Connected to your Tailscale network</p>` +
            `<p class="text-sm text-sky-400 font-mono break-all"><a href="${ts.tailnet_url}" target="_blank">${ts.tailnet_url}</a></p>` +
            `<p class="text-xs text-gray-500 mt-1">Open this URL on any device signed into your Tailscale account. Trusted HTTPS, no port forwarding, rotates automatically.</p>`;
          document.getElementById('done-tailscale-cta').classList.add('hidden');
        } else if (ts.enabled) {
          document.getElementById('done-tailscale-status').innerHTML =
            `<p class="text-xs text-yellow-400">Starting up — give it a minute and refresh.</p>`;
        } else {
          document.getElementById('done-tailscale-status').textContent = '';
        }
      } else {
        document.getElementById('done-tailscale-status').textContent = '';
      }
    } catch(e) {
      document.getElementById('done-tailscale-status').textContent = '';
    }
  } catch(e) {
    document.getElementById('done-local-url').textContent = 'http://' + location.host;
  }
}

// Step 1: Hardware scan
async function runScan() {
  try {
    // Fetch hardware scan + setup status in parallel
    const [scanResp, statusResp] = await Promise.all([
      fetch('/api/setup/scan'),
      fetch('/api/setup/status')
    ]);
    const scan = await scanResp.json();
    const status = await statusResp.json();

    document.getElementById('scan-status').innerHTML = `
      <div class="flex items-center gap-3 text-green-400">
        <span>&#10003;</span> Hardware scan complete
      </div>`;
    document.getElementById('scan-results').classList.remove('hidden');

    // CPU
    document.getElementById('hw-cpu-val').textContent = scan.cpu;

    // RAM
    document.getElementById('hw-ram-val').textContent = `${scan.ram_total_gb} GB (${scan.ram_available_gb} GB free)`;

    // Compute — render local (including Apple / AMD / CPU) then LAN.
    // Kind → badge color mapping keeps the icons meaningful at a glance.
    const kindBadge = (k) => ({
      'nvidia':     '<span class="badge badge-green">NVIDIA</span>',
      'apple':      '<span class="badge badge-blue">Apple Silicon</span>',
      'amd':        '<span class="badge badge-red">AMD</span>',
      'intel-gpu':  '<span class="badge badge-blue">Intel</span>',
      'cpu':        '<span class="badge badge-gray">CPU</span>',
    })[k] || '<span class="badge badge-gray">Unknown</span>';
    const memUnit = (k) => ({ 'VRAM': 'GB VRAM', 'unified': 'GB unified', 'RAM': 'GB RAM' })[k] || 'GB';

    // Local box
    if (scan.gpu_name) {
      document.getElementById('hw-gpu-val').innerHTML =
        `${scan.gpu_name} <span class="text-oc-500">(${scan.gpu_vram_gb} ${memUnit(scan.compute_memory_kind)})</span> ${kindBadge(scan.compute_kind)}`
        + (scan.compute_runtime ? ` <span class="text-xs text-gray-500">via ${scan.compute_runtime}</span>` : '');
      selectLlm('local');
      // AMD on Linux can't use Ollama's default CUDA build. Offer a
      // one-click Vulkan llama.cpp installer (see setup_install.rs);
      // progress shown in-place. Manual link kept as fallback for users
      // who want to pick their own runtime.
      if (scan.compute_kind === 'amd') {
        document.getElementById('llm-local-desc').innerHTML =
          `Run a model on your ${scan.gpu_name} via Vulkan. `
          + `<button type="button" onclick="event.stopPropagation(); startLlamaVulkanInstall()" `
          +   `class="mt-2 text-xs px-3 py-1.5 rounded bg-oc-600 hover:bg-oc-500 text-white">Install automatically</button> `
          + `<span id="llama-vulkan-status" class="text-xs text-gray-400 ml-2"></span>`
          + `<br><a href="https://github.com/ggerganov/llama.cpp/releases" target="_blank" class="text-xs text-gray-500 hover:text-gray-300" onclick="event.stopPropagation()">Or install manually &#8599;</a>`;
      } else {
        document.getElementById('llm-local-desc').textContent = `Run a model on your ${scan.gpu_name}`;
      }
    } else if (scan.network_compute && scan.network_compute.length > 0) {
      // Split probed (real capacity) vs discovered-only (mDNS, no capacity).
      // Render probed first, discovered-only second with the hint line.
      const probed = scan.network_compute.filter(c => c.source !== 'mdns' && c.memory_gb);
      const discovered = scan.network_compute.filter(c => c.source === 'mdns');
      probed.sort((a, b) => parseFloat(b.memory_gb) - parseFloat(a.memory_gb));

      const probedLines = probed.map(c =>
        `${c.name} <span class="text-oc-500">(${c.memory_gb} ${memUnit(c.memory_kind)})</span> ${kindBadge(c.kind)} <span class="badge badge-blue ml-1">on ${c.host}</span>`
      );
      const discoveredLines = discovered.map(c =>
        `<span class="text-gray-300">${c.name}</span> <span class="text-xs text-gray-500">(capacity unknown)</span> ${kindBadge(c.kind)} <span class="badge badge-blue ml-1">on ${c.host}</span>`
        + (c.probe_hint ? `<br><span class="text-xs text-gray-500 pl-4">↳ ${c.probe_hint}</span>` : '')
      );

      document.getElementById('hw-gpu-val').innerHTML =
        [...probedLines, ...discoveredLines].join('<br>');
      if (probed.length > 1) {
        document.getElementById('hw-gpu-val').innerHTML +=
          `<br><span class="text-xs text-gray-500 mt-1">${probed.length} probed + ${discovered.length} discovered</span>`;
        document.getElementById('gpu-roles').classList.remove('hidden');
        renderGpuRoles(probed.map(c => ({ name: c.name, vram_gb: c.memory_gb, host: c.host, kind: c.kind })));
      }
      if (probed.length > 0) {
        selectLlm('network');
        const best = probed[0];
        document.getElementById('llm-network-desc').textContent = `Use ${best.name} on ${best.host}`;
      } else {
        selectLlm('cloud');
      }
    } else if (scan.compute_kind === 'cpu' && parseFloat(scan.ram_total_gb) >= 8) {
      // Local CPU-only but enough RAM for tiny-model inference.
      document.getElementById('hw-gpu-val').innerHTML =
        `<span class="text-gray-400">No GPU — using CPU inference</span> ${kindBadge('cpu')}`
        + ` <span class="text-xs text-gray-500">${scan.ram_total_gb} GB RAM available</span>`;
      selectLlm('cloud');
    } else {
      document.getElementById('hw-gpu-val').innerHTML = '<span class="text-gray-500">None detected locally</span>';
      if (scan.gpu_scan_blocked) {
        document.getElementById('gpu-blocked-banner').classList.remove('hidden');
      }
      selectLlm('cloud');
    }

    // Disk
    document.getElementById('hw-disk-val').textContent = `${scan.disk_free_gb} GB free`;

    // Tier badge
    const tierColors = {
      'Powerful': 'badge-green',
      'Capable': 'badge-green',
      'Limited': 'badge-yellow',
      'CPU-only': 'badge-yellow',
      'Minimal': 'badge-red'
    };
    document.getElementById('hw-tier-badge').textContent = scan.tier;
    document.getElementById('hw-tier-badge').className = `badge ${tierColors[scan.tier] || 'badge-gray'}`;

    // LLM recommendation text
    const recs = {
      'Powerful': 'Your GPU is great for local AI. We recommend running a model locally for privacy and speed.',
      'Capable': 'Your GPU can run medium models locally. A cloud fallback is recommended for complex tasks.',
      'Limited': 'Your GPU can run small models. A cloud API will give better results for most tasks.',
      'CPU-only': 'No GPU detected. A cloud API is recommended, with a local model as offline backup.',
      'Minimal': 'Limited hardware. A cloud API (free tier available) is the best option.'
    };
    document.getElementById('llm-recommendation').textContent = recs[scan.tier] || '';

    // Network LLMs
    if (scan.network_llms && scan.network_llms.length > 0) {
      document.getElementById('hw-network').classList.remove('hidden');
      document.getElementById('hw-network-list').innerHTML = scan.network_llms.map(s => {
        const models = s.models.length > 0 ? ` <span class="text-gray-500">(${s.models.join(', ')})</span>` : '';
        return `<p class="text-sm text-gray-300">&bull; ${s.name}${models}</p>`;
      }).join('');
      // If no local GPU but network LLM found, suggest network
      if (!scan.gpu_name) {
        selectLlm('network');
        document.getElementById('llm-network-desc').textContent =
          `Found ${scan.network_llms.length} service(s) on your network`;
      }
    }

    document.getElementById('btn-next-1').disabled = false;

    if (status.agent_name) {
      document.getElementById('agent-name').value = status.agent_name;
    }
  } catch(e) {
    document.getElementById('scan-status').innerHTML = `
      <div class="text-yellow-400">Could not auto-detect hardware. You can continue manually.</div>`;
    document.getElementById('btn-next-1').disabled = false;
  }
}

// Firewall fix + rescan
async function enableFirewallAndRescan() {
  const result = document.getElementById('firewall-result');
  result.classList.remove('hidden');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Adding firewall rule and rescanning... this may take a moment.';

  try {
    // Ask the gateway to add a firewall rule and rescan
    const resp = await fetch('/api/setup/fix-firewall', { method: 'POST' });
    const data = await resp.json();

    if (data.success) {
      result.className = 'text-sm text-green-400';
      result.textContent = 'Firewall updated. Rescanning...';

      // Wait a moment then rescan
      await new Promise(r => setTimeout(r, 1000));
      const scanResp = await fetch('/api/setup/scan');
      const scan = await scanResp.json();

      if (scan.network_gpus && scan.network_gpus.length > 0) {
        const best = scan.network_gpus[0];
        document.getElementById('hw-gpu-val').innerHTML = `${best.name} <span class="text-oc-500">(${best.vram_gb} GB VRAM)</span> <span class="badge badge-blue ml-1">on ${best.host}</span>`;
        document.getElementById('hw-tier-badge').textContent = scan.tier;
        document.getElementById('hw-tier-badge').className = 'badge badge-green';
        document.getElementById('gpu-blocked-banner').classList.add('hidden');
        selectLlm('network');
        document.getElementById('llm-network-desc').textContent = `Use ${best.name} on ${best.host}`;
        result.textContent = `Found ${best.name} at ${best.host}!`;
      } else {
        result.className = 'text-sm text-yellow-400';
        result.textContent = 'Rescan complete but no GPUs found. Try entering the IP manually.';
        document.getElementById('gpu-manual-section').classList.remove('hidden');
      }
    } else {
      result.className = 'text-sm text-red-400';
      result.textContent = data.message || 'Could not update firewall. Try entering the IP manually.';
      document.getElementById('gpu-manual-section').classList.remove('hidden');
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Error: ' + e.message;
    document.getElementById('gpu-manual-section').classList.remove('hidden');
  }
}

// GPU Role Assignment.
// `runtimes` limits which compute kinds can claim the role. LLM + embedding
// run on any backend (llama.cpp Vulkan/CUDA/Metal/CPU). TTS (Orpheus) and
// image generation have no working Vulkan path today — they need CUDA
// (NVIDIA) or Metal (Apple). STT (whisper.cpp) works on Vulkan but
// benefits from CUDA/Metal acceleration, so we allow it everywhere but
// note the tradeoff in the UI.
const GPU_ROLES = [
  { id: 'llm', name: 'AI Brain (LLM)', desc: 'Main AI model for chat and reasoning', minVram: 6, recommended: 16, runtimes: ['nvidia','apple','amd','intel-gpu','cpu'] },
  { id: 'stt', name: 'Voice Input (STT)', desc: 'Speech-to-text for voice commands', minVram: 2, recommended: 4, runtimes: ['nvidia','apple','amd','cpu'] },
  { id: 'tts', name: 'Voice Output (TTS)', desc: 'Text-to-speech for AI voice responses', minVram: 3, recommended: 4, runtimes: ['nvidia','apple'] },
  { id: 'imggen', name: 'Image Generation', desc: 'Create images from text descriptions', minVram: 6, recommended: 12, runtimes: ['nvidia','apple'] },
  { id: 'embedding', name: 'Document Search', desc: 'Index and search through your files', minVram: 1, recommended: 2, runtimes: ['nvidia','apple','amd','intel-gpu','cpu'] },
];

let gpuRoleAssignments = {};

function renderGpuRoles(gpus) {
  const list = document.getElementById('gpu-role-list');

  // Auto-assign roles based on VRAM
  const sorted = [...gpus].sort((a, b) => parseFloat(b.vram_gb) - parseFloat(a.vram_gb));

  const memLabel = (g) => ({
    'apple': 'GB unified', 'cpu': 'GB RAM', 'intel-gpu': 'GB VRAM',
  })[g.kind] || 'GB VRAM';
  list.innerHTML = gpus.map((gpu, i) => {
    const vram = parseFloat(gpu.vram_gb);
    // Eligible = has enough memory AND role's runtimes list includes this
    // GPU kind. This is how AMD gets LLM + embedding but not TTS/imggen.
    const eligible = GPU_ROLES.filter(r =>
      vram >= r.minVram && (!r.runtimes || r.runtimes.includes(gpu.kind))
    );
    return `
      <div class="p-3 rounded-lg bg-gray-800">
        <div class="flex items-center justify-between mb-2">
          <div>
            <p class="text-sm font-medium text-gray-300">${gpu.name}</p>
            <p class="text-xs text-gray-500">${gpu.vram_gb} ${memLabel(gpu)}${gpu.host ? ' · ' + gpu.host : ''}</p>
          </div>
        </div>
        <div class="flex flex-wrap gap-1">
          ${eligible.map(role => {
            const isDefault = suggestRole(gpu, gpus, role);
            return `<button onclick="toggleGpuRole('${i}','${role.id}',this)"
              class="text-xs px-2 py-1 rounded-full border transition-colors
              ${isDefault ? 'bg-oc-600/20 border-oc-600 text-oc-400' : 'bg-gray-900 border-gray-700 text-gray-500 hover:border-gray-500'}"
              data-gpu="${i}" data-role="${role.id}" data-active="${isDefault}">
              ${role.name}
            </button>`;
          }).join('')}
        </div>
      </div>`;
  }).join('');

  // Show suggestion
  suggestGpuConfig(gpus);
}

function suggestRole(gpu, allGpus, role) {
  const vram = parseFloat(gpu.vram_gb);
  const sorted = [...allGpus].sort((a, b) => parseFloat(b.vram_gb) - parseFloat(a.vram_gb));
  const isLargest = gpu === sorted[0] || gpu.name === sorted[0].name;
  const isSmallest = gpu === sorted[sorted.length - 1] || gpu.name === sorted[sorted.length - 1].name;

  // Runtime gate first — no point defaulting a Vulkan-only card to TTS.
  if (role.runtimes && !role.runtimes.includes(gpu.kind)) return false;

  if (allGpus.length === 1) {
    // Single GPU — assign all roles it can handle
    return vram >= role.minVram;
  }

  // Multi-GPU: largest gets LLM, smallest gets voice/embedding
  if (role.id === 'llm') return isLargest;
  if (role.id === 'stt' || role.id === 'tts') return !isLargest;
  if (role.id === 'embedding') return !isLargest;
  if (role.id === 'imggen') return isLargest && vram >= 12;
  return false;
}

function toggleGpuRole(gpuIdx, roleId, btn) {
  const active = btn.dataset.active !== 'true';
  btn.dataset.active = active;
  btn.className = `text-xs px-2 py-1 rounded-full border transition-colors ${
    active ? 'bg-oc-600/20 border-oc-600 text-oc-400' : 'bg-gray-900 border-gray-700 text-gray-500 hover:border-gray-500'
  }`;
  // Track assignments
  if (!gpuRoleAssignments[gpuIdx]) gpuRoleAssignments[gpuIdx] = [];
  if (active) {
    gpuRoleAssignments[gpuIdx].push(roleId);
  } else {
    gpuRoleAssignments[gpuIdx] = gpuRoleAssignments[gpuIdx].filter(r => r !== roleId);
  }
}

function suggestGpuConfig(gpus) {
  const el = document.getElementById('gpu-role-suggestion');
  // Radeon hosts: the LLM runs on Vulkan llama.cpp but voice/image roles
  // stay NVIDIA or Apple only, so the role table above already hides
  // them. Surface the *why* here so the choice looks intentional.
  const hasAmd = gpus.some(g => g.kind === 'amd');
  const amdNote = hasAmd
    ? '<br><span class="text-xs text-gray-500">AMD hosts run the LLM via Vulkan llama.cpp. Voice (Orpheus) and image generation have no Vulkan backend today — those roles are hidden on AMD cards and can use a cloud service or a separate NVIDIA/Apple host.</span>'
    : '';
  if (gpus.length === 1) {
    const vram = parseFloat(gpus[0].vram_gb);
    if (vram >= 24) {
      el.innerHTML = '<strong class="text-gray-400">Recommended:</strong> Your GPU has plenty of VRAM — it can handle all AI tasks (chat, voice, images) simultaneously.' + amdNote;
    } else if (vram >= 12) {
      el.innerHTML = '<strong class="text-gray-400">Recommended:</strong> Run your AI model + voice on this GPU. Image generation may need the model to be unloaded temporarily.' + amdNote;
    } else {
      el.innerHTML = '<strong class="text-gray-400">Recommended:</strong> Focus this GPU on the AI model. Use cloud services for voice and images for the best experience.' + amdNote;
    }
  } else {
    const sorted = [...gpus].sort((a, b) => parseFloat(b.vram_gb) - parseFloat(a.vram_gb));
    el.innerHTML = `<strong class="text-gray-400">Recommended:</strong> Use your <strong>${sorted[0].name}</strong> (${sorted[0].vram_gb} GB) for the AI brain, and your <strong>${sorted[sorted.length-1].name}</strong> (${sorted[sorted.length-1].vram_gb} GB) for voice processing and document search. This way both GPUs work in parallel without competing for memory.`;
  }
}

// GPU Guide
async function showGpuGuide() {
  document.getElementById('gpu-guide').classList.remove('hidden');
  // Fetch this machine's SSH public key
  try {
    const resp = await fetch('/api/setup/ssh-pubkey');
    const data = await resp.json();
    if (data.key) {
      document.getElementById('gpu-guide-pubkey').textContent = data.key;
      document.getElementById('gpu-guide-key-linux').textContent = data.key;
      document.getElementById('gpu-guide-key-windows').textContent = data.key;
    } else {
      document.getElementById('gpu-guide-pubkey').textContent = data.error || 'Could not read SSH key. You may need to generate one first.';
    }
  } catch(e) {
    document.getElementById('gpu-guide-pubkey').textContent = 'Error loading key';
  }
}

function copyGpuKey() {
  const key = document.getElementById('gpu-guide-pubkey').textContent;
  navigator.clipboard.writeText(key).then(() => {
    event.target.textContent = 'Copied!';
    setTimeout(() => event.target.textContent = 'Copy Key', 1500);
  });
}

async function testGpuConnection() {
  const ip = document.getElementById('gpu-guide-ip').value.trim();
  const user = document.getElementById('gpu-guide-username').value.trim();
  if (!ip) { alert('Enter the IP address of your GPU computer'); return; }
  if (!user) { alert('Enter your username on the GPU computer'); return; }

  const result = document.getElementById('gpu-guide-result');
  result.classList.remove('hidden');
  result.className = 'text-sm mt-2 text-gray-400';
  result.textContent = 'Connecting and scanning for GPUs...';

  try {
    const resp = await fetch('/api/setup/test-gpu', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ host: ip, username: user })
    });
    const data = await resp.json();
    if (data.gpus && data.gpus.length > 0) {
      result.className = 'text-sm mt-2 text-green-400';
      result.innerHTML = `Found ${data.gpus.length} GPU(s):<br>` +
        data.gpus.map(g => `&bull; <strong>${g.name}</strong> (${g.vram_gb} GB VRAM)`).join('<br>');

      // Update the main GPU display
      const gpuVal = document.getElementById('hw-gpu-val');
      gpuVal.innerHTML = data.gpus.map(g =>
        `${g.name} <span class="text-oc-500">(${g.vram_gb} GB)</span> <span class="badge badge-blue ml-1">on ${ip}</span>`
      ).join('<br>');

      // Update tier
      const totalVram = data.gpus.reduce((s, g) => s + parseFloat(g.vram_gb), 0);
      const tier = totalVram >= 16 ? 'Powerful' : totalVram >= 8 ? 'Capable' : 'Limited';
      document.getElementById('hw-tier-badge').textContent = `${tier} (${data.gpus.length > 1 ? data.gpus.length + ' GPUs, ' : ''}network)`;
      document.getElementById('hw-tier-badge').className = 'badge badge-green';

      document.getElementById('gpu-blocked-banner').classList.add('hidden');
      selectLlm('network');
      document.getElementById('llm-network-desc').textContent = `Use ${data.gpus[0].name} on ${ip}`;
      // Show role assignment
      const gpusWithHost = data.gpus.map(g => ({ ...g, host: ip }));
      document.getElementById('gpu-roles').classList.remove('hidden');
      renderGpuRoles(gpusWithHost);
    } else if (data.connected) {
      result.className = 'text-sm mt-2 text-yellow-400';
      result.textContent = 'Connected successfully but no GPU detected. Syntaur scans for NVIDIA (CUDA), AMD (Vulkan), Apple (Metal), and Intel (OpenVINO). If this computer has a GPU the scan missed, let us know.';
    } else {
      result.className = 'text-sm mt-2 text-red-400';
      result.textContent = data.error || 'Could not connect. Make sure you completed Steps 1 and 2, and that the IP address and username are correct.';
    }
  } catch(e) {
    result.className = 'text-sm mt-2 text-red-400';
    result.textContent = 'Connection error: ' + e.message;
  }
}

// Manual GPU probe
async function probeManualGpu() {
  const ip = document.getElementById('gpu-manual-ip').value.trim();
  if (!ip) return;
  const result = document.getElementById('gpu-manual-result');
  result.classList.remove('hidden');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Scanning ' + ip + '...';

  try {
    // Try to find an LLM service on that host
    const resp = await fetch('/api/setup/test-llm', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ base_url: `http://${ip}:1235/v1` })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm text-green-400';
      const models = data.models.length > 0 ? ` (${data.models.join(', ')})` : '';
      result.textContent = `Found LLM service at ${ip}:1235${models}`;
      document.getElementById('hw-gpu-val').innerHTML = `GPU at <span class="text-oc-500">${ip}</span> — running${models}`;
      selectLlm('network');
      document.getElementById('llm-network-desc').textContent = `Use LLM service at ${ip}`;
    } else {
      result.className = 'text-sm text-yellow-400';
      result.textContent = 'No LLM service found on port 1235. Try a different port or check if the service is running.';
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Could not reach ' + ip;
  }
}

// Step 2: LLM selection
function selectLlm(type) {
  llmChoice = type;
  ['local','network','cloud'].forEach(t => {
    const el = document.getElementById(`llm-opt-${t}`);
    el.className = el.className.replace(/border-oc-600|border-gray-700/g, '') +
      (t === type ? ' border-oc-600' : ' border-gray-700');
  });
  document.getElementById('cloud-config').classList.toggle('hidden', type !== 'cloud');
  suggestFallback();
}

function updateCloudProvider() {
  const provider = document.getElementById('cloud-provider').value;
  const link = document.getElementById('provider-signup-link');
  const detail = document.getElementById('provider-help-detail');
  const helpText = document.querySelector('#provider-help .text-gray-300');

  const providers = {
    openrouter: {
      url: 'https://openrouter.ai/settings/keys',
      label: 'Open OpenRouter Keys Page &#8599;',
      intro: 'Get your free API key in under a minute:',
      detail: 'Free tier includes tool-capable models. No credit card required.'
    },
    groq: {
      url: 'https://console.groq.com/keys',
      label: 'Open Groq Console &#8599;',
      intro: 'Sign up (free, no card) and create an API key:',
      detail: 'Runs Llama 3.3 70B at ~250 tok/s on custom LPU hardware. Free tier: ~30 req/min, 14k req/day.'
    },
    cerebras: {
      url: 'https://cloud.cerebras.ai/',
      label: 'Open Cerebras Cloud &#8599;',
      intro: 'Sign up (free, no card) → API Keys → Create:',
      detail: 'Runs Qwen 3 235B MoE at ~2000 tok/s on wafer-scale chips — fastest inference available. Free tier: 1M tokens/day.'
    },
    openai: {
      url: 'https://platform.openai.com/api-keys',
      label: 'Open OpenAI API Keys Page &#8599;',
      intro: 'Create an API key:',
      detail: 'Requires account + payment method. GPT-4o-mini: ~$0.15/1M tokens (~$5-15/mo typical).'
    },
    anthropic: {
      url: 'https://console.anthropic.com/settings/keys',
      label: 'Open Anthropic Console &#8599;',
      intro: 'Create an API key:',
      detail: 'Requires account + payment method. Claude Sonnet: ~$3/1M tokens. Best reasoning quality.'
    }
  };

  const p = providers[provider];
  link.href = p.url;
  link.innerHTML = p.label;
  helpText.textContent = p.intro;
  detail.textContent = p.detail;
}

async function testLlm() {
  const provider = document.getElementById('cloud-provider').value;
  const key = document.getElementById('cloud-key').value;
  const urls = {
    openrouter: 'https://openrouter.ai/api/v1',
    groq: 'https://api.groq.com/openai/v1',
    cerebras: 'https://api.cerebras.ai/v1',
    openai: 'https://api.openai.com/v1',
    anthropic: 'https://api.anthropic.com/v1'
  };
  const result = document.getElementById('llm-test-result');
  result.className = 'text-sm mt-2 text-gray-400';
  result.classList.remove('hidden');
  result.textContent = 'Testing...';

  try {
    const resp = await fetch('/api/setup/test-llm', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ base_url: urls[provider], api_key: key })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm mt-2 text-green-400';
      result.textContent = `Connected! ${data.models.length} models available (${data.latency_ms}ms)`;
    } else {
      result.className = 'text-sm mt-2 text-red-400';
      result.textContent = data.error || 'Connection failed';
    }
  } catch(e) {
    result.className = 'text-sm mt-2 text-red-400';
    result.textContent = 'Network error';
  }
}

// One-click Vulkan llama.cpp install for AMD hosts. See
// setup_install.rs — kicks off background install, polls status, points
// the 'Network LLM' slot at http://127.0.0.1:1235 when ready.
let llamaVulkanPollTimer = null;

async function startLlamaVulkanInstall() {
  const status = document.getElementById('llama-vulkan-status');
  if (!status) return;
  status.className = 'text-xs text-gray-400 ml-2';
  status.textContent = 'Starting...';
  try {
    const resp = await fetch('/api/setup/install-llama-vulkan', {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: '{}'
    });
    if (!resp.ok) {
      const msg = await resp.text();
      status.className = 'text-xs text-red-400 ml-2';
      status.textContent = msg || 'Could not start install';
      return;
    }
    pollLlamaVulkanStatus();
  } catch(e) {
    status.className = 'text-xs text-red-400 ml-2';
    status.textContent = 'Network error starting install';
  }
}

function pollLlamaVulkanStatus() {
  if (llamaVulkanPollTimer) clearInterval(llamaVulkanPollTimer);
  const status = document.getElementById('llama-vulkan-status');
  llamaVulkanPollTimer = setInterval(async () => {
    try {
      const resp = await fetch('/api/setup/install-llama-vulkan/status');
      const data = await resp.json();
      const label = {
        idle: 'Idle',
        fetching_release: 'Fetching llama.cpp release info...',
        downloading: data.mb_total
          ? `Downloading runtime (${data.mb_done}/${data.mb_total} MB)...`
          : `Downloading runtime (${data.mb_done || 0} MB)...`,
        extracting: 'Extracting runtime...',
        writing_service: 'Configuring service...',
        starting_service: 'Starting llama-server...',
        waiting_for_model: 'Downloading model — first run takes 1–3 minutes...',
        done: `Ready at ${data.url}. Use the 'Network LLM' option.`,
        error: `Install failed: ${data.message || 'unknown error'}`,
      }[data.phase] || data.phase;
      if (data.phase === 'done') {
        clearInterval(llamaVulkanPollTimer);
        llamaVulkanPollTimer = null;
        status.className = 'text-xs text-green-400 ml-2';
        status.textContent = label;
        // Point the Network LLM card at the freshly-started server and
        // nudge the wizard over to it.
        const netInput = document.getElementById('llm-opt-network');
        if (netInput) netInput.querySelector('input').checked = true;
        selectLlm('network');
        const desc = document.getElementById('llm-network-desc');
        if (desc) desc.textContent = `Your local Vulkan llama.cpp server at ${data.url}`;
      } else if (data.phase === 'error') {
        clearInterval(llamaVulkanPollTimer);
        llamaVulkanPollTimer = null;
        status.className = 'text-xs text-red-400 ml-2';
        status.textContent = label;
      } else {
        status.className = 'text-xs text-gray-400 ml-2';
        status.textContent = label;
      }
    } catch(e) {
      // Network blip — just keep polling.
    }
  }, 2000);
}

// Step 2b: Fallback provider
function updateFallbackProvider() {
  const provider = document.getElementById('fallback-provider').value;
  const details = document.getElementById('fallback-details');
  const keyRow = document.getElementById('fallback-key-row');
  const helpText = document.getElementById('fallback-help-text');
  const helpLink = document.getElementById('fallback-help-link');
  const skipNote = document.getElementById('fallback-skip-note');

  if (provider === 'none') {
    details.classList.add('hidden');
    skipNote.textContent = 'You can always add a fallback later in Settings.';
    return;
  }

  details.classList.remove('hidden');
  skipNote.textContent = '';

  const providers = {
    openrouter: {
      text: 'Free tier available — no credit card required. Great as a backup.',
      link: 'https://openrouter.ai/settings/keys',
      linkText: 'Get OpenRouter API Key &#8599;',
      showKey: true
    },
    groq: {
      text: 'Free tier, no card. Llama 3.3 70B at ~250 tok/s — very fast as a backup.',
      link: 'https://console.groq.com/keys',
      linkText: 'Get Groq API Key &#8599;',
      showKey: true
    },
    cerebras: {
      text: 'Free tier, no card. Qwen 3 235B at ~2000 tok/s — fastest backup available.',
      link: 'https://cloud.cerebras.ai/',
      linkText: 'Get Cerebras API Key &#8599;',
      showKey: true
    },
    ollama: {
      text: 'Run a small model locally as an offline backup. Install Ollama first.',
      link: 'https://ollama.com/download',
      linkText: 'Download Ollama &#8599;',
      showKey: false
    },
    openai: {
      text: 'Reliable backup. Requires account + payment method.',
      link: 'https://platform.openai.com/api-keys',
      linkText: 'Get OpenAI API Key &#8599;',
      showKey: true
    },
    anthropic: {
      text: 'Best reasoning quality. Requires account + payment method.',
      link: 'https://console.anthropic.com/settings/keys',
      linkText: 'Get Anthropic API Key &#8599;',
      showKey: true
    }
  };

  const p = providers[provider];
  helpText.textContent = p.text;
  helpLink.href = p.link;
  helpLink.innerHTML = p.linkText;
  keyRow.classList.toggle('hidden', !p.showKey);
}

// Auto-suggest a fallback based on primary choice
function suggestFallback() {
  const fb = document.getElementById('fallback-provider');
  if (llmChoice === 'local') {
    fb.value = 'openrouter';
  } else if (llmChoice === 'cloud') {
    fb.value = 'ollama';
  } else {
    fb.value = 'openrouter';
  }
  updateFallbackProvider();
}

// Step 3: Voice toggle
function toggleVoice() {
  voiceEnabled = !voiceEnabled;
  const btn = document.getElementById('voice-toggle');
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${voiceEnabled ? 'bg-oc-600' : 'bg-gray-600'}`;
  dot.className = `toggle-dot ${voiceEnabled ? 'translate-x-6' : 'translate-x-1'}`;
  document.getElementById('voice-options').classList.toggle('hidden', !voiceEnabled);
}

// TTS choice
function updateTtsChoice() {
  const choice = document.getElementById('tts-choice').value;
  const help = document.getElementById('tts-help');
  const elConfig = document.getElementById('tts-elevenlabs-config');

  const helpText = {
    piper: 'Piper runs locally on your CPU with no internet required. Good quality for most uses.',
    orpheus: 'Orpheus runs locally on your GPU for very natural speech. Requires an NVIDIA GPU with 4+ GB VRAM.',
    elevenlabs: 'ElevenLabs provides the most natural voices but requires a paid account and sends audio to the cloud.'
  };

  help.textContent = helpText[choice] || '';
  elConfig.classList.toggle('hidden', choice !== 'elevenlabs');
}

// Tailscale
let tailscaleEnabled = false;

function toggleTailscale() {
  tailscaleEnabled = !tailscaleEnabled;
  const btn = document.getElementById('tailscale-toggle');
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${tailscaleEnabled ? 'bg-oc-600' : 'bg-gray-600'}`;
  dot.className = `toggle-dot ${tailscaleEnabled ? 'translate-x-6' : 'translate-x-1'}`;
  const config = document.getElementById('tailscale-config');
  config.classList.toggle('hidden', !tailscaleEnabled);
  if (tailscaleEnabled) checkTailscale();
}

async function checkTailscale() {
  const status = document.getElementById('tailscale-detect');
  try {
    const resp = await fetch('/api/setup/check-tailscale');
    const data = await resp.json();
    if (data.installed && data.connected) {
      status.innerHTML = `
        <p class="text-green-400 font-medium">&#10003; Tailscale is connected on this machine!</p>
        <p class="text-gray-400 mt-2">Scan this QR code with your phone to open your Syntaur dashboard:</p>
        <div class="flex flex-col sm:flex-row items-center gap-4 mt-3">
          <div class="bg-white p-3 rounded-xl">
            <img src="https://api.qrserver.com/v1/create-qr-code/?size=160x160&data=${encodeURIComponent(data.url)}&bgcolor=ffffff&color=0c4a6e" alt="QR Code" class="w-40 h-40">
          </div>
          <div class="text-left">
            <p class="text-gray-400 text-sm">Or type this URL:</p>
            <p class="text-oc-500 font-mono text-sm mt-1 bg-gray-900 px-3 py-2 rounded-lg select-all">${data.url}</p>
            <div class="mt-3 space-y-2">
              <p class="text-gray-300 text-xs font-medium">Save as an app on your phone:</p>
              <p class="text-gray-500 text-xs"><strong class="text-gray-400">iPhone:</strong> Open in Safari &rarr; tap Share &#8599; &rarr; "Add to Home Screen"</p>
              <p class="text-gray-500 text-xs"><strong class="text-gray-400">Android:</strong> Open in Chrome &rarr; tap &#8942; menu &rarr; "Add to Home screen"</p>
              <p class="text-gray-500 text-xs">This gives you a full-screen app icon — no browser bar, looks like a native app.</p>
            </div>
          </div>
        </div>
        <button onclick="checkTailscale()" class="text-xs text-oc-500 hover:text-oc-400 mt-3">Recheck &#8635;</button>`;
    } else if (data.installed) {
      status.innerHTML = `
        <p class="text-yellow-400 font-medium">Tailscale is installed but not connected yet.</p>
        <p class="text-gray-400 mt-1">Open a terminal on this machine and run:</p>
        <p class="font-mono text-sm bg-gray-900 px-3 py-2 rounded-lg mt-1 text-gray-300">sudo tailscale up</p>
        <p class="text-gray-500 text-xs mt-2">Follow the link it gives you to sign in, then come back here.</p>
        <button onclick="checkTailscale()" class="text-xs text-oc-500 hover:text-oc-400 mt-2">Recheck &#8635;</button>`;
    } else {
      status.innerHTML = `
        <p class="text-yellow-400 font-medium">Tailscale is not installed on this machine yet.</p>
        <p class="text-gray-400 mt-1">Follow the steps above to install it on this machine and your phone/laptop, then come back here.</p>
        <button onclick="checkTailscale()" class="text-xs text-oc-500 hover:text-oc-400 mt-2">Recheck &#8635;</button>`;
    }
  } catch(e) {
    status.innerHTML = '<p class="text-red-400">Could not check Tailscale status.</p>';
  }
}

// Step 4: Telegram
function toggleTelegram() {
  telegramEnabled = !telegramEnabled;
  const btn = document.getElementById('tg-toggle');
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${telegramEnabled ? 'bg-oc-600' : 'bg-gray-600'}`;
  dot.className = `toggle-dot ${telegramEnabled ? 'translate-x-6' : 'translate-x-1'}`;
  document.getElementById('tg-config').classList.toggle('hidden', !telegramEnabled);
}

async function testTelegram() {
  const token = document.getElementById('tg-token').value;
  const result = document.getElementById('tg-result');
  result.classList.remove('hidden');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Verifying...';

  try {
    const resp = await fetch('/api/setup/test-telegram', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ bot_token: token })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm text-green-400';
      result.textContent = `Verified! Bot: @${data.bot_username}`;
    } else {
      result.className = 'text-sm text-red-400';
      result.textContent = data.error || 'Invalid token';
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Network error';
  }
}

// Step 5: Modules
async function loadModules() {
  try {
    const resp = await fetch('/api/setup/modules');
    const data = await resp.json();
    modules = [...data.core_modules, ...data.extension_modules];
    const list = document.getElementById('module-list');
    list.innerHTML = modules.map(m => `
      <div class="flex items-center justify-between p-3 rounded-lg bg-gray-900">
        <div>
          <p class="font-medium text-sm">${m.name}</p>
          <p class="text-xs text-gray-500">${m.description} (${m.tool_count} tools)</p>
        </div>
        <button class="toggle ${m.enabled ? 'bg-oc-600' : 'bg-gray-600'}" onclick="toggleModule(this, '${m.id}')" data-id="${m.id}">
          <span class="toggle-dot ${m.enabled ? 'translate-x-6' : 'translate-x-1'}"></span>
        </button>
      </div>
    `).join('');
  } catch(e) {
    document.getElementById('module-list').innerHTML = '<p class="text-gray-400">Could not load modules</p>';
  }
}

function toggleModule(btn, id) {
  const m = modules.find(x => x.id === id);
  if (m) m.enabled = !m.enabled;
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${m.enabled ? 'bg-oc-600' : 'bg-gray-600'}`;
  dot.className = `toggle-dot ${m.enabled ? 'translate-x-6' : 'translate-x-1'}`;
}

// Module card toggles
function toggleMod(btn, id) {
  const enabled = btn.dataset.enabled !== 'true';
  btn.dataset.enabled = enabled;
  const dot = btn.querySelector('.toggle-dot');
  btn.className = `toggle ${enabled ? 'bg-oc-600' : 'bg-gray-600'}`;
  dot.className = `toggle-dot ${enabled ? 'translate-x-6' : 'translate-x-1'}`;
  const config = document.getElementById(`mod-config-${id}`);
  if (config) {
    // For modules with setup forms, show/hide the config
    const hasForm = config.querySelector('input, select');
    if (hasForm) {
      config.classList.toggle('hidden', !enabled);
    }
  }
}

async function testHa() {
  const url = document.getElementById('ha-url').value;
  const token = document.getElementById('ha-token').value;
  const result = document.getElementById('ha-result');
  result.classList.remove('hidden');
  result.className = 'text-sm text-gray-400';
  result.textContent = 'Testing...';
  try {
    const resp = await fetch('/api/setup/test-ha', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ base_url: url, token: token })
    });
    const data = await resp.json();
    if (data.success) {
      result.className = 'text-sm text-green-400';
      let msg = 'Connected!';
      if (data.version) msg += ` HA v${data.version}`;
      if (data.device_count) msg += ` (${data.device_count} entities)`;
      result.textContent = msg;
    } else {
      result.className = 'text-sm text-red-400';
      result.textContent = data.error || 'Connection failed';
    }
  } catch(e) {
    result.className = 'text-sm text-red-400';
    result.textContent = 'Network error';
  }
}

// Step 2b: Image generation provider (Pollinations default, opt-in local/paid)
let imageProviderChoice = 'pollinations';
function selectImageProvider(which) {
  imageProviderChoice = which;
  for (const t of ['pollinations','local','openrouter']) {
    const el = document.getElementById(`img-opt-${t}`);
    if (!el) continue;
    el.className = el.className.replace(/border-oc-600|border-gray-700/g, '') +
      (t === which ? ' border-oc-600' : ' border-gray-700');
  }
  // Show inline config for local + openrouter, hide others
  const lc = document.getElementById('img-local-config');
  if (lc) lc.classList.toggle('hidden', which !== 'local');
  const oc = document.getElementById('img-openrouter-config');
  if (oc) oc.classList.toggle('hidden', which !== 'openrouter');
}
function buildImageGenPayload() {
  if (imageProviderChoice === 'local') {
    const url = (document.getElementById('img-local-url')?.value || '').trim();
    return url ? { local_sd_url: url } : {}; // empty block -> default Pollinations
  }
  if (imageProviderChoice === 'openrouter') {
    const model = (document.getElementById('img-openrouter-model')?.value || '').trim()
      || 'google/gemini-2.5-flash-image';
    return { openrouter_paid_model: model };
  }
  return {}; // pollinations = zero config, no block needed (default)
}

// Step 6: Summary
function buildSummary() {
  const name = document.getElementById('agent-name').value || 'Claw';
  const user = document.getElementById('user-name').value || 'User';
  const llmLabels = { local: 'Local (Ollama)', network: 'Network LLM', cloud: 'Cloud API' };
  const fbProvider = document.getElementById('fallback-provider').value;
  const fbLabels = { none: 'None', openrouter: 'OpenRouter', groq: 'Groq', cerebras: 'Cerebras', ollama: 'Local (Ollama)', openai: 'OpenAI', anthropic: 'Anthropic' };
  const imgLabels = { pollinations: 'Pollinations (free)', local: 'Local SD (free)', openrouter: 'OpenRouter (paid)' };
  const enabledMods = modules.filter(m => m.enabled).length;

  document.getElementById('summary').innerHTML = `
    <div class="flex justify-between"><span>Assistant</span><span class="text-white">${name}</span></div>
    <div class="flex justify-between"><span>Your name</span><span class="text-white">${user}</span></div>
    <div class="flex justify-between"><span>Primary LLM</span><span class="text-white">${llmLabels[llmChoice]}</span></div>
    <div class="flex justify-between"><span>Fallback LLM</span><span class="text-white">${fbLabels[fbProvider] || 'None'}${fbProvider === 'none' ? ' <span class="text-yellow-500 text-xs">(not recommended)</span>' : ''}</span></div>
    <div class="flex justify-between"><span>Image generation</span><span class="text-white">${imgLabels[imageProviderChoice] || 'Pollinations (free)'}</span></div>
    <div class="flex justify-between"><span>Voice</span><span class="text-white">${voiceEnabled ? 'Enabled' : 'Off'}</span></div>
    <div class="flex justify-between"><span>Telegram</span><span class="text-white">${telegramEnabled ? 'Paired' : 'Off'}</span></div>
    <div class="flex justify-between"><span>Modules</span><span class="text-white">${enabledMods} enabled</span></div>
  `;
}

// Finish
async function finishSetup() {
  const pass = document.getElementById('admin-pass').value;
  const confirm = document.getElementById('admin-pass-confirm').value;
  if (pass !== confirm) { alert('Passwords do not match'); return; }
  if (pass.length < 6) { alert('Password must be at least 6 characters'); return; }

  const name = document.getElementById('agent-name').value || 'Claw';

  document.getElementById('btn-finish').disabled = true;
  document.getElementById('btn-finish').textContent = 'Setting up...';

  // Collect all choices
  const cloudProvider = document.getElementById('cloud-provider')?.value || 'openrouter';
  const cloudKey = document.getElementById('cloud-key')?.value || '';
  const providerModels = {
    openrouter: 'nvidia/nemotron-3-super-120b-a12b:free',
    groq: 'llama-3.3-70b-versatile',
    cerebras: 'qwen-3-235b-a22b-instruct-2507',
    openai: 'gpt-4o-mini',
    anthropic: 'claude-sonnet-4-6'
  };

  const body = {
    agent_name: name,
    user_name: document.getElementById('user-name').value || 'User',
    password: pass,
    llm_primary: llmChoice === 'cloud' ? {
      provider: cloudProvider,
      api_key: cloudKey,
      model: providerModels[cloudProvider] || 'default'
    } : llmChoice === 'local' ? {
      provider: 'ollama',
      base_url: 'http://127.0.0.1:11434/v1'
    } : null,
    llm_fallbacks: buildFallbacks(),
    voice_enabled: voiceEnabled,
    telegram_token: telegramEnabled ? (document.getElementById('tg-token')?.value || null) : null,
    telegram_chat_id: null,
    ha_url: document.querySelector('#ha-url')?.value || null,
    ha_token: document.querySelector('#ha-token')?.value || null,
    disabled_modules: [],
    image_gen: buildImageGenPayload(),
  };

  try {
    const resp = await fetch('/api/setup/apply', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body)
    });
    const data = await resp.json();
    if (!data.success) {
      alert('Setup failed: ' + data.message);
      document.getElementById('btn-finish').disabled = false;
      document.getElementById('btn-finish').textContent = 'Complete Setup';
      return;
    }
  } catch(e) {
    alert('Connection error: ' + e.message);
    document.getElementById('btn-finish').disabled = false;
    document.getElementById('btn-finish').textContent = 'Complete Setup';
    return;
  }

  document.getElementById('done-name').textContent = name;
  if (telegramEnabled) {
    document.getElementById('done-telegram').textContent =
      'Telegram bot is paired — you can chat from your phone!';
  }
  goStep(7);
}

// Email provider detection
function detectEmailProvider() {
  const email = document.getElementById('email-addr').value;
  const domain = email.split('@')[1]?.toLowerCase();
  if (!domain) {
    document.getElementById('email-provider-help').classList.add('hidden');
    return;
  }

  const providers = {
    'gmail.com': {
      name: 'Gmail detected',
      steps: 'You need a Google App Password. This requires 2-Step Verification to be enabled on your Google account.',
      link: 'https://myaccount.google.com/apppasswords',
      linkText: 'Create Gmail App Password &#8599;'
    },
    'googlemail.com': {
      name: 'Gmail detected',
      steps: 'You need a Google App Password. This requires 2-Step Verification to be enabled on your Google account.',
      link: 'https://myaccount.google.com/apppasswords',
      linkText: 'Create Gmail App Password &#8599;'
    },
    'outlook.com': {
      name: 'Outlook detected',
      steps: 'Create an app password in your Microsoft account security settings.',
      link: 'https://account.live.com/proofs/AppPassword',
      linkText: 'Create Outlook App Password &#8599;'
    },
    'hotmail.com': {
      name: 'Outlook/Hotmail detected',
      steps: 'Create an app password in your Microsoft account security settings.',
      link: 'https://account.live.com/proofs/AppPassword',
      linkText: 'Create Outlook App Password &#8599;'
    },
    'live.com': {
      name: 'Outlook/Live detected',
      steps: 'Create an app password in your Microsoft account security settings.',
      link: 'https://account.live.com/proofs/AppPassword',
      linkText: 'Create Outlook App Password &#8599;'
    },
    'yahoo.com': {
      name: 'Yahoo Mail detected',
      steps: 'Generate an app password in your Yahoo account security settings.',
      link: 'https://login.yahoo.com/myaccount/security/app-password',
      linkText: 'Create Yahoo App Password &#8599;'
    },
    'icloud.com': {
      name: 'iCloud Mail detected',
      steps: 'Generate an app-specific password from your Apple ID settings.',
      link: 'https://appleid.apple.com/account/manage/section/security',
      linkText: 'Apple ID Security Settings &#8599;'
    },
    'me.com': {
      name: 'iCloud Mail detected',
      steps: 'Generate an app-specific password from your Apple ID settings.',
      link: 'https://appleid.apple.com/account/manage/section/security',
      linkText: 'Apple ID Security Settings &#8599;'
    },
    'proton.me': {
      name: 'ProtonMail detected',
      steps: 'ProtonMail requires the Proton Mail Bridge app for IMAP access (paid plan only).',
      link: 'https://proton.me/mail/bridge',
      linkText: 'Proton Mail Bridge &#8599;'
    },
    'protonmail.com': {
      name: 'ProtonMail detected',
      steps: 'ProtonMail requires the Proton Mail Bridge app for IMAP access (paid plan only).',
      link: 'https://proton.me/mail/bridge',
      linkText: 'Proton Mail Bridge &#8599;'
    },
  };

  const p = providers[domain];
  const help = document.getElementById('email-provider-help');
  if (p) {
    document.getElementById('email-provider-name').textContent = p.name;
    document.getElementById('email-provider-steps').textContent = p.steps;
    const link = document.getElementById('email-provider-link');
    link.href = p.link;
    link.innerHTML = p.linkText;
    help.classList.remove('hidden');
  } else if (domain.includes('.')) {
    document.getElementById('email-provider-name').textContent = 'Custom email provider';
    document.getElementById('email-provider-steps').textContent = 'Check with your email provider for IMAP/SMTP settings and app password instructions. You may need to contact your IT admin.';
    document.getElementById('email-provider-link').innerHTML = '';
    document.getElementById('email-provider-link').href = '#';
    help.classList.remove('hidden');
  } else {
    help.classList.add('hidden');
  }
}

function buildFallbacks() {
  const fbProvider = document.getElementById('fallback-provider').value;
  if (fbProvider === 'none') return [];
  const fbKey = document.getElementById('fallback-key')?.value || '';
  const models = { openrouter: 'nvidia/llama-3.3-nemotron-super-49b-v1:free', openai: 'gpt-4o-mini', anthropic: 'claude-sonnet-4-6', ollama: 'qwen3:4b' };
  const fb = { provider: fbProvider, model: models[fbProvider] || 'default' };
  if (fbKey) fb.api_key = fbKey;
  if (fbProvider === 'ollama') fb.base_url = 'http://127.0.0.1:11434/v1';
  return [fb];
}

// Init — step 0 shows first, runScan starts when user chooses "server" mode
selectLlm('cloud');
</script>"##;
