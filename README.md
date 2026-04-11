# Syntaur

Your personal AI platform. One binary, runs on your hardware, stays private.

## Quick Start

```bash
# Linux / macOS
curl -sSL https://buddyholly007.github.io/syntaur/install.sh | sh

# Or download directly and run
./syntaur
```

Your browser opens automatically. Follow the setup wizard.

## What You Get

- **AI Chat** — talk to your AI through a polished web interface with markdown, code blocks, and tool visualization
- **88 tools** — web search, email, file management, browser automation, office documents, smart home control, and more
- **Modular** — enable only what you need, install add-on modules
- **Private** — runs 100% on your hardware. Your conversations never leave your network
- **Multiple LLM backends** — local GPU (Ollama), network LLM, or cloud API (OpenRouter free tier, OpenAI, Anthropic)
- **Always-on fallback** — configure 2+ backends so you're never stuck during an outage
- **Voice** — talk to your AI with wake word, speech-to-text, and natural text-to-speech
- **Smart Home** — control lights, thermostats, and more through Home Assistant
- **Telegram** — chat from your phone, get push notifications, approve actions remotely

## System Requirements

| | Minimum | Recommended |
|---|---|---|
| **RAM** | 4 GB | 16 GB |
| **Disk** | 500 MB | 20 GB (with local model) |
| **GPU** | None (use cloud API) | NVIDIA 8+ GB VRAM |
| **OS** | Linux, macOS, Windows | Any |

## How It Works

Syntaur is a single binary (~35MB) that runs as a background service. It serves a web dashboard at `http://localhost:18789` where you can:

1. **Chat** — full-featured conversation with your AI
2. **Manage modules** — enable/disable capabilities
3. **Configure LLM backends** — local, network, or cloud
4. **Monitor** — system status, uptime, tool usage

## LLM Options

| Backend | Privacy | Speed | Cost |
|---|---|---|---|
| **Local GPU** (Ollama) | Full | Fast | Free |
| **Network LLM** | LAN-only | Fast | Free |
| **OpenRouter** | Cloud | Fast | Free tier available |
| **OpenAI** | Cloud | Fast | ~$5-15/mo |
| **Anthropic** | Cloud | Fast | ~$10-30/mo |

The setup wizard auto-detects your hardware and recommends the best configuration with automatic fallbacks.

## License

3-day free trial with full access. License key for continued use.
