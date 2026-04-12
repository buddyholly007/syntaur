# Wake Word Training Guide

Train a custom wake word ("Hey Syntaur", "Hey Jarvis", or any phrase) for hands-free voice activation. This guide covers three approaches from simplest to most advanced.

## Quick Start: Browser Wake Word (Zero Training)

The Syntaur voice UI at `/voice` includes built-in wake word detection using your browser's speech recognition. No training needed.

1. Open `http://your-syntaur:18800/voice`
2. Change Mode to **"Wake word"**
3. Set your wake phrase (default: "hey syntaur")
4. Grant microphone permission
5. Say the phrase — Syntaur activates and listens for your command

**Pros:** Zero setup, works on any device with a browser
**Cons:** Requires browser tab open, uses cloud STT (Google/Apple), slight latency

---

## Option 1: openWakeWord (Best for General Use)

[openWakeWord](https://github.com/dscripka/openWakeWord) runs on any machine with Python. No special hardware. Supports custom wake words with ~50 real samples.

### Requirements
- Python 3.9+
- 2GB RAM
- Microphone (USB, built-in, or network)
- No GPU required

### Install

```bash
pip install openwakeword
# For training:
pip install openwakeword[training]
```

### Pre-trained Models (Use Immediately)

openWakeWord ships with several pre-trained models:
- `hey_jarvis` — high quality, low false positive
- `alexa` — Amazon-style
- `hey_mycroft` — open source assistant
- `timer` / `weather` — command words

```python
from openwakeword.model import Model
import pyaudio, numpy as np

model = Model(wakeword_models=["hey_jarvis"])
mic = pyaudio.PyAudio().open(rate=16000, channels=1, format=pyaudio.paInt16, input=True, frames_per_buffer=1280)

print("Listening for 'Hey Jarvis'...")
while True:
    audio = np.frombuffer(mic.read(1280), dtype=np.int16)
    prediction = model.predict(audio)
    for name, score in prediction.items():
        if score > 0.5:
            print(f"Wake word detected! ({name}: {score:.2f})")
            # Trigger Syntaur voice capture here
```

### Train a Custom Wake Word

**Step 1: Collect positive samples (CRITICAL — most important step)**

You need **at minimum 50 real recordings**, ideally 200+. The single biggest cause of false positives is too few real samples.

```bash
mkdir -p wake_data/positive wake_data/negative
```

Record yourself saying the wake phrase:
```python
import sounddevice as sd
import soundfile as sf

PHRASE = "hey syntaur"
SAMPLE_RATE = 16000

for i in range(100):
    input(f"Press Enter, then say '{PHRASE}' ({i+1}/100)...")
    audio = sd.rec(int(2 * SAMPLE_RATE), samplerate=SAMPLE_RATE, channels=1, dtype='int16')
    sd.wait()
    sf.write(f"wake_data/positive/sample_{i:03d}.wav", audio, SAMPLE_RATE)
    print(f"  Saved sample_{i:03d}.wav")
```

**Data collection best practices (lessons from real deployment):**
- Record from the **actual room positions** where you'll use it (desk, kitchen, couch)
- Include variations: whisper, normal, loud, tired voice, morning voice
- Record with **background noise**: TV on, fan running, typing, dishwasher
- Get **multiple speakers** if others will use it (family members)
- Minimum 50 samples for a basic model, 200+ for production quality
- **88 samples is NOT enough** — this caused persistent false positives in production

**Step 2: Collect negative samples**

Record ambient room noise and similar-sounding phrases that should NOT trigger:
```bash
# Record 5 minutes of room ambient noise
arecord -f S16_LE -r 16000 -c 1 -d 300 wake_data/negative/ambient.wav

# Record confusing phrases
# "hey there", "hey siri", "his encounter", "hey centaur", etc.
```

**Step 3: Generate synthetic augmentation**

```python
# Generate synthetic positive samples with Piper TTS
# This supplements (but NEVER replaces) real recordings
pip install piper-tts

from openwakeword.train import train_model
import piper

# Generate 3000 synthetic samples across different voices
piper_voices = ["en_US-amy-medium", "en_US-danny-low", "en_US-lessac-medium"]
for voice in piper_voices:
    for i in range(1000):
        # Vary speed, pitch slightly for each
        ...
```

**Step 4: Train**

```python
from openwakeword.train import train_model

train_model(
    model_name="hey_syntaur",
    positive_data="wake_data/positive/",
    negative_data="wake_data/negative/",
    # Synthetic data supplements real recordings
    synthetic_positive="wake_data/synthetic/",
    output_dir="wake_data/models/",
    # Training parameters
    epochs=100,
    batch_size=32,
    # Target: <1 false activation per hour (FAPH)
    target_fp_per_hour=0.5,
)
```

**Step 5: Test before deploying**

```python
model = Model(wakeword_models=["wake_data/models/hey_syntaur.onnx"])

# Test with recordings you held out
for wav in held_out_test_files:
    audio = load_audio(wav)
    score = model.predict(audio)
    print(f"{wav}: {score}")

# Test with adversarial audio (TV, music, conversation)
# Should NOT trigger on any of these
```

**Step 6: Deploy with Syntaur**

Run as a sidecar process that POSTs to Syntaur when wake word detected:

```python
#!/usr/bin/env python3
"""Syntaur wake word listener — triggers voice capture on detection."""
import requests, pyaudio, numpy as np
from openwakeword.model import Model

SYNTAUR_URL = "http://localhost:18800"
MODEL_PATH = "wake_data/models/hey_syntaur.onnx"
THRESHOLD = 0.5

model = Model(wakeword_models=[MODEL_PATH])
mic = pyaudio.PyAudio().open(rate=16000, channels=1, format=pyaudio.paInt16,
                              input=True, frames_per_buffer=1280)

print(f"Listening for wake word (threshold={THRESHOLD})...")
while True:
    audio = np.frombuffer(mic.read(1280), dtype=np.int16)
    for name, score in model.predict(audio).items():
        if score > THRESHOLD:
            print(f"Wake word! ({score:.2f}) — capturing command...")
            # Record 5 seconds of audio for the command
            command_audio = mic.read(16000 * 5)
            # Send to Syntaur STT
            resp = requests.post(f"{SYNTAUR_URL}/api/v1/stt",
                                data=command_audio,
                                headers={"Content-Type": "audio/wav"})
            transcript = resp.json().get("text", "")
            if transcript:
                # Send to chat
                chat = requests.post(f"{SYNTAUR_URL}/api/v1/chat",
                                    json={"message": transcript})
                reply = chat.json().get("content", "")
                # Generate speech
                tts = requests.post(f"{SYNTAUR_URL}/api/v1/tts",
                                   json={"text": reply, "voice": "aria"})
                # Play audio...
```

---

## Option 2: microWakeWord (For ESP32/ESPHome Devices)

For users with ESPHome-based smart speakers (ESP32-S3, FutureProof Satellite1, etc.). The model runs directly on the device's DSP chip.

### Requirements
- ESP32-S3 or compatible device with ESPHome
- Python 3.9+, TensorFlow 2.x
- GPU recommended for training (CPU works but slow)

### Training

microWakeWord uses a MixConv architecture optimized for microcontrollers (26K parameters, ~60KB model).

```bash
git clone https://github.com/kahrendt/microWakeWord.git
cd microWakeWord
pip install -r requirements.txt
```

**Prepare training data** (same collection process as openWakeWord above — real samples are critical):

```python
# training_config.yaml
wake_word: "hey syntaur"
model_type: "mixconv_medium"  # 26K params, 61KB .tflite
positive_samples: "./data/positive/"
negative_samples: "./data/negative/"
synthetic_samples: "./data/synthetic/"
epochs: 200
target_false_accepts_per_hour: 0.5
```

```bash
python train.py --config training_config.yaml
```

**Output:** `hey_syntaur.tflite` (~60KB)

### Deploy to ESPHome Device

```yaml
# esphome/satellite.yaml
micro_wake_word:
  models:
    - model: hey_syntaur.tflite
      probability_cutoff: 0.94
      sliding_window_size: 5
  on_wake_word_detected:
    - voice_assistant.start
```

**Critical settings:**
- `tensor_arena_size: 55000` (for MixConv medium; default 22860 is too small)
- Test at "Moderately sensitive" first (cutoff 0.94), only lower if miss rate is high
- **Never deploy with fewer than 100 real training samples** — 88 caused persistent false positive loops in production

### Flash firmware
```bash
esphome run satellite.yaml
# Or OTA:
esphome run satellite.yaml --device 192.168.1.190
```

---

## Option 3: Picovoice Porcupine (Commercial, Cross-Platform)

For users who want a commercial solution with guaranteed quality.

### Features
- Free tier: 3 custom wake words
- Runs on: Linux, macOS, Windows, Raspberry Pi, Android, iOS, ESP32
- Languages: 30+
- Models trained in the cloud via console.picovoice.ai

### Steps

1. Sign up at https://console.picovoice.ai
2. Create a custom wake word
3. Record 3-10 samples in the web console
4. Download the model file (.ppn)
5. Integrate with Syntaur:

```python
import pvporcupine, pvrecorder

porcupine = pvporcupine.create(
    access_key="YOUR_KEY",
    keyword_paths=["hey_syntaur.ppn"]
)
recorder = pvrecorder.PvRecorder(frame_length=porcupine.frame_length)
recorder.start()

while True:
    pcm = recorder.read()
    if porcupine.process(pcm) >= 0:
        print("Wake word detected!")
        # Trigger Syntaur...
```

---

## Troubleshooting False Positives

From real production experience, here's what causes and fixes false activations:

### Common Causes
| Cause | Symptom | Fix |
|---|---|---|
| Too few real samples (<100) | Triggers on ambient noise every 30s | Collect 200+ real samples, retrain |
| Synthetic samples dominate | Model fits TTS artifacts, not real speech | Ratio should be 1:3 real:synthetic max |
| No negative samples | Triggers on TV, music, conversation | Record 30+ min of ambient noise as negatives |
| Sensitivity too high | Triggers on similar-sounding words | Raise probability cutoff (0.94 → 0.97) |
| Echo from speaker | Triggers on own TTS playback | Add cooldown period after TTS (duration + 1s) |

### Echo Mitigation (Smart Speaker Loop Prevention)

When using wake word with a speaker (smart speaker setup), the device's microphone picks up its own TTS output and falsely triggers. Solution:

```
After TTS playback:
  1. Calculate TTS audio duration in seconds
  2. Suppress wake word detection for (duration + 1.5) seconds
  3. Allow ONE follow-up activation (for "and also..." commands)
  4. Block all subsequent activations during cooldown
  5. Filter short filler words ("yeah", "mm", "okay") as echo artifacts
```

### Testing Protocol

Before deploying a custom wake word to production:

1. **True positive test**: Say the wake phrase 20 times from different positions → expect >95% activation
2. **False positive test**: Leave the mic listening for 30 minutes with normal ambient noise → expect 0 false activations
3. **Similar phrase test**: Say 10 similar-sounding phrases → expect 0 activations
4. **Background noise test**: Play music/TV for 10 minutes → expect 0 activations
5. **Multi-speaker test**: Have 3 different people say the phrase → expect >80% activation

---

## Hardware Recommendations

| Setup | Device | Cost | Wake Word Tech |
|---|---|---|---|
| **Browser only** | Any computer | $0 | Web Speech API (built-in) |
| **USB mic + Pi** | Raspberry Pi 4 + USB mic | ~$60 | openWakeWord |
| **Smart speaker** | ESP32-S3 DevKit + speaker + mic | ~$30 | microWakeWord |
| **Commercial** | FutureProof Satellite1 | ~$130 | microWakeWord (XMOS DSP) |
| **Premium** | Any + Picovoice Porcupine | $0-$25/mo | Porcupine cloud-trained |

## Sample Counts Guide

| Samples | Quality | False Positive Rate |
|---|---|---|
| 20-50 | Prototype only | High (~10+ per hour) |
| 50-100 | Basic | Moderate (~2-5 per hour) |
| 100-200 | Good | Low (~0.5-1 per hour) |
| 200-500 | Production | Very low (<0.5 per hour) |
| 500+ | Professional | Negligible (<0.1 per hour) |

**Rule of thumb:** If you're getting false positives, the answer is almost always "more real samples" — not parameter tuning.
