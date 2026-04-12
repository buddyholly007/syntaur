# Rust Voice Pipeline Design (Phase 6)

## Goal

Replace HA's assist_pipeline + core_whisper + core_piper + extended_openai_conversation
with a pure-Rust voice pipeline. HA stays as: (1) HomeKit Bridge for iPhone, (2) Matter
bridge for the 53 devices until rs-matter ships CASE initiator, (3) Z-Wave bridge.

## Current Architecture (what we're replacing)

```
Sat1 ESPHome → Wyoming protocol → HA assist_pipeline →
  STT: core_whisper addon (faster-whisper, distil-medium.en, CPU on HA mini-PC)
  LLM: extended_openai_conversation → Syntaur /v1/chat/completions
  TTS: Wyoming fish-audio → rust-llm-proxy → Fish Audio cloud / s2.cpp local
← Wyoming protocol ← Sat1 plays audio
```

## Target Architecture

```
Sat1 ESPHome → Wyoming protocol → rust-voice-pipeline (new Rust binary)
  ├── STT: embedded whisper.cpp via whisper-rs crate (GPU on gaming PC)
  │   or: HTTP call to a whisper.cpp server (like TurboQuant but for whisper)
  ├── Intent: regex first-pass for instant commands (lights, thermostat)
  │   fallback: Syntaur voice_chat HTTP call (LLM + tools)
  ├── TTS: existing rust-llm-proxy Wyoming TTS on port 10400
  │   (already Rust, Fish Audio cloud primary, s2.cpp local fallback)
  └── Audio: Wyoming protocol response back to satellite
```

## Component Design

### 1. rust-voice-pipeline binary

New standalone Rust binary in the Syntaur workspace. Runs on the **gaming PC**
(where the GPU lives for STT + TTS) and listens for Wyoming protocol connections
from the Sat1.

```
[package]
name = "rust-voice-pipeline"
# deps: wyoming-rs (Wyoming protocol), whisper-rs or HTTP to whisper server,
#        reqwest (for Syntaur + TTS calls), tokio, tracing
```

**Wyoming protocol**: The satellite connects via TCP and sends/receives Wyoming
protocol messages (JSON-framed). Key message types:
- `AudioStart` → audio metadata (sample rate, channels)
- `AudioChunk` → PCM audio data
- `AudioStop` → end of utterance
- `Transcript` → STT result
- `Synthesize` → TTS request
- `AudioResponse` → TTS audio back to satellite

The protocol is simple enough to implement directly in Rust without a framework:
each message is a JSON header line + optional binary payload, length-prefixed.

### 2. STT options (pick one)

**Option A: whisper-rs crate (embedded whisper.cpp)**
- Pros: single process, no network hop, GPU via CUDA
- Cons: compiles whisper.cpp from source, CUDA build complexity, shares GPU with TurboQuant
- VRAM: whisper turbo float16 needs ~1.6 GB (fits in the ~3 GB headroom after TurboQuant + s2.cpp)

**Option B: HTTP call to a separate whisper.cpp server**
- Run whisper.cpp's `server` binary (like llama-server but for whisper) on the gaming PC
- The pipeline just POSTs audio to localhost:10301/inference and gets back text
- Pros: separate process, can be restarted independently, proven whisper.cpp server
- Cons: extra service to manage, audio serialization overhead
- This is closer to what HA's core_whisper does (faster-whisper behind Wyoming)

**Option C: faster-whisper via Python subprocess**
- Against the "Rust first" directive. Skip.

**Recommendation**: Option B for phase 1 (proven server, separate process, easy to swap
models), then consider Option A for tighter integration later.

### 3. Intent matching (fast path)

Before calling the LLM, run the user's transcript through a **regex-based intent matcher**
for common commands that don't need LLM reasoning:

```rust
// Intent matching patterns (compiled once at startup)
lazy_static! {
    static ref LIGHT_ON: Regex = Regex::new(r"(?i)turn (?:on|off) (?:the )?(.+?) lights?").unwrap();
    static ref TIMER: Regex = Regex::new(r"(?i)set (?:a )?(\d+) (?:minute|second|hour)s? timer").unwrap();
    // ... ~20 patterns covering the most common voice commands
}
```

