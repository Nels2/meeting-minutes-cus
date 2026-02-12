// audio/transcription/custom_openai_provider.rs
//
// Custom OpenAI-compatible transcription provider (remote HTTP API).

use super::provider::{TranscriptionError, TranscriptionProvider, TranscriptResult};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use log::warn;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const WHISPER_SAMPLE_RATE: u32 = 16_000;
const WAV_BITS_PER_SAMPLE: u16 = 16;
const WAV_CHANNELS: u16 = 1;
const DEFAULT_CHAT_PROMPT: &str = "You are a speech-to-text engine. Transcribe the audio with high accuracy. Return a JSON object with a single key \"text\" containing the transcript. Do not include any other keys or commentary.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptionApiMode {
    Audio,
    Chat,
}

impl TranscriptionApiMode {
    fn from_config(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()) {
            Some(mode)
                if mode == "chat"
                    || mode == "vision"
                    || mode == "chat-vision"
                    || mode == "chat/vision" =>
            {
                Self::Chat
            }
            _ => Self::Audio,
        }
    }
}

pub struct CustomOpenAIProvider {
    endpoint: String,
    api_key: Option<String>,
    model: String,
    client: Client,
    transcription_api: TranscriptionApiMode,
    transcription_prompt: Option<String>,
}

impl CustomOpenAIProvider {
    pub fn new(
        endpoint: String,
        api_key: Option<String>,
        model: String,
        transcription_api: Option<String>,
        transcription_prompt: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            endpoint,
            api_key,
            model,
            client,
            transcription_api: TranscriptionApiMode::from_config(transcription_api.as_deref()),
            transcription_prompt: transcription_prompt
                .as_deref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        }
    }

    fn transcription_url(&self) -> String {
        self.audio_url("transcriptions")
    }

    fn translations_url(&self) -> String {
        self.audio_url("translations")
    }

    fn audio_url(&self, endpoint: &str) -> String {
        let trimmed = self.endpoint.trim_end_matches('/');
        if trimmed.ends_with("/audio/transcriptions") {
            let base = trimmed.trim_end_matches("/audio/transcriptions");
            format!("{}/audio/{}", base, endpoint)
        } else if trimmed.ends_with("/audio/translations") {
            let base = trimmed.trim_end_matches("/audio/translations");
            format!("{}/audio/{}", base, endpoint)
        } else if trimmed.ends_with("/v1") {
            format!("{}/audio/{}", trimmed, endpoint)
        } else {
            format!("{}/v1/audio/{}", trimmed, endpoint)
        }
    }

    fn chat_url(&self) -> String {
        let trimmed = self.endpoint.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") {
            trimmed.to_string()
        } else if trimmed.ends_with("/v1/chat/completions") {
            trimmed.to_string()
        } else if trimmed.ends_with("/v1/chat") {
            format!("{}/completions", trimmed)
        } else if trimmed.ends_with("/v1") {
            format!("{}/chat/completions", trimmed)
        } else {
            format!("{}/v1/chat/completions", trimmed)
        }
    }

    fn encode_wav(audio: &[f32]) -> Vec<u8> {
        let mut pcm_bytes = Vec::with_capacity(audio.len() * 2);

        for &sample in audio {
            let clamped = sample.clamp(-1.0, 1.0);
            let scaled = (clamped * i16::MAX as f32).round() as i16;
            pcm_bytes.extend_from_slice(&scaled.to_le_bytes());
        }

        let data_size = pcm_bytes.len() as u32;
        let file_size = 36 + data_size;
        let block_align = WAV_CHANNELS * (WAV_BITS_PER_SAMPLE / 8);
        let byte_rate = WHISPER_SAMPLE_RATE * block_align as u32;

        let mut wav = Vec::with_capacity(44 + pcm_bytes.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&WAV_CHANNELS.to_le_bytes());
        wav.extend_from_slice(&WHISPER_SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&WAV_BITS_PER_SAMPLE.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(&pcm_bytes);
        wav
    }

    fn build_chat_prompt(&self, language: Option<&str>, translate: bool) -> String {
        let mut prompt = self
            .transcription_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_CHAT_PROMPT.to_string());

        if translate {
            prompt.push_str("\n\nTranslate to English.");
        } else if let Some(lang) = language {
            if !lang.trim().is_empty() {
                prompt.push_str(&format!("\n\nTranscribe in {}.", lang.trim()));
            }
        }

        prompt
    }

    fn extract_chat_text(value: &Value) -> Option<String> {
        let content = value
            .get("choices")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("content"));

        match content {
            Some(Value::String(text)) => Some(text.clone()),
            Some(Value::Array(parts)) => {
                let mut combined = String::new();
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(text);
                    }
                }
                if combined.is_empty() {
                    None
                } else {
                    Some(combined)
                }
            }
            _ => None,
        }
    }

    fn extract_json_transcript(value: &Value) -> Option<String> {
        if let Some(obj) = value.as_object() {
            for key in ["text", "transcript", "transcription", "output", "content"] {
                if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        return Some(text.trim().to_string());
                    }
                }
            }

            if let Some(segments) = obj.get("segments").and_then(|v| v.as_array()) {
                let mut combined = String::new();
                for segment in segments {
                    if let Some(text) = segment.get("text").and_then(|v| v.as_str()) {
                        if !combined.is_empty() {
                            combined.push(' ');
                        }
                        combined.push_str(text.trim());
                    }
                }
                if !combined.trim().is_empty() {
                    return Some(combined.trim().to_string());
                }
            }
        }

        if let Some(arr) = value.as_array() {
            let mut combined = String::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if !combined.is_empty() {
                        combined.push(' ');
                    }
                    combined.push_str(text.trim());
                }
            }
            if !combined.trim().is_empty() {
                return Some(combined.trim().to_string());
            }
        }

        None
    }

    fn normalize_chat_output(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        let mut candidate = trimmed;
        if candidate.starts_with("```") {
            if let Some(end) = candidate.rfind("```") {
                candidate = &candidate[3..end];
                candidate = candidate.trim();
                if candidate.starts_with("json") {
                    candidate = candidate.trim_start_matches("json").trim();
                }
            }
        }

        if candidate.starts_with('{') || candidate.starts_with('[') {
            if let Ok(value) = serde_json::from_str::<Value>(candidate) {
                if let Some(extracted) = Self::extract_json_transcript(&value) {
                    return extracted;
                }
            }
        }

        trimmed.to_string()
    }

    async fn transcribe_audio(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
        use_translation_endpoint: bool,
    ) -> Result<TranscriptResult, TranscriptionError> {
        let wav_bytes = Self::encode_wav(&audio);
        let retry_wav_bytes = if use_translation_endpoint {
            Some(wav_bytes.clone())
        } else {
            None
        };

        let part = Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        let mut form = Form::new()
            .part("file", part)
            .text("model", self.model.clone());

        if let Some(lang) = language.as_ref() {
            if !lang.trim().is_empty() {
                form = form.text("language", lang.clone());
            }
        }

        let url = if use_translation_endpoint {
            self.translations_url()
        } else {
            self.transcription_url()
        };

        let mut request = self.client.post(url).multipart(form);
        if let Some(api_key) = &self.api_key {
            if !api_key.trim().is_empty() {
                request = request.bearer_auth(api_key);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        if !status.is_success() {
            if use_translation_endpoint && matches!(status.as_u16(), 404 | 405) {
                warn!(
                    "Custom OpenAI endpoint does not support /audio/translations (HTTP {}). Falling back to /audio/transcriptions.",
                    status
                );
                let retry_wav_bytes = retry_wav_bytes.ok_or_else(|| {
                    TranscriptionError::EngineFailed("Failed to retry request".to_string())
                })?;
                let retry_part = Part::bytes(retry_wav_bytes)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;
                let mut retry_form = Form::new()
                    .part("file", retry_part)
                    .text("model", self.model.clone());
                if let Some(lang) = language.as_ref() {
                    if !lang.trim().is_empty() {
                        retry_form = retry_form.text("language", lang.clone());
                    }
                }

                let mut retry_request = self
                    .client
                    .post(self.transcription_url())
                    .multipart(retry_form);
                if let Some(api_key) = &self.api_key {
                    if !api_key.trim().is_empty() {
                        retry_request = retry_request.bearer_auth(api_key);
                    }
                }

                let retry_response = retry_request
                    .send()
                    .await
                    .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

                let retry_status = retry_response.status();
                let retry_body = retry_response
                    .text()
                    .await
                    .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

                if !retry_status.is_success() {
                    return Err(TranscriptionError::EngineFailed(format!(
                        "HTTP {}: {}",
                        retry_status, retry_body
                    )));
                }

                let value: Value = serde_json::from_str(&retry_body)
                    .map_err(|e| TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e)))?;
                let text = value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                return Ok(TranscriptResult {
                    text,
                    confidence: None,
                    is_partial: false,
                });
            }

            return Err(TranscriptionError::EngineFailed(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|e| TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e)))?;
        let text = value
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        Ok(TranscriptResult {
            text,
            confidence: None,
            is_partial: false,
        })
    }

    async fn transcribe_chat(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
        translate: bool,
    ) -> Result<TranscriptResult, TranscriptionError> {
        let wav_bytes = Self::encode_wav(&audio);
        let audio_b64 = general_purpose::STANDARD.encode(wav_bytes);
        let prompt = self.build_chat_prompt(language.as_deref(), translate);

        let content = serde_json::json!([
            { "type": "text", "text": prompt.clone() },
            { "type": "input_audio", "input_audio": { "data": audio_b64.clone(), "format": "wav" } }
        ]);

        let request_body = serde_json::json!({
            "model": self.model.clone(),
            "messages": [
                { "role": "user", "content": content }
            ],
            "temperature": 0
        });

        let mut request = self.client.post(self.chat_url()).json(&request_body);
        if let Some(api_key) = &self.api_key {
            if !api_key.trim().is_empty() {
                request = request.bearer_auth(api_key);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        if !status.is_success() && matches!(status.as_u16(), 400 | 415 | 422) {
            warn!(
                "Chat transcription failed with status {}. Retrying with alternative payload format.",
                status
            );

            let alt_content = serde_json::json!([
                { "type": "text", "text": prompt },
                { "type": "audio", "audio": { "data": audio_b64, "format": "wav" } }
            ]);

            let alt_body = serde_json::json!({
                "model": self.model.clone(),
                "messages": [
                    { "role": "user", "content": alt_content }
                ],
                "temperature": 0
            });

            let mut retry_request = self.client.post(self.chat_url()).json(&alt_body);
            if let Some(api_key) = &self.api_key {
                if !api_key.trim().is_empty() {
                    retry_request = retry_request.bearer_auth(api_key);
                }
            }

            let retry_response = retry_request
                .send()
                .await
                .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

            let retry_status = retry_response.status();
            let retry_body = retry_response
                .text()
                .await
                .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

            if !retry_status.is_success() {
                return Err(TranscriptionError::EngineFailed(format!(
                    "HTTP {}: {}",
                    retry_status, retry_body
                )));
            }

            let value: Value = serde_json::from_str(&retry_body)
                .map_err(|e| TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e)))?;
            let raw_text = Self::extract_chat_text(&value).unwrap_or_default();
            let text = Self::normalize_chat_output(&raw_text);

            if text.trim().is_empty() {
                return Err(TranscriptionError::EngineFailed(
                    "Chat transcription response missing content".to_string(),
                ));
            }

            return Ok(TranscriptResult {
                text,
                confidence: None,
                is_partial: false,
            });
        }

        if !status.is_success() {
            return Err(TranscriptionError::EngineFailed(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|e| TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e)))?;
        let raw_text = Self::extract_chat_text(&value).unwrap_or_default();
        let text = Self::normalize_chat_output(&raw_text);

        if text.trim().is_empty() {
            return Err(TranscriptionError::EngineFailed(
                "Chat transcription response missing content".to_string(),
            ));
        }

        Ok(TranscriptResult {
            text,
            confidence: None,
            is_partial: false,
        })
    }
}

