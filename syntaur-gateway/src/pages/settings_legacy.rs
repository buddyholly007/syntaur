//! Per-tab maud-rendered bodies for the six legacy settings tabs.
//!
//! Generated from `settings_chunks/*.html` via `/tmp/html2maud.py`.
//! The HTML files stay in the tree as the authoritative source so future
//! regeneration is a one-liner; these Rust fns are the runtime render path.
//! All element IDs, classes, and `onclick` handlers are preserved exactly so
//! the legacy `page.js` keeps working unchanged.

use maud::{html, Markup};

pub fn tab_general() -> Markup {
    html! {
        div class="tab-content" id="tab-general" {
            div class="space-y-4" {
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Assistant"
                    }
                    div class="grid grid-cols-2 gap-4" {
                        div {
                            label class="label" {
                                "Agent name"
                            }
                            p class="text-gray-300" id="set-agent-name" {
                                "--"
                            }
                        }
                        div {
                            label class="label" {
                                "Version"
                            }
                            p class="text-gray-300" id="set-version" {
                                "--"
                            }
                        }
                    }
                    h3 class="font-medium mb-3 mt-6" {
                        "Agent Avatars"
                    }
                    p class="text-xs text-gray-500 mb-3" {
                        "Upload a custom image for each agent. It will appear in chat and on the dashboard."
                    }
                    div class="space-y-3" id="avatar-list" {
                        // Populated by JS
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Connections"
                    }
                    div class="space-y-3" {
                        div class="flex items-center justify-between p-3 rounded-lg bg-gray-900" {
                            div {
                                p class="text-sm font-medium" {
                                    "Telegram"
                                }
                                p class="text-xs text-gray-500" id="set-telegram-status" {
                                    "Not configured"
                                }
                            }
                            span class="badge" id="set-telegram-badge" {
                            }
                        }
                        div class="flex items-center justify-between p-3 rounded-lg bg-gray-900" {
                            div {
                                p class="text-sm font-medium" {
                                    "Home Assistant"
                                }
                                p class="text-xs text-gray-500" id="set-ha-status" {
                                    "Not configured"
                                }
                            }
                            span class="badge" id="set-ha-badge" {
                            }
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Data"
                    }
                    div class="grid grid-cols-2 gap-4 text-sm" {
                        div {
                            label class="label" {
                                "Config directory"
                            }
                            p class="text-gray-400 font-mono text-xs" id="set-config-dir" {
                                "~/.syntaur/"
                            }
                        }
                        div {
                            label class="label" {
                                "Gateway port"
                            }
                            p class="text-gray-400" id="set-port" {
                                "18789"
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn tab_llm() -> Markup {
    html! {
        div class="tab-content hidden" id="tab-llm" {
            div class="space-y-4" {
                div class="card" {
                    h3 class="font-medium mb-2" {
                        "AI models Syntaur can use"
                    }
                    p class="text-sm text-gray-400 mb-4" {
                        "Syntaur tries these in order. If the first one is down or busy, it automatically uses the next. You can add more below."
                    }
                    div class="space-y-3" id="llm-providers-list" {
                        p class="text-gray-500" {
                            "Loading..."
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-2" {
                        "Add a model"
                    }
                    p class="text-sm text-gray-400 mb-4" {
                        "Pick the kind that matches your setup. You can mix and match — add a cloud service and a model on your computer, and Syntaur will use both."
                    }
                    div class="space-y-3" {
                        details class="group" {
                            summary class="flex items-center gap-3 p-3 rounded-lg bg-gray-900 hover:bg-gray-800 cursor-pointer transition-colors" {
                                span class="text-lg" {
                                    "☁"
                                }
                                div class="flex-1" {
                                    p class="text-sm font-medium text-white" {
                                        "A cloud service (easiest)"
                                    }
                                    p class="text-xs text-gray-500" {
                                        "OpenRouter, Groq, Cerebras are free with a sign-up. Anthropic and OpenAI are paid but top quality."
                                    }
                                }
                                span class="text-xs text-gray-600 group-open:rotate-90 transition-transform" {
                                    "▶"
                                }
                            }
                            div class="mt-2 ml-9 space-y-3 text-sm" {
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "OpenRouter (easiest, free tier available)"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Sign up at "
                                            a href="https://openrouter.ai" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "openrouter.ai"
                                            }
                                        }
                                        li {
                                            "Go to Keys → Create Key"
                                        }
                                        li {
                                            "Paste below and click Save"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-or-key" placeholder="sk-or-v1-...";
                                        button onclick="saveProvider('openrouter','https://openrouter.ai/api/v1',document.getElementById('setup-or-key').value,'openai-completions','nvidia/nemotron-3-super-120b-a12b:free')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                    div class="mt-2 p-2 bg-gray-800/50 rounded-lg text-[11px] space-y-1" {
                                        p class="text-gray-400" {
                                            "Free model: "
                                            span class="text-gray-300" {
                                                "nvidia/nemotron-3-super-120b-a12b:free"
                                            }
                                            " (262K context)"
                                        }
                                        div class="flex gap-4 text-gray-500" {
                                            div {
                                                p class="text-gray-500 font-medium" {
                                                    "Without credits"
                                                }
                                                p {
                                                    "50 requests/day"
                                                }
                                                p {
                                                    "20 requests/min"
                                                }
                                            }
                                            div {
                                                p class="text-green-500 font-medium" {
                                                    "With $10+ credits"
                                                }
                                                p {
                                                    "1,000 requests/day"
                                                }
                                                p {
                                                    "Dynamic rate limit"
                                                }
                                            }
                                        }
                                        p class="text-gray-600" {
                                            "Adding just $10 in credits unlocks 20x more daily requests — free models still cost $0, the balance just proves you're a real user. "
                                            a href="https://openrouter.ai/credits" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "Add credits →"
                                            }
                                        }
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "Groq (fast free tier, no card)"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Sign up at "
                                            a href="https://console.groq.com/" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "console.groq.com"
                                            }
                                        }
                                        li {
                                            "Go to "
                                            a href="https://console.groq.com/keys" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "API Keys"
                                            }
                                            " → Create API Key"
                                        }
                                        li {
                                            "Paste below and click Save"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-groq-key" placeholder="gsk_...";
                                        button onclick="saveProvider('groq','https://api.groq.com/openai/v1',document.getElementById('setup-groq-key').value,'openai-completions','llama-3.3-70b-versatile')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                    p class="text-[11px] text-gray-500 mt-2" {
                                        "Llama 3.3 70B at ~250 tok/s on custom LPU hardware. Free tier: ~30 req/min, 14,400 req/day. No credit card required."
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "Cerebras (fastest free tier, no card)"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Sign up at "
                                            a href="https://cloud.cerebras.ai/" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "cloud.cerebras.ai"
                                            }
                                        }
                                        li {
                                            "Dashboard → API Keys → Create"
                                        }
                                        li {
                                            "Paste below and click Save"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-cerebras-key" placeholder="csk-...";
                                        button onclick="saveProvider('cerebras','https://api.cerebras.ai/v1',document.getElementById('setup-cerebras-key').value,'openai-completions','qwen-3-235b-a22b-instruct-2507')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                    p class="text-[11px] text-gray-500 mt-2" {
                                        "Qwen 3 235B MoE at ~2000 tok/s on wafer-scale chips — fastest inference available. Free tier: 1M tokens/day. Occasional 'high traffic' queues on free tier are normal — Syntaur auto-skips to the next provider."
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "Anthropic (Claude)"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Get an API key at "
                                            a href="https://console.anthropic.com/settings/keys" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "console.anthropic.com"
                                            }
                                        }
                                        li {
                                            "Paste below and click Save"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-anthropic-key" placeholder="sk-ant-...";
                                        button onclick="saveProvider('anthropic','https://api.anthropic.com',document.getElementById('setup-anthropic-key').value,'anthropic','claude-sonnet-4-6')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "OpenAI"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Get an API key at "
                                            a href="https://platform.openai.com/api-keys" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "platform.openai.com"
                                            }
                                        }
                                        li {
                                            "Paste below and click Save"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-openai-key" placeholder="sk-...";
                                        button onclick="saveProvider('openai','https://api.openai.com/v1',document.getElementById('setup-openai-key').value,'openai-completions','gpt-4o')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                }
                            }
                        }
                        details class="group" {
                            summary class="flex items-center gap-3 p-3 rounded-lg bg-gray-900 hover:bg-gray-800 cursor-pointer transition-colors" {
                                span class="text-lg" {
                                    "💻"
                                }
                                div class="flex-1" {
                                    p class="text-sm font-medium text-white" {
                                        "A model on your own computer (most private)"
                                    }
                                    p class="text-xs text-gray-500" {
                                        "Ollama, LM Studio, or llama.cpp. Nothing you say leaves your computer. Needs a decent graphics card to be fast."
                                    }
                                }
                                span class="text-xs text-gray-600 group-open:rotate-90 transition-transform" {
                                    "▶"
                                }
                            }
                            div class="mt-2 ml-9 space-y-3 text-sm" {
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "Ollama (simplest local option)"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Install from "
                                            a href="https://ollama.ai" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                                "ollama.ai"
                                            }
                                        }
                                        li {
                                            "Run: "
                                            code class="bg-gray-800 px-1 rounded" {
                                                "ollama pull llama3.1"
                                            }
                                        }
                                        li {
                                            "Ollama runs on port 11434 by default"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-ollama-url" placeholder="http://localhost:11434" value="http://localhost:11434";
                                        button onclick="saveProvider('ollama',document.getElementById('setup-ollama-url').value,'','openai-completions','llama3.1')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "LM Studio / llama.cpp"
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Start your server (default port 1234 for LM Studio)"
                                        }
                                        li {
                                            "Enter the URL below"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-local-url" placeholder="http://localhost:1234/v1";
                                        input class="input w-32 text-xs" id="setup-local-model" placeholder="Model name";
                                        button onclick="saveProvider('local',document.getElementById('setup-local-url').value,'','openai-completions',document.getElementById('setup-local-model').value||'default')" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                }
                            }
                        }
                        details class="group" {
                            summary class="flex items-center gap-3 p-3 rounded-lg bg-gray-900 hover:bg-gray-800 cursor-pointer transition-colors" {
                                span class="text-lg" {
                                    "⚡"
                                }
                                div class="flex-1" {
                                    p class="text-sm font-medium text-white" {
                                        "Both — cloud first, your computer as backup"
                                    }
                                    p class="text-xs text-gray-500" {
                                        "Fast cloud for everyday use, falls back to your computer when the cloud is down or you're offline."
                                    }
                                }
                                span class="text-xs text-gray-600 group-open:rotate-90 transition-transform" {
                                    "▶"
                                }
                            }
                            div class="mt-2 ml-9 text-sm" {
                                p class="text-xs text-gray-400 p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    "Add a cloud service at the top, then add a model on your computer below it. Syntaur tries the cloud first. If it's down or your internet is out, your local model takes over — no interruption."
                                }
                            }
                        }
                        details class="group" {
                            summary class="flex items-center gap-3 p-3 rounded-lg bg-gray-900 hover:bg-gray-800 cursor-pointer transition-colors" {
                                span class="text-lg" {
                                    "📷"
                                }
                                div class="flex-1" {
                                    p class="text-sm font-medium text-white" {
                                        "Reading pictures (for receipts, screenshots, docs)"
                                    }
                                    p class="text-xs text-gray-500" {
                                        "A separate model for when Syntaur needs to look at images. Your cloud service can do this, or a model on your computer."
                                    }
                                }
                                span class="text-xs text-gray-600 group-open:rotate-90 transition-transform" {
                                    "▶"
                                }
                            }
                            div class="mt-2 ml-9 space-y-3 text-sm" {
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "On your computer (fastest, most private)"
                                    }
                                    p class="text-xs text-gray-400 mb-2" {
                                        "If you have a decent graphics card (NVIDIA, 8 GB+), Syntaur can read pictures locally. Your documents never leave your computer."
                                    }
                                    ol class="text-xs text-gray-400 space-y-1 list-decimal ml-4" {
                                        li {
                                            "Install "
                                            a href="https://ollama.ai" target="_blank" class="text-oc-500" {
                                                "Ollama"
                                            }
                                            " or "
                                            a href="https://github.com/ggml-org/llama.cpp" target="_blank" class="text-oc-500" {
                                                "llama.cpp"
                                            }
                                        }
                                        li {
                                            "Download a vision model: "
                                            code class="bg-gray-800 px-1 rounded" {
                                                "ollama pull qwen2.5-vl:7b"
                                            }
                                        }
                                        li {
                                            "Enter the endpoint below"
                                        }
                                    }
                                    div class="flex gap-2 mt-2" {
                                        input class="input flex-1 text-xs" id="setup-vision-url" placeholder="http://localhost:11434/v1" value="";
                                        button onclick="saveVisionModel(document.getElementById('setup-vision-url').value)" class="btn-primary text-xs" {
                                            "Save"
                                        }
                                    }
                                    p class="text-[10px] text-gray-600 mt-1" {
                                        "Suggested model: Qwen2.5-VL-7B (about 5 GB). Alternatives: MiniCPM-V, LLaVA."
                                    }
                                }
                                div class="p-3 bg-gray-900/50 rounded-lg border border-gray-800" {
                                    p class="text-gray-300 font-medium mb-2" {
                                        "Use the cloud (no special hardware needed)"
                                    }
                                    p class="text-xs text-gray-400" {
                                        "If you don't set up a picture reader, Syntaur uses the cloud service you added above. OpenRouter's free picture reader works well — nothing else to do."
                                    }
                                }
                            }
                        }
                    }
                    span id="setup-result" class="text-sm mt-2 block" {
                    }
                }
                div class="card" id="gpu-assignment-card" {
                    div class="flex items-center justify-between mb-3" {
                        h3 class="font-medium" {
                            "Share work across your computers"
                        }
                        button onclick="scanGpus()" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1 rounded-lg" id="gpu-scan-btn" {
                            "Find computers"
                        }
                    }
                    p class="text-sm text-gray-400 mb-3" {
                        "If you have more than one computer at home with a graphics card, Syntaur can use them. Give each one a job — chatting, reading pictures, voice, or quick tasks."
                    }
                    div id="gpu-list" class="space-y-2" {
                        p class="text-xs text-gray-600" {
                            "Click \"Find computers\" to look for ones on your home network."
                        }
                    }
                    div id="gpu-assignments" class="hidden mt-4 pt-3 border-t border-gray-700" {
                        p class="text-xs text-gray-500 font-medium mb-2" {
                            "Who does what"
                        }
                        div class="space-y-2 text-sm" {
                            div class="flex items-center justify-between p-2 rounded-lg bg-gray-900" {
                                div {
                                    span class="text-gray-400" {
                                        "Chatting"
                                    }
                                    p class="text-[10px] text-gray-600" {
                                        "The main model your helpers use when you talk to them"
                                    }
                                }
                                select id="assign-chat" class="bg-gray-800 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-300 outline-none w-48" onchange="saveAssignment('primary', this.value)" {
                                    option value="" {
                                        "Cloud only"
                                    }
                                }
                            }
                            div class="flex items-center justify-between p-2 rounded-lg bg-gray-900" {
                                div {
                                    span class="text-gray-400" {
                                        "Reading pictures"
                                    }
                                    p class="text-[10px] text-gray-600" {
                                        "Receipts, screenshots, photos"
                                    }
                                }
                                select id="assign-vision" class="bg-gray-800 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-300 outline-none w-48" onchange="saveAssignment('vision', this.value)" {
                                    option value="" {
                                        "Cloud only"
                                    }
                                }
                            }
                            div class="flex items-center justify-between p-2 rounded-lg bg-gray-900" {
                                div {
                                    span class="text-gray-400" {
                                        "Listening and speaking"
                                    }
                                    p class="text-[10px] text-gray-600" {
                                        "Turning your voice into words, and words back into voice"
                                    }
                                }
                                select id="assign-voice" class="bg-gray-800 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-300 outline-none w-48" onchange="saveAssignment('voice', this.value)" {
                                    option value="" {
                                        "Cloud only"
                                    }
                                }
                            }
                            div class="flex items-center justify-between p-2 rounded-lg bg-gray-900" {
                                div {
                                    span class="text-gray-400" {
                                        "Quick jobs"
                                    }
                                    p class="text-[10px] text-gray-600" {
                                        "Short questions, routing to the right helper"
                                    }
                                }
                                select id="assign-fast" class="bg-gray-800 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-300 outline-none w-48" onchange="saveAssignment('fast', this.value)" {
                                    option value="" {
                                        "Use chat model"
                                    }
                                }
                            }
                        }
                        span id="assign-result" class="text-xs mt-2 block" {
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-2" {
                        "Test a model"
                    }
                    p class="text-sm text-gray-400 mb-4" {
                        "Paste an address and key to check if Syntaur can reach the model."
                    }
                    div class="space-y-3" {
                        div class="grid grid-cols-3 gap-3" {
                            div class="col-span-2" {
                                label class="label" {
                                    "Base URL"
                                }
                                input class="input" id="test-url" placeholder="https://openrouter.ai/api/v1";
                            }
                            div {
                                label class="label" {
                                    "API Key (optional)"
                                }
                                input type="password" class="input" id="test-key" placeholder="sk-...";
                            }
                        }
                        div class="flex items-center gap-3" {
                            button class="btn-primary" onclick="testConnection()" {
                                "Test"
                            }
                            span id="test-result" class="text-sm" {
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn tab_sync() -> Markup {
    html! {
        div class="tab-content hidden" id="tab-sync" {
            div class="space-y-4" {
                // Get Started — primary CTA, simplest path first
                div id="sync-getstarted-card" class="card border-2 border-oc-700 bg-gradient-to-br from-oc-900/30 to-gray-800" {
                    div class="flex items-start gap-4" {
                        div class="text-4xl flex-shrink-0" {
                            "📱"
                        }
                        div class="flex-1 min-w-0" {
                            div class="flex items-center justify-between mb-1" {
                                h3 class="font-semibold text-base" {
                                    "Start here: pair your phone"
                                }
                                span class="badge badge-blue" {
                                    "Recommended first"
                                }
                            }
                            p class="text-sm text-gray-300 mb-3" {
                                "Install the Syntaur Voice PWA on your phone. "
                                strong class="text-white" {
                                    "That's all most people need."
                                }
                                " Once paired, Syntaur can listen to you, send notifications, capture your voice journal, and play music through your phone."
                            }
                            ul class="text-xs text-gray-400 space-y-1 mb-3" {
                                li {
                                    "✓ Voice in / voice out (Syntaur listens and replies on your phone)"
                                }
                                li {
                                    "✓ Music playback through phone speakers, AirPods, or AirPlay"
                                }
                                li {
                                    "✓ Real-time notifications and reminders"
                                }
                                li {
                                    "✓ Voice journal capture from anywhere"
                                }
                            }
                            div class="bg-gray-900/60 border border-gray-700 rounded-lg p-3 mb-4" {
                                p class="text-xs text-gray-300" {
                                    strong class="text-oc-400" {
                                        "✨ Pair once, works anywhere."
                                    }
                                    " Also install "
                                    a href="https://tailscale.com/download" target="_blank" class="text-oc-500 hover:text-oc-400 underline" {
                                        "Tailscale"
                                    }
                                    " on your phone (one-time, free) — the same QR below then works at the coffee shop, on cellular, anywhere you have internet. No extra setup."
                                }
                            }
                            button onclick="openSyncModal('telegram')" class="hidden text-xs bg-oc-600 hover:bg-oc-700 text-white px-4 py-2 rounded-lg font-medium" id="sync-cta-telegram-btn" {
                                "Connect Telegram"
                            }
                            button onclick="window.open('http://' + location.hostname + ':18803', '_blank')" class="bg-oc-600 hover:bg-oc-700 text-white px-4 py-2 rounded-lg text-sm font-medium" {
                                "Pair my phone"
                            }
                            button onclick="openSyncModal('phone_music_pwa')" class="ml-2 text-xs text-gray-400 hover:text-gray-200" {
                                "Already paired? Connect it →"
                            }
                        }
                    }
                }
                div id="sync-getstarted-done" class="hidden card border-green-700/50" {
                    div class="flex items-center gap-3" {
                        span class="text-2xl" {
                            "✓"
                        }
                        div class="flex-1" {
                            p class="text-sm font-medium text-green-400" {
                                "Phone paired"
                            }
                            p class="text-xs text-gray-500" {
                                "You're set. Add more below to expand what Syntaur can do."
                            }
                        }
                        button onclick="document.getElementById('sync-getstarted-card').classList.remove('hidden'); this.parentElement.parentElement.classList.add('hidden');" class="text-xs text-gray-500 hover:text-gray-300" {
                            "Show again"
                        }
                    }
                }
                // Want Syntaur to... progressive disclosure
                div class="card" {
                    div class="flex items-center justify-between mb-1" {
                        h3 class="font-medium" {
                            "Want Syntaur to do more?"
                        }
                        button onclick="loadSyncProviders()" class="text-xs text-gray-400 hover:text-gray-200" {
                            "↺ Refresh"
                        }
                    }
                    p class="text-xs text-gray-500 mb-4" {
                        "Each row shows the simplest path first. Add more for extra capability."
                    }
                    div id="sync-usecases" class="space-y-3" {
                        p class="text-xs text-gray-500 italic" {
                            "Loading…"
                        }
                    }
                }
                // All services — power-user view, collapsed by default
                div class="card" {
                    button onclick="toggleAllServices()" class="w-full flex items-center justify-between text-left" id="sync-all-toggle" {
                        div {
                            h3 class="font-medium text-sm" {
                                "All services"
                            }
                            p class="text-[11px] text-gray-500 mt-0.5" {
                                "Browse the full catalog (29 providers) or jump to a specific service."
                            }
                        }
                        span class="text-gray-500 text-xs" id="sync-all-arrow" {
                            "▸ Show all"
                        }
                    }
                    div id="sync-all-services" class="hidden mt-4" {
                        input type="text" id="sync-filter" placeholder="Filter…" oninput="filterProviders(this.value)" class="w-full mb-4 bg-gray-900 border border-gray-700 rounded-lg text-xs text-gray-300 px-3 py-2 outline-none focus:border-oc-500";
                        div id="sync-categories" class="space-y-5" {
                            p class="text-xs text-gray-500 italic" {
                                "Loading providers…"
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn tab_media() -> Markup {
    html! {
        div class="tab-content hidden" id="tab-media" {
            div class="space-y-4" {
                div class="card" {
                    div class="flex items-center justify-between mb-3" {
                        h3 class="font-medium" {
                            "Syntaur Media Bridge"
                        }
                        span id="mb-status-badge" class="badge badge-red" {
                            "checking…"
                        }
                    }
                    p class="text-sm text-gray-400 mb-3" {
                        " A small companion that runs on your desktop and plays Apple Music, Spotify, Tidal and YouTube Music through your speakers — no popup window, no app switching. It hosts a hidden Chromium that decrypts FairPlay/Widevine exactly as a normal tab would. "
                    }
                    div id="mb-details" class="text-sm text-gray-300 space-y-1 mb-3 hidden" {
                        div {
                            "Version: "
                            code id="mb-version" class="text-oc-500" {
                                "—"
                            }
                        }
                        div {
                            "Audio backend: "
                            code id="mb-backend" class="text-oc-500" {
                                "—"
                            }
                        }
                        div {
                            "Authenticated services: "
                            span id="mb-authed" class="text-gray-400" {
                                "none yet"
                            }
                        }
                    }
                    div id="mb-install" class="hidden text-sm text-gray-300 space-y-2" {
                        p class="text-amber-400" {
                            "No bridge process detected on "
                            code {
                                "127.0.0.1:18790"
                            }
                            "."
                        }
                        p class="font-medium" {
                            "Install on this machine (Linux / macOS):"
                        }
                        pre class="text-xs bg-gray-900 p-3 rounded overflow-x-auto" {
                            code {
                                "scp user@server:/path/syntaur-media-bridge ~/.local/bin/ chmod +x ~/.local/bin/syntaur-media-bridge bash install.sh # systemd-user service / launchd agent"
                            }
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-3" {
                        "Authenticate a service"
                    }
                    p class="text-sm text-gray-400 mb-3" {
                        " One-time login per service. The bridge opens a visible Chromium, you sign in normally, cookies persist so future playback is headless. "
                    }
                    div class="flex flex-wrap gap-2" {
                        button class="btn-secondary" onclick="copyAuthCmd('apple_music')" {
                            "Apple Music — copy command"
                        }
                        button class="btn-secondary" onclick="copyAuthCmd('spotify')" {
                            "Spotify — copy command"
                        }
                        button class="btn-secondary" onclick="copyAuthCmd('tidal')" {
                            "Tidal — copy command"
                        }
                        button class="btn-secondary" onclick="copyAuthCmd('youtube_music')" {
                            "YT Music — copy command"
                        }
                    }
                    p class="text-xs text-gray-500 mt-3" id="mb-copy-hint" {
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-3" {
                        "Quick test"
                    }
                    p class="text-sm text-gray-400 mb-3" {
                        " Open the music page to verify end-to-end playback. "
                    }
                    a href="/music" class="btn-primary inline-block" {
                        "Open /music"
                    }
                }
            }
        }
    }
}

pub fn tab_system() -> Markup {
    html! {
        div class="tab-content hidden" id="tab-system" {
            div class="space-y-4" {
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "System Status"
                    }
                    div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm" id="sys-stats" {
                        div {
                            p class="text-gray-500" {
                                "Uptime"
                            }
                            p class="text-lg font-semibold" id="sys-uptime" {
                                "--"
                            }
                        }
                        div {
                            p class="text-gray-500" {
                                "Agents"
                            }
                            p id="sys-agents" class="text-gray-300" {
                                "--"
                            }
                        }
                        div {
                            p class="text-gray-500" {
                                "Core Modules"
                            }
                            p id="sys-core-mods" class="text-gray-300" {
                                "--"
                            }
                        }
                        div {
                            p class="text-gray-500" {
                                "Extension Modules"
                            }
                            p id="sys-ext-mods" class="text-gray-300" {
                                "--"
                            }
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Developer key"
                    }
                    p class="text-sm text-gray-400 mb-3" {
                        "A long, random password that lets your own scripts and automations talk to Syntaur. If you don't write scripts, you can ignore this."
                    }
                    div class="flex gap-2" {
                        input type="password" class="input font-mono" id="api-token-display" readonly;
                        button class="btn-secondary" onclick="toggleTokenVisibility()" {
                            "Show"
                        }
                        button class="btn-secondary" onclick="copyToken()" {
                            "Copy"
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Your Syntaur license"
                    }
                    div id="license-info" class="mb-4" {
                        p class="text-sm text-gray-400" {
                            "Loading..."
                        }
                    }
                    div {
                        label class="label" {
                            "Paste the activation code"
                        }
                        textarea class="input font-mono text-xs" rows="3" id="license-key-input" placeholder="Paste the activation code you got after buying Syntaur Pro..." {
                        }
                        p class="text-xs text-gray-500 mt-1" {
                            "Copy-paste the whole block. Don't worry if it looks like a long scrambled string — that's normal."
                        }
                        div class="flex items-center gap-3 mt-2" {
                            button class="btn-primary" onclick="activateLicense()" {
                                "Activate"
                            }
                            span id="license-result" class="text-sm" {
                            }
                        }
                    }
                }
                div class="card" {
                    h3 class="font-medium mb-4" {
                        "Updates"
                    }
                    div class="space-y-3 text-sm mb-6" {
                        div class="flex items-center justify-between" {
                            div {
                                p class="text-gray-300" id="update-version-text" {
                                    "Checking for updates..."
                                }
                                p class="text-xs text-gray-500" id="update-status-text" {
                                }
                            }
                            button onclick="checkForUpdates()" class="px-4 py-2 rounded-lg bg-gray-800 hover:bg-gray-700 text-gray-300 text-sm transition-colors" id="btn-check-update" {
                                "Check Now"
                            }
                        }
                        div id="update-available" class="hidden p-3 rounded-lg bg-oc-900/30 border border-oc-800/30" {
                            p class="text-sm text-oc-400 font-medium" {
                                "Update available!"
                            }
                            p class="text-xs text-gray-400 mt-1" id="update-notes" {
                            }
                            a href="" id="update-download-link" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 mt-2 inline-block" {
                                "View release →"
                            }
                        }
                        div id="tax-bracket-status" class="p-3 rounded-lg bg-gray-900" {
                            p class="text-xs text-gray-500" {
                                "Tax brackets: "
                                span id="bracket-status-text" {
                                    "checking..."
                                }
                            }
                        }
                    }
                    h3 class="font-medium mb-4" {
                        "Desktop Shortcut"
                    }
                    div class="space-y-3 text-sm mb-6" {
                        p class="text-gray-400" {
                            "Create or update the Syntaur shortcut in your app launcher, Start Menu, or desktop so you can always find it."
                        }
                        div class="flex gap-3" {
                            button onclick="installShortcut('menu')" class="px-4 py-2 rounded-lg bg-gray-800 hover:bg-gray-700 text-gray-300 text-sm transition-colors" id="btn-shortcut-menu" {
                                " Add to your app menu "
                            }
                            button onclick="installShortcut('desktop')" class="px-4 py-2 rounded-lg bg-gray-800 hover:bg-gray-700 text-gray-300 text-sm transition-colors" id="btn-shortcut-desktop" {
                                " Put on the desktop "
                            }
                        }
                        p class="text-xs text-gray-500" id="shortcut-status" {
                        }
                        div class="p-3 rounded-lg bg-gray-900" {
                            p class="text-xs text-gray-500" {
                                "You can always reach Syntaur at "
                                span class="text-sky-400 font-mono" {
                                    "http://localhost:18789"
                                }
                            }
                            p class="text-xs text-gray-500 mt-1" {
                                "Syntaur runs in the background — the shortcut just opens this dashboard."
                            }
                        }
                    }
                    h3 class="font-medium mb-4" {
                        "Links"
                    }
                    div class="space-y-2 text-sm" {
                        a href="/setup" class="block p-3 rounded-lg bg-gray-900 hover:bg-gray-800 transition-colors" {
                            p class="font-medium text-gray-300" {
                                "Start setup over"
                            }
                            p class="text-xs text-gray-500" {
                                "Walk through the whole setup wizard again"
                            }
                        }
                        a href="/modules" class="block p-3 rounded-lg bg-gray-900 hover:bg-gray-800 transition-colors" {
                            p class="font-medium text-gray-300" {
                                "Modules"
                            }
                            p class="text-xs text-gray-500" {
                                "Turn Syntaur features on and off"
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn tab_users() -> Markup {
    html! {
        div class="tab-content hidden" id="tab-users" {
            div class="space-y-4" {
                div class="bg-gray-900 rounded-xl border border-gray-700 p-6" {
                    div class="flex items-center justify-between mb-4" {
                        h3 class="font-medium" {
                            "User Accounts"
                        }
                        button onclick="showInviteDialog()" class="text-sm bg-oc-600 hover:bg-oc-700 text-white px-4 py-1.5 rounded-lg" {
                            "Invite User"
                        }
                    }
                    div id="users-list" class="space-y-2" {
                        p class="text-sm text-gray-500" {
                            "Loading..."
                        }
                    }
                }
                div class="bg-gray-900 rounded-xl border border-gray-700 p-6" {
                    h3 class="font-medium mb-4" {
                        "Data Sharing"
                    }
                    p class="text-xs text-gray-500 mb-3" {
                        "Controls whether users share data or have separate accounts."
                    }
                    div class="space-y-2" id="sharing-radios" {
                        label class="flex items-center gap-3 p-3 rounded-lg bg-gray-800 cursor-pointer hover:bg-gray-750" {
                            input type="radio" name="sharing" value="shared" onchange="setSharingMode(this.value)" class="accent-sky-500";
                            div {
                                p class="text-sm font-medium text-gray-300" {
                                    "Shared"
                                }
                                p class="text-xs text-gray-500" {
                                    "All users see all data — conversations, knowledge, agents"
                                }
                            }
                        }
                        label class="flex items-center gap-3 p-3 rounded-lg bg-gray-800 cursor-pointer hover:bg-gray-750" {
                            input type="radio" name="sharing" value="isolated" onchange="setSharingMode(this.value)" class="accent-sky-500";
                            div {
                                p class="text-sm font-medium text-gray-300" {
                                    "Isolated"
                                }
                                p class="text-xs text-gray-500" {
                                    "Each user has their own conversations, knowledge, and agents"
                                }
                            }
                        }
                    }
                }
                div class="bg-gray-900 rounded-xl border border-gray-700 p-6" {
                    div class="flex items-center justify-between mb-4" {
                        h3 class="font-medium" {
                            "Pending Invites"
                        }
                    }
                    div id="invites-list" class="space-y-2" {
                        p class="text-sm text-gray-500" {
                            "Loading..."
                        }
                    }
                }
                div class="bg-gray-900 rounded-xl border border-gray-700 p-6" {
                    h3 class="font-medium mb-4" {
                        "Change Your Password"
                    }
                    div class="space-y-3 max-w-sm" {
                        input type="password" id="pw-current" placeholder="Current password (if set)" class="w-full bg-gray-800 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 focus:border-oc-500 outline-none";
                        input type="password" id="pw-new" placeholder="New password (min 8 chars)" class="w-full bg-gray-800 border border-gray-700 rounded-lg px-4 py-2 text-sm text-white placeholder-gray-500 focus:border-oc-500 outline-none";
                        button onclick="changePassword()" class="text-sm bg-gray-700 hover:bg-gray-600 text-white px-4 py-1.5 rounded-lg" id="pw-btn" {
                            "Update Password"
                        }
                        p id="pw-status" class="text-xs hidden" {
                        }
                    }
                }
            }
        }
    }
}