If a pattern matches → execute the tool directly (no LLM round-trip, sub-200ms).
If no pattern matches → call Syntaur voice_chat (LLM path, ~2-5s).

This mirrors HA's `prefer_local_intents=True` but in Rust.

### 4. TTS (existing, no new work)

rust-llm-proxy already serves Wyoming TTS on port 10400 with Fish Audio cloud primary
and s2.cpp local fallback. The pipeline just needs to call this endpoint with the
response text and pipe the audio back to the satellite.

### 5. Wake word (stays on satellite)

The Hey Peter microWakeWord model runs on the Sat1's XMOS DSP. No change needed.
When it triggers, the satellite opens a Wyoming connection to the pipeline's TCP port.

## Implementation Plan

### Step 1: Wyoming protocol library (~1 day)
- Implement Wyoming message framing (JSON header + binary payload)
- TCP server that accepts satellite connections
- Types: AudioStart, AudioChunk, AudioStop, Transcript, Synthesize, AudioResponse

### Step 2: STT integration (~1 day)
- Start a whisper.cpp server on gaming PC (systemd service, port 10301)
- Pipeline receives AudioChunk frames, accumulates PCM, POSTs to whisper server
- Parse transcript from JSON response

### Step 3: Intent matching + LLM fallback (~1 day)
- Regex-based fast-path matcher for common commands
- HTTP call to Syntaur voice_chat for everything else
- Response text → TTS → audio back to satellite

### Step 4: TTS response (~half day)
- Call rust-llm-proxy Wyoming TTS endpoint or use direct HTTP
- Pipe TTS audio chunks back to satellite via Wyoming

### Step 5: Integration + testing (~1-2 days)
- Configure Sat1 ESPHome to point at the new pipeline instead of HA
- Test end-to-end: "Hey Peter, turn on the kitchen lights" → sub-500ms response
- Verify TTS plays cleanly on the satellite speaker

### Step 6: Whisper model upgrade (~half day)
- Switch the whisper.cpp server to `large-v3-turbo` model (GPU, float16)
- Set initial_prompt for smart home vocab bias
- Test accuracy on the known problem phrases

## What stays in HA after Phase 6

- HomeKit Bridge (so iPhones see devices in Apple Home app)
- Matter integration (bridge for 53 devices until rs-matter ships CASE)
- Z-Wave JS (for the Trane thermostat)
- Mobile app integration (presence, notifications to iPhones)
- Automations that Sean has already built (don't migrate unless they break)

## What gets removed from HA after Phase 6

- core_whisper addon (STT moves to whisper.cpp on gaming PC GPU)
- extended_openai_conversation (LLM routing moves to the pipeline)
- The assist_pipeline configuration that ties these together
- core_piper addon (if still running — TTS is already via rust-llm-proxy)

## Files to create

```
openclaw-workspace/
  rust-voice-pipeline/       # new crate in the workspace
    Cargo.toml
    src/
      main.rs                # TCP server, orchestrator
      wyoming.rs             # Wyoming protocol impl
      stt.rs                 # Whisper client (HTTP to whisper.cpp server)
      intent.rs              # Regex-based fast-path matcher
      tts.rs                 # TTS client (HTTP to rust-llm-proxy)
      pipeline.rs            # Main pipeline: audio → STT → intent/LLM → TTS → audio
```

## Dependencies

- `tokio` (async runtime, TCP)
- `serde` + `serde_json` (Wyoming protocol)
- `reqwest` (HTTP to whisper server + Syntaur + TTS)
- `tracing` (logging)
- `regex` (intent matching)
- `byteorder` (audio frame handling)

No new heavy deps. No Python. No Node. Pure Rust.

## Estimated effort

~5 days of focused work for a minimal viable pipeline that handles
the happy path. Polish (error recovery, reconnection, multi-satellite
routing, speaker recognition) is ongoing after that.