#[async_trait]
impl TranscriptionProvider for CustomOpenAIProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
        if audio.is_empty() {
            return Err(TranscriptionError::AudioTooShort {
                samples: 0,
                minimum: 1,
            });
        }

        let mut use_translation_endpoint = false;
        let language = match language {
            Some(lang) => {
                let trimmed = lang.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let normalized = trimmed.to_ascii_lowercase();
                    match normalized.as_str() {
                        "auto" => None,
                        "auto-translate" => {
                            use_translation_endpoint = true;
                            None
                        }
                        _ => Some(trimmed.to_string()),
                    }
                }
            }
            None => None,
        };

        match self.transcription_api {
            TranscriptionApiMode::Chat => {
                self.transcribe_chat(audio, language, use_translation_endpoint)
                    .await
            }
            TranscriptionApiMode::Audio => {
                self.transcribe_audio(audio, language, use_translation_endpoint)
                    .await
            }
        }
    }

    async fn is_model_loaded(&self) -> bool {
        !self.model.trim().is_empty()
    }

    async fn get_current_model(&self) -> Option<String> {
        if self.model.trim().is_empty() {
            None
        } else {
            Some(self.model.clone())
        }
    }

    fn provider_name(&self) -> &'static str {
        "Custom OpenAI"
    }
}
