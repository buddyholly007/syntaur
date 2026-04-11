//! ESPHome native API voice assistant client.
//!
//! Connects directly to the Satellite1 ESPHome device on port 6053,
//! replacing Home Assistant as the voice pipeline controller. The satellite
//! thinks it's talking to HA — we implement just enough of the ESPHome
//! native API to handle the voice assistant flow.
//!
//! ## Architecture
//!
//! ```text
//! Satellite (6053) ←TCP→ syntaur (this module)
//!   Wake word detected → VoiceAssistantRequest(start=true)
//!   syntaur → VoiceAssistantResponse(port=0)  // API audio mode
//!   syntaur → EventResponse(RUN_START)
//!   syntaur → EventResponse(STT_START)
//!   Satellite streams VoiceAssistantAudio(pcm) →
//!   syntaur runs Parakeet STT
//!   syntaur → EventResponse(STT_END, text=transcript)
//!   syntaur → EventResponse(INTENT_START)
//!   syntaur calls voice_chat LLM
//!   syntaur → EventResponse(INTENT_END, text=response)
//!   syntaur → EventResponse(TTS_START)
//!   syntaur fetches TTS audio from Fish Audio
//!   syntaur → VoiceAssistantAudio(tts_pcm)
//!   syntaur → EventResponse(TTS_END)
//!   syntaur → EventResponse(RUN_END)
//! ```

pub mod esphome_api;
pub mod satellite_client;

use axum::{extract::Path, http::StatusCode, response::IntoResponse};

/// HTTP handler: GET /voice/tts/:id.wav — serves cached TTS audio to the satellite.
pub async fn handle_tts_audio(Path(filename): Path<String>) -> impl IntoResponse {
    let id = filename.trim_end_matches(".wav");
    match satellite_client::take_tts_audio(id).await {
        Some(audio) => {
            log::info!("[voice/tts] serving {} bytes for {}", audio.len(), id);
            (
                StatusCode::OK,
                [
                    ("content-type", "audio/wav"),
                    ("cache-control", "no-store"),
                ],
                audio,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
