//! Browser-based voice UI served at /voice.
//!
//! Three modes:
//! 1. Push-to-talk: click mic button to speak
//! 2. Always-listen: continuous wake word detection ("Hey Syntaur" default)
//! 3. Type: keyboard input
//!
//! Wake word detection uses Web Speech API in continuous mode, filtering
//! for the configured phrase. Turns any browser tab into a smart speaker.

use axum::response::Html;

pub async fn handle_voice_ui() -> Html<&'static str> {
    Html(VOICE_PAGE)
}

const VOICE_PAGE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Syntaur Voice</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
    font-family: system-ui, -apple-system, sans-serif;
    background: #0a0a0a; color: #e5e5e5;
    display: flex; flex-direction: column; align-items: center;
    min-height: 100vh; padding: 20px;
}
h1 { color: #0ea5e9; margin: 30px 0 10px; font-size: 28px; }
.subtitle { color: #666; margin-bottom: 20px; font-size: 14px; }
#chat-container {
    width: 100%; max-width: 600px; flex: 1;
    overflow-y: auto; margin-bottom: 20px;
    display: flex; flex-direction: column; gap: 12px;
}
.msg {
    padding: 12px 16px; border-radius: 12px; max-width: 85%;
    line-height: 1.5; font-size: 15px;
}
.msg.user { background: #1e3a5f; align-self: flex-end; border-bottom-right-radius: 4px; }
.msg.assistant { background: #1a1a2e; align-self: flex-start; border-bottom-left-radius: 4px; }
.msg.system { color: #666; font-size: 13px; align-self: center; }
.settings {
    display: flex; flex-wrap: wrap; gap: 10px; align-items: center;
    margin-bottom: 15px; width: 100%; max-width: 600px;
}
.settings label { color: #888; font-size: 13px; }
.settings select, .settings input[type="text"] {
    background: #111; color: #e5e5e5; border: 1px solid #333;
    padding: 6px 10px; border-radius: 8px; font-size: 13px;
}
.settings input[type="text"] { width: 140px; }
#controls {
    display: flex; gap: 12px; align-items: center; width: 100%; max-width: 600px;
}
#mic-btn {
    width: 64px; height: 64px; border-radius: 50%; border: none;
    background: #0284c7; color: white; font-size: 28px; cursor: pointer;
    transition: all 0.2s; flex-shrink: 0;
}
#mic-btn:hover { background: #0ea5e9; transform: scale(1.05); }
#mic-btn.listening { background: #dc2626; animation: pulse 1.5s infinite; }
#mic-btn.wake-listening { background: #059669; animation: pulse 2s infinite; }
#mic-btn:disabled { background: #333; cursor: not-allowed; }
@keyframes pulse { 0%,100% { box-shadow: 0 0 0 0 rgba(220,38,38,0.4); } 50% { box-shadow: 0 0 0 12px rgba(220,38,38,0); } }
#text-input {
    flex: 1; padding: 12px 16px; border-radius: 24px;
    border: 1px solid #333; background: #111; color: #e5e5e5;
    font-size: 15px; outline: none;
}
#text-input:focus { border-color: #0ea5e9; }
#send-btn {
    padding: 12px 20px; border-radius: 24px; border: none;
    background: #0284c7; color: white; font-size: 15px; cursor: pointer;
}
#send-btn:disabled { background: #333; }
#status { color: #666; font-size: 13px; margin-top: 8px; height: 20px; text-align: center; }
#wake-indicator {
    display: none; position: fixed; top: 20px; right: 20px;
    background: #059669; color: white; padding: 8px 16px;
    border-radius: 20px; font-size: 13px; font-weight: 600;
}
#wake-indicator.active { display: block; }
</style>
</head>
<body>
<h1>Syntaur Voice</h1>
<p class="subtitle">Click the mic, say the wake word, or type</p>

<div class="settings">
    <label>Voice:</label>
    <select id="voice-sel">
        <option value="aria">Aria</option>
        <option value="guy">Guy</option>
        <option value="jenny">Jenny</option>
        <option value="emma">Emma</option>
        <option value="brian">Brian</option>
        <option value="andrew">Andrew</option>
    </select>
    <label>Mode:</label>
    <select id="mode-sel">
        <option value="push">Push to talk</option>
        <option value="wake">Wake word</option>
    </select>
    <label>Wake phrase:</label>
    <input type="text" id="wake-phrase" value="hey syntaur" placeholder="hey syntaur">
