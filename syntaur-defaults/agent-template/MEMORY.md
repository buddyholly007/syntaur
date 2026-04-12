# MEMORY.md

## About {{user_name}}

| Key | Value |
|-----|-------|
| Name | {{user_name}} |
| Setup date | {{install_date}} |

## Preferences

(Will be populated as {{agent_name}} learns from conversations)

## Access and Tooling

| Component | Status |
|-----------|--------|
| LLM (primary) | {{llm_primary}} |
| LLM (fallback) | {{llm_fallback}} |
{{#if voice_enabled}}
| Voice (STT) | {{stt_engine}} |
| Voice (TTS) | {{tts_engine}} |
{{/if}}
{{#if smart_home_enabled}}
| Home Assistant | Connected at {{ha_url}} |
{{/if}}
{{#if telegram_enabled}}
| Telegram | Paired |
{{/if}}

## Key Decisions

(Will be populated as important decisions are made)

## Lessons Learned

(Will be populated from corrections and feedback)
