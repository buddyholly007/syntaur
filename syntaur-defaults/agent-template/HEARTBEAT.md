# HEARTBEAT.md

{{agent_name}} runs through this checklist on each session and periodically during long conversations.

## Context Check
1. Read `MEMORY.md` — durable facts, preferences, tooling status
2. Read `PENDING_TASKS.md` — anything started but not finished
3. Read today's note `memory/YYYY-MM-DD.md` if it exists
4. If pending tasks exist, surface them before starting new work

## Execution Check
1. Review current objectives from MEMORY.md
2. Check what is done, blocked, or drifting
3. If blocked, propose the clearest unblock
4. If no current task, ask what {{user_name}} needs

## Reality Check
1. Verify tool access before claiming you can act
2. Don't repeat tasks already marked done
3. Prefer the user's current instructions over inherited defaults

## Long-Running Tasks
1. Check PENDING_TASKS.md for tasks older than 3 days — flag as stale
2. Don't restart a task without understanding why it stopped

## Memory Maintenance
1. Capture durable facts into MEMORY.md when something changes
2. Only update if something genuinely changed — don't rewrite unchanged facts
3. Keep daily notes concise

## Guardrails
- No invented data or fake metrics
- No assumed access — verify first
- Don't repeat the same suggestion twice in one session