</div>

<div id="wake-indicator">Listening for wake word...</div>
<div id="chat-container"></div>

<div id="controls">
    <button id="mic-btn" title="Click to speak">&#x1f3a4;</button>
    <input id="text-input" type="text" placeholder="Or type here..." autocomplete="off">
    <button id="send-btn">Send</button>
</div>
<div id="status"></div>

<script>
const chatEl = document.getElementById('chat-container');
const micBtn = document.getElementById('mic-btn');
const textInput = document.getElementById('text-input');
const sendBtn = document.getElementById('send-btn');
const statusEl = document.getElementById('status');
const voiceSel = document.getElementById('voice-sel');
const modeSel = document.getElementById('mode-sel');
const wakePhraseInput = document.getElementById('wake-phrase');
const wakeIndicator = document.getElementById('wake-indicator');

let conversationId = null;
let isListening = false;
let isProcessing = false;
let wakeWordActive = false;
let wakeRecognition = null;
let commandRecognition = null;

// ── Speech Recognition setup ────────────────────────────────────────
const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;

if (SpeechRecognition) {
    // Recognition for push-to-talk / command capture after wake word
    commandRecognition = new SpeechRecognition();
    commandRecognition.continuous = false;
    commandRecognition.interimResults = true;
    commandRecognition.lang = 'en-US';
    commandRecognition.onresult = (e) => {
        const last = e.results[e.results.length - 1];
        if (last.isFinal) {
            const text = last[0].transcript.trim();
            if (text) sendMessage(text);
            stopCommandListening();
        } else {
            statusEl.textContent = 'Hearing: ' + last[0].transcript;
        }
    };
    commandRecognition.onerror = (e) => {
        if (e.error !== 'aborted' && e.error !== 'no-speech')
            statusEl.textContent = 'Error: ' + e.error;
        stopCommandListening();
    };
    commandRecognition.onend = () => {
        if (isListening) stopCommandListening();
    };

    // Recognition for continuous wake word detection
    wakeRecognition = new SpeechRecognition();
    wakeRecognition.continuous = true;
    wakeRecognition.interimResults = true;
    wakeRecognition.lang = 'en-US';
    wakeRecognition.onresult = (e) => {
        const wakePhrase = wakePhraseInput.value.toLowerCase().trim();
        for (let i = e.resultIndex; i < e.results.length; i++) {
            const transcript = e.results[i][0].transcript.toLowerCase().trim();
            // Check if wake phrase was spoken
            if (transcript.includes(wakePhrase)) {
                // Extract command after wake phrase (if any)
                const afterWake = transcript.split(wakePhrase).pop().trim();
                wakeRecognition.stop();
                wakeIndicator.classList.remove('active');

                if (afterWake && afterWake.length > 3 && e.results[i].isFinal) {
                    // Command included with wake word: "Hey Syntaur, what time is it"
                    addMessage('system', 'Wake word detected');
                    sendMessage(afterWake);
                } else {
                    // Just wake word: start command capture
                    addMessage('system', 'Listening...');
                    startCommandListening();
                }
                return;
            }
        }
    };
    wakeRecognition.onend = () => {
        // Auto-restart wake word listening (browser stops after silence)
        if (wakeWordActive && !isListening && !isProcessing) {
            setTimeout(() => {
                if (wakeWordActive) {
                    try { wakeRecognition.start(); } catch(e) {}
                }
            }, 300);
        }
    };
    wakeRecognition.onerror = (e) => {
        if (e.error === 'not-allowed') {
            statusEl.textContent = 'Microphone permission denied';
            stopWakeWordMode();
        }
    };
} else {
    modeSel.querySelector('option[value="wake"]').disabled = true;
}

// ── Mode switching ──────────────────────────────────────────────────
modeSel.addEventListener('change', () => {
    if (modeSel.value === 'wake') {
        startWakeWordMode();
    } else {
        stopWakeWordMode();
    }
});

