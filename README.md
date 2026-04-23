# Syntaur

Your personal AI platform. One binary, runs on your hardware, stays private.

## Quick Start

**Recommended — verified install (sha256 + cosign):**

```bash
# Linux / macOS
wget https://github.com/buddyholly007/syntaur/releases/latest/download/install.sh
sh install.sh --server

# Windows (PowerShell)
iwr https://github.com/buddyholly007/syntaur/releases/latest/download/install.ps1 -OutFile install.ps1
.\install.ps1 --server
```

The installer downloads each binary alongside its `checksums.txt` + cosign signature bundle from the matching release, verifies both, and aborts loudly on any mismatch. See [SECURITY.md](SECURITY.md) for the verification flow.

**Developer-convenience shortcut (unverified — warns loudly):**

```bash
curl -sSL https://github.com/buddyholly007/syntaur/releases/latest/download/install.sh | sh
```

Your browser opens automatically. Follow the setup wizard.

The installer creates a **Syntaur** shortcut in your app launcher (Linux), Applications folder (macOS), or Start Menu and Desktop (Windows).

## What You Get

- **AI Chat** — web interface with markdown, code blocks, and tool visualization
- **100+ built-in tools** — web search, email, file management, browser automation, office documents, smart home control, social media, finance, and more
- **Modular** — enable only what you need
- **Private by default** — your conversations never leave your network when you use a local or LAN LLM. Cloud APIs are optional.
- **Multiple LLM backends** — local GPU (llama.cpp Vulkan for AMD, CUDA for NVIDIA, Metal for Apple, or Ollama), LAN LLM, or cloud API (OpenRouter free tier, Groq, Cerebras, OpenAI, Anthropic)
- **Fallback chain** — configure 2+ backends so an outage doesn't stop you
- **Voice** — wake word, speech-to-text, and text-to-speech (currently NVIDIA/Apple; AMD voice backends not yet shipped)
- **Smart Home** — built-in Matter + Kasa (TP-Link) + aidot (Wi-Fi bulb) drivers; MQTT with Shelly / OpenMQTTGateway / Zigbee2MQTT dialects; Home Assistant is optional, not required
- **Telegram** — chat from your phone, get notifications, approve actions remotely

## System Requirements

| | Minimum | Recommended |
|---|---|---|
| **RAM** | 4 GB | 16 GB |
| **Disk** | 500 MB | 20 GB (with local model) |
| **GPU** | None (use cloud API) | NVIDIA 8+ GB VRAM |
| **OS** | Linux, macOS, Windows | Any |

## How It Works

Syntaur is a single binary (~60 MB release build, ~35 MB stripped) that runs as a background service. It serves a web dashboard at `http://localhost:18789` where you can:

1. **Chat** — full-featured conversation with your AI
2. **Manage modules** — enable/disable capabilities
3. **Configure LLM backends** — local, network, or cloud
4. **Monitor** — system status, uptime, tool usage

## LLM Options

| Backend | Privacy | Cost |
|---|---|---|
| **Local GPU — NVIDIA / Apple** | Full | Free (Ollama, llama.cpp, MLX) |
| **Local GPU — AMD (Vulkan)** | Full | Free (one-click llama.cpp install) |
| **Local CPU** | Full | Free (slow but works on any box) |
| **LAN LLM** (another box on your network) | LAN-only | Free |
| **OpenRouter** | Cloud | Free tier available |
| **Groq / Cerebras** | Cloud | Free tier |
| **OpenAI / Anthropic** | Cloud | Pay-per-use (optional) |

The setup wizard auto-detects your hardware and recommends a configuration with automatic fallbacks. A free cloud API (OpenRouter / Groq / Cerebras) is always an option — you never need to buy credits to use Syntaur.

## License

**Free tier** — AI chat, web search, file management, shell & code, Telegram, community modules.

**Pro tier ($49 one-time)** — voice assistant, smart home, email/SMS, office docs, browser automation, social media, finance, security cameras. Perpetual license — pay once, use forever.

See the [landing page](https://syntaur.dev) for a full feature split.
