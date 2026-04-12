# Custom Voice Training Guide

Syntaur ships with Edge TTS (Microsoft's neural voices) as the default — zero config, high quality, dozens of voices. For users who want a unique voice identity, this guide covers training and deploying custom voice models.

## Option 1: Piper TTS (Easiest, Local)

Best for: users with a GPU or decent CPU who want fast local TTS with custom voices.

### What you need
- 30-60 minutes of clean speech audio from your target voice
- Python 3.10+, CUDA toolkit (optional, CPU works)
- ~2GB disk for model + training data

### Steps

1. **Prepare audio data**
   - Record or collect clean speech (no background noise, no music)
   - Split into 5-15 second clips
   - Create a metadata CSV: `filename|transcription`

2. **Install Piper training tools**
   ```bash
   pip install piper-tts piper-phonemize
   git clone https://github.com/rhasspy/piper.git
   cd piper/src/python
   pip install -e .
   ```

3. **Preprocess**
   ```bash
   python -m piper_train.preprocess \
     --language en-us \
     --input-dir /path/to/wavs \
     --output-dir /path/to/training \
     --dataset-format ljspeech
   ```

4. **Train** (fine-tune from a base model)
   ```bash
   python -m piper_train \
     --dataset-dir /path/to/training \
     --accelerator gpu \
     --devices 1 \
     --batch-size 32 \
     --validation-split 0.05 \
     --max-epochs 10000 \
     --resume-from-checkpoint /path/to/base-model.ckpt
   ```
   Training typically takes 2-8 hours on a 3090. Monitor loss — stop when validation loss plateaus.

5. **Export to ONNX**
   ```bash
   python -m piper_train.export_onnx \
     /path/to/best-checkpoint.ckpt \
     /path/to/output/model.onnx
   ```

6. **Deploy as Wyoming server**
   ```bash
   pip install wyoming-piper
   wyoming-piper \
     --model /path/to/model.onnx \
     --data-dir /path/to/output \
     --uri tcp://0.0.0.0:10400
   ```

7. **Point Syntaur at it**
   ```bash
   TTS_URL=http://localhost:10400 syntaur
   ```

## Option 2: Orpheus TTS (Best Quality, GPU Required)

Best for: users with an NVIDIA GPU (8GB+ VRAM) who want the most natural-sounding voice.

### What you need
- NVIDIA GPU with 8GB+ VRAM (3060+)
- 1-5 hours of target voice audio
- llama.cpp (for inference)
- ~8GB disk

### Steps

1. **Get the base model**
   - Download orpheus-3b-0.1-ft-Q4_K_M.gguf (~2GB)
   - This is the fine-tuned Orpheus base for voice cloning

2. **Prepare training data**
   - Clean speech audio, 22050Hz mono WAV
   - Transcriptions in LJSpeech format
   - Minimum 30 minutes, ideal 2-5 hours

3. **Fine-tune with Unsloth** (fastest method)
   ```python
   from unsloth import FastLanguageModel
   model, tokenizer = FastLanguageModel.from_pretrained(
       model_name="orpheus-ai/orpheus-3b-0.1",
       max_seq_length=4096,
       load_in_4bit=True,
   )
   # Add LoRA adapters and train on your voice data
   # See: https://github.com/orpheus-ai/orpheus-tts/blob/main/finetune/
   ```

4. **Export and quantize**
   ```bash
   python export_to_gguf.py --model-dir ./finetuned --output orpheus-custom.gguf
   llama-quantize orpheus-custom.gguf orpheus-custom-Q4_K_M.gguf Q4_K_M
   ```

5. **Run inference server**
   ```bash
   # Token generation via llama-server
   llama-server -m orpheus-custom-Q4_K_M.gguf --port 1236 -ngl 99

   # SNAC decoder (converts tokens to audio)
   # Use the Rust SNAC decoder from candle for CPU decoding
   ```

6. **Wrap in Wyoming or HTTP endpoint, point Syntaur at it**
   ```bash
   TTS_URL=http://localhost:1236 syntaur
   ```

## Option 3: XTTS v2 (Voice Cloning, Minimal Data)

Best for: users who want to clone a voice with just 6-30 seconds of reference audio.

### What you need
- 6-30 seconds of clean reference audio (WAV, 22050Hz)
- Python 3.10+, GPU recommended but CPU works
- ~4GB VRAM

### Steps

1. **Install**
   ```bash
   pip install TTS
   ```

2. **Clone with reference audio** (zero-shot, no training)
   ```python
   from TTS.api import TTS
   tts = TTS("tts_models/multilingual/multi-dataset/xtts_v2")
   tts.tts_to_file(
       text="Hello, this is my custom voice.",
       speaker_wav="/path/to/reference.wav",
       language="en",
       file_path="output.wav"
   )
   ```

3. **Fine-tune for better quality** (optional, 10 minutes of data)
   ```bash
   python -m TTS.bin.train_tts \
     --config_path /path/to/xtts_config.json \
     --restore_path /path/to/xtts_v2_checkpoint.pth
   ```

4. **Run as HTTP server**
   ```python
   from TTS.api import TTS
   from flask import Flask, request, send_file

   app = Flask(__name__)
   tts = TTS("tts_models/multilingual/multi-dataset/xtts_v2")

   @app.post("/tts")
   def synthesize():
       text = request.json["text"]
       tts.tts_to_file(text=text, speaker_wav="ref.wav", language="en", file_path="/tmp/out.wav")
       return send_file("/tmp/out.wav", mimetype="audio/wav")

   app.run(port=10400)
   ```

5. **Point Syntaur at it**
   ```bash
   TTS_URL=http://localhost:10400 syntaur
   ```

## Option 4: Fish Audio (Cloud, Easy)

Best for: users who want high-quality custom voices without training infrastructure.

### Steps

1. Sign up at https://fish.audio
2. Upload 1-5 minutes of reference audio
3. Get your model ID and API key
4. Configure Syntaur:
   ```bash
   TTS_URL=https://api.fish.audio
   FISH_API_KEY=your_key
   FISH_MODEL_ID=your_model_id
   syntaur
   ```

## Connecting Custom TTS to Syntaur

Syntaur's TTS fallback chain:

1. **Wyoming endpoint** (`TTS_URL`) — any Wyoming-compatible TTS server
2. **OpenAI-compatible** (`OPENAI_TTS_URL` + `OPENAI_API_KEY`) — any OpenAI TTS API
3. **Edge TTS** (built-in default) — Microsoft neural voices, zero config

To use your custom voice, wrap it in either:
- A Wyoming-compatible HTTP server (POST `/tts` with JSON `{"text": "..."}` → WAV body)
- An OpenAI-compatible TTS endpoint (POST with `{"input": "...", "voice": "..."}` → audio body)

Set the corresponding env var and Syntaur will prefer your custom voice over Edge TTS.

## STT (Speech-to-Text) Options

| Option | Quality | Latency | Config |
|--------|---------|---------|--------|
| Browser Web Speech API | Good | ~500ms | None (default in /voice UI) |
| NVIDIA Parakeet (local) | Excellent | ~150ms | `STT_URL=http://host:10300` |
| OpenAI Whisper (cloud) | Excellent | ~1-2s | `OPENAI_STT_URL` + `OPENAI_API_KEY` |
| Faster Whisper (local) | Very Good | ~300ms | Run faster-whisper-server, set `STT_URL` |

## Hardware Requirements Summary

| Setup | GPU | VRAM | CPU | RAM | Notes |
|-------|-----|------|-----|-----|-------|
| Edge TTS (default) | None | 0 | Any | 256MB | Cloud, free |
| Piper TTS (local) | Optional | 0 | 4+ cores | 1GB | CPU inference is fast |
| Orpheus TTS | Required | 8GB+ | - | 16GB | Best quality |
| XTTS v2 | Recommended | 4GB+ | 8+ cores | 8GB | Zero-shot cloning |
| Parakeet STT | Optional | 0 | 4+ cores | 2GB | CPU ONNX inference |
| Full local stack | Required | 12GB+ | 8+ cores | 32GB | STT + LLM + TTS all local |