function startWakeWordMode() {
    wakeWordActive = true;
    wakeIndicator.classList.add('active');
    wakeIndicator.textContent = 'Listening for "' + wakePhraseInput.value + '"...';
    micBtn.classList.add('wake-listening');
    statusEl.textContent = 'Wake word mode active';
    try { wakeRecognition.start(); } catch(e) {}
}

function stopWakeWordMode() {
    wakeWordActive = false;
    wakeIndicator.classList.remove('active');
    micBtn.classList.remove('wake-listening');
    try { wakeRecognition.stop(); } catch(e) {}
    statusEl.textContent = '';
}

// ── Push-to-talk ────────────────────────────────────────────────────
micBtn.addEventListener('click', () => {
    if (modeSel.value === 'wake') {
        if (wakeWordActive) stopWakeWordMode();
        else startWakeWordMode();
        modeSel.value = wakeWordActive ? 'wake' : 'push';
        return;
    }
    if (isListening) stopCommandListening();
    else startCommandListening();
});

function startCommandListening() {
    isListening = true;
    micBtn.classList.add('listening');
    statusEl.textContent = 'Listening...';
    try { commandRecognition.start(); } catch(e) {}
}

function stopCommandListening() {
    isListening = false;
    micBtn.classList.remove('listening');
    try { commandRecognition.stop(); } catch(e) {}
    // Resume wake word mode if active
    if (wakeWordActive && !isProcessing) {
        setTimeout(() => {
            wakeIndicator.classList.add('active');
            micBtn.classList.add('wake-listening');
            try { wakeRecognition.start(); } catch(e) {}
        }, 1500); // Cooldown to avoid echo
    }
}

// ── Message handling ────────────────────────────────────────────────
function addMessage(role, text) {
    const div = document.createElement('div');
    div.className = 'msg ' + role;
    div.textContent = text;
    chatEl.appendChild(div);
    chatEl.scrollTop = chatEl.scrollHeight;
}

textInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && textInput.value.trim()) {
        sendMessage(textInput.value.trim());
        textInput.value = '';
    }
});
sendBtn.addEventListener('click', () => {
    if (textInput.value.trim()) {
        sendMessage(textInput.value.trim());
        textInput.value = '';
    }
});

async function sendMessage(text) {
    addMessage('user', text);
    statusEl.textContent = 'Thinking...';
    isProcessing = true;
    micBtn.disabled = true;
    sendBtn.disabled = true;

    try {
        const body = { message: text };
        if (conversationId) body.conversation_id = conversationId;

        const resp = await fetch('/api/v1/chat', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        const data = await resp.json();
        conversationId = data.conversation_id || conversationId;
        // Show sub-agent activity if any
        if (data.agents_used && data.agents_used.length > 0) {
            for (const a of data.agents_used) {
                const dur = a.duration_ms > 0 ? ` (${(a.duration_ms/1000).toFixed(1)}s)` : '';
                addMessage('system', `${a.agent}${dur}: ${a.summary}`);
            }
        }
        addMessage('assistant', data.content);
        await speakResponse(data.content);
    } catch (e) {
        addMessage('system', 'Error: ' + e.message);
    }

    isProcessing = false;
    micBtn.disabled = false;
    sendBtn.disabled = false;
    statusEl.textContent = wakeWordActive ? 'Wake word mode active' : '';
}

async function speakResponse(text) {
    if (!text) return;
    statusEl.textContent = 'Speaking...';
    try {
        const resp = await fetch('/api/v1/tts', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ text, voice: voiceSel.value }),
        });
        const data = await resp.json();
        if (data.audio_url) {
            await new Promise((resolve) => {
                const audio = new Audio(data.audio_url);
                audio.onended = resolve;
                audio.onerror = () => { browserSpeak(text).then(resolve); };
                audio.play().catch(() => browserSpeak(text).then(resolve));
            });
        } else {
            await browserSpeak(text);
        }
    } catch (e) {
        await browserSpeak(text);
    }
}

function browserSpeak(text) {
    return new Promise(resolve => {
        if ('speechSynthesis' in window) {
            const utter = new SpeechSynthesisUtterance(text);
            utter.onend = resolve;
            utter.onerror = resolve;
            speechSynthesis.speak(utter);
        } else {
            resolve();
        }
    });
}

addMessage('system', 'Ready. Click mic, switch to wake word mode, or type.');
</script>
</body>
</html>
"##;
