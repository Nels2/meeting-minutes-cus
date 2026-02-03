// audio/transcription/custom_openai_provider.rs
//
// Custom OpenAI-compatible transcription provider (remote HTTP API).

use super::provider::{TranscriptionError, TranscriptionProvider, TranscriptResult};
use async_trait::async_trait;
use log::warn;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const WHISPER_SAMPLE_RATE: u32 = 16_000;
const WAV_BITS_PER_SAMPLE: u16 = 16;
const WAV_CHANNELS: u16 = 1;

pub struct CustomOpenAIProvider {
    endpoint: String,
    api_key: Option<String>,
    model: String,
    client: Client,
}

impl CustomOpenAIProvider {
    pub fn new(endpoint: String, api_key: Option<String>, model: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            endpoint,
            api_key,
            model,
            client,
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
