# Syntaur

Your personal AI platform. One binary, runs on your hardware, stays private.

## Quick Start

```bash
# Linux / macOS
curl -sSL https://get.syntaur.dev | sh

# Or download directly and run
./syntaur
```

Your browser opens automatically. Follow the setup wizard.

## What You Get

- **AI Chat** — web interface with markdown, code blocks, and tool visualization
- **100+ built-in tools** — web search, email, file management, browser automation, office documents, smart home, social media, finance, and more
- **Modular** — enable only what you need
- **Private by default** — conversations stay on your network when you use a local or LAN LLM; cloud APIs are optional
- **Multiple LLM backends** — local GPU (CUDA / Metal / Vulkan for AMD), LAN LLM, or cloud API (free tier with OpenRouter / Groq / Cerebras; OpenAI and Anthropic optional)
- **Fallback chain** — configure 2+ backends so an outage doesn't stop you
- **Voice** — wake word + STT + TTS (NVIDIA / Apple supported today; AMD voice backends not yet shipped)
- **Smart Home** — built-in Matter, Kasa, aidot drivers + MQTT dialects; Home Assistant optional
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
| **Local GPU — NVIDIA / Apple** | Full | Free |
| **Local GPU — AMD (Vulkan)** | Full | Free — one-click installer in the setup wizard |
| **Local CPU** | Full | Free (slow but works on any box) |
| **LAN LLM** | LAN-only | Free |
| **OpenRouter / Groq / Cerebras** | Cloud | Free tier available |
| **OpenAI / Anthropic** | Cloud | Pay-per-use (optional) |

The setup wizard auto-detects your hardware and recommends a configuration with automatic fallbacks. A free cloud option is always available — you never need a paid account to use Syntaur.

## License

**Free tier** — AI chat, web search, file management, shell & code, Telegram.

**Pro tier ($49 one-time)** — voice assistant, smart home, email/SMS, office docs, browser automation, social media, finance, security cameras. Perpetual license.
