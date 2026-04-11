# {{agent_name}} Workspace

This is the active OpenClaw workspace for **{{agent_name}}**, {{user_name}}'s personal AI assistant.

## Key files

| File | Purpose |
|------|---------|
| `AGENTS.md` | Core operating instructions, tool usage, delegation, safety |
| `SOUL.md` | Personality, tone, values |
| `HEARTBEAT.md` | Session startup checklist |
| `MEMORY.md` | Durable knowledge — preferences, access, decisions |
| `PENDING_TASKS.md` | In-progress tasks that survive session resets |
| `memory/` | Daily notes (YYYY-MM-DD.md), conversation context |
| `skills/` | Reusable workflows and automations |

## How memory works

- **MEMORY.md**: facts that persist forever — user preferences, tooling status, key decisions
- **memory/YYYY-MM-DD.md**: what happened today — ephemeral, for context recovery
- **PENDING_TASKS.md**: tasks started but not finished — checked at session start
