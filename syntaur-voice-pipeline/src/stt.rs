//! STT engine — runs NVIDIA Parakeet TDT 0.6B v3 (INT8) locally via sherpa-onnx.
//!
//! No external server, no Python, no HTTP — pure Rust FFI to the sherpa-onnx
//! C library which runs the ONNX model on CPU.

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info};

const MODEL_NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";

/// Wrapper to make OfflineRecognizer Send+Sync.
/// Safety: we serialize all access through a std::sync::Mutex.
struct RecognizerWrapper(OfflineRecognizer);
unsafe impl Send for RecognizerWrapper {}
unsafe impl Sync for RecognizerWrapper {}

/// Inline STT engine wrapping a sherpa-onnx OfflineRecognizer.
pub struct SttEngine {
    recognizer: Arc<std::sync::Mutex<RecognizerWrapper>>,
}

impl SttEngine {
    /// Create a new STT engine. Downloads the model if not present.
    pub async fn new(model_dir: &str) -> Result<Self, String> {
        let model_path = PathBuf::from(model_dir).join(MODEL_NAME);

        if !model_path.exists() {
            info!("[stt] model not found at {:?}, downloading...", model_path);
            download_model(model_dir).await?;
        }

        info!("[stt] loading Parakeet model from {:?}", model_path);
        let recognizer = create_recognizer(&model_path)?;
        info!("[stt] Parakeet model loaded, ready for inference");

        Ok(Self {
            recognizer: Arc::new(std::sync::Mutex::new(RecognizerWrapper(recognizer))),
        })
    }

    /// Transcribe raw PCM audio (16-bit signed LE, 16kHz, mono) to text.
    pub async fn transcribe(&self, pcm_i16: &[u8]) -> Result<String, String> {
        if pcm_i16.len() < 32 {
            return Ok(String::new());
        }

        // Convert i16 PCM bytes to f32 samples normalized to [-1, 1]
        let samples: Vec<f32> = pcm_i16
            .chunks_exact(2)
            .map(|chunk| {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                sample as f32 / 32768.0
            })
            .collect();

        let duration_secs = samples.len() as f64 / 16000.0;
        info!("[stt] transcribing {:.1}s of audio ({} samples)", duration_secs, samples.len());

        let recognizer = self.recognizer.lock()
            .map_err(|e| format!("lock: {}", e))?;

        let start = std::time::Instant::now();

        let stream = recognizer.0.create_stream();
        stream.accept_waveform(16000, &samples);
        recognizer.0.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| "no result from recognizer".to_string())?;

        let elapsed = start.elapsed();
        let text = result.text.trim().to_string();

        info!(
            "[stt] transcript ({:.0}ms, {:.1}x realtime): \"{}\"",
            elapsed.as_millis(),
            duration_secs / elapsed.as_secs_f64(),
            text.chars().take(120).collect::<String>()
        );

        Ok(text)
    }
}

fn create_recognizer(model_path: &Path) -> Result<OfflineRecognizer, String> {
    let encoder = model_path.join("encoder.int8.onnx");
    let decoder = model_path.join("decoder.int8.onnx");
    let joiner = model_path.join("joiner.int8.onnx");
    let tokens = model_path.join("tokens.txt");

    for f in [&encoder, &decoder, &joiner, &tokens] {
        if !f.exists() {
            return Err(format!("missing model file: {:?}", f));
        }
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(encoder.to_string_lossy().to_string()),
        decoder: Some(decoder.to_string_lossy().to_string()),
        joiner: Some(joiner.to_string_lossy().to_string()),
    };
    config.model_config.tokens = Some(tokens.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = 4;
    config.model_config.debug = false;

    OfflineRecognizer::create(&config)
        .ok_or_else(|| "failed to create OfflineRecognizer — check model files".to_string())
}

async fn download_model(model_dir: &str) -> Result<(), String> {
    let dir = Path::new(model_dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {}", e))?;

    let archive_path = dir.join(format!("{}.tar.bz2", MODEL_NAME));

    info!("[stt] downloading model from {} (~640 MB)", MODEL_URL);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client: {}", e))?;

    let resp = client
        .get(MODEL_URL)
        .send()
        .await
        .map_err(|e| format!("download: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("download: HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await.map_err(|e| format!("download body: {}", e))?;
    std::fs::write(&archive_path, &bytes).map_err(|e| format!("write archive: {}", e))?;
    info!("[stt] downloaded {} MB, extracting...", bytes.len() / 1024 / 1024);

    let output = tokio::process::Command::new("tar")
        .args(["xjf", archive_path.to_str().unwrap(), "-C", model_dir])
        .output()
        .await
        .map_err(|e| format!("extract: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "extract failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let _ = std::fs::remove_file(&archive_path);
    info!("[stt] model extracted to {:?}", dir.join(MODEL_NAME));
    Ok(())
}
