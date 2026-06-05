// audio/transcription/custom_openai_provider.rs
//
// Custom OpenAI-compatible transcription provider (remote HTTP API).

use super::provider::{
    TranscriptResult, TranscriptSegmentResult, TranscriptionError, TranscriptionProvider,
    TranscriptionRequestMetadata,
};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use log::warn;
use reqwest::header::CONTENT_TYPE;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const WHISPER_SAMPLE_RATE: u32 = 16_000;
const WAV_BITS_PER_SAMPLE: u16 = 16;
const WAV_CHANNELS: u16 = 1;
const DEFAULT_CHAT_PROMPT: &str = "You are a speech-to-text engine. Transcribe the audio with high accuracy. Return a JSON object with a single key \"text\" containing the transcript. Do not include any other keys or commentary.";
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const CHAT_CHUNK_SECONDS: f32 = 60.0;
const CHAT_CHUNK_OVERLAP_SECONDS: f32 = 1.0;
const CHAT_DEDUP_WINDOW: usize = 200;

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
    send_chunk_metadata_fields: bool,
}

impl CustomOpenAIProvider {
    pub fn new(
        endpoint: String,
        api_key: Option<String>,
        model: String,
        transcription_api: Option<String>,
        transcription_prompt: Option<String>,
        send_chunk_metadata_fields: bool,
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
            send_chunk_metadata_fields,
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

    pub(crate) fn chunk_metadata_fields(
        enabled: bool,
        metadata: Option<&TranscriptionRequestMetadata>,
    ) -> Vec<(&'static str, String)> {
        if !enabled {
            return Vec::new();
        }

        let Some(metadata) = metadata else {
            return Vec::new();
        };

        let mut fields = Vec::new();
        if let Some(meeting_id) = metadata
            .meeting_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            fields.push(("meeting_id", meeting_id.to_string()));
        }
        if let Some(chunk_index) = metadata.chunk_index {
            fields.push(("chunk_index", chunk_index.to_string()));
        }
        if let Some(chunk_start_seconds) = metadata.chunk_start_seconds {
            if chunk_start_seconds.is_finite() {
                fields.push((
                    "chunk_start_seconds",
                    format!("{:.3}", chunk_start_seconds.max(0.0)),
                ));
            }
        }

        fields
    }

    fn apply_chunk_metadata(
        &self,
        mut form: Form,
        metadata: Option<&TranscriptionRequestMetadata>,
    ) -> Form {
        for (key, value) in Self::chunk_metadata_fields(self.send_chunk_metadata_fields, metadata) {
            form = form.text(key, value);
        }
        form
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

    fn chunk_audio_for_chat(audio: &[f32]) -> Vec<Vec<f32>> {
        let max_samples = (CHAT_CHUNK_SECONDS * WHISPER_SAMPLE_RATE as f32).round() as usize;
        if audio.len() <= max_samples {
            return vec![audio.to_vec()];
        }

        let overlap_samples =
            (CHAT_CHUNK_OVERLAP_SECONDS * WHISPER_SAMPLE_RATE as f32).round() as usize;
        let step = max_samples.saturating_sub(overlap_samples).max(1);

        let mut chunks = Vec::new();
        let mut start = 0usize;
        while start < audio.len() {
            let end = (start + max_samples).min(audio.len());
            chunks.push(audio[start..end].to_vec());
            if end == audio.len() {
                break;
            }
            start = start.saturating_add(step);
        }

        chunks
    }

    fn merge_transcripts_with_overlap(chunks: Vec<String>) -> String {
        let mut combined = String::new();

        for chunk in chunks {
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                continue;
            }

            if combined.is_empty() {
                combined = trimmed.to_string();
                continue;
            }

            let combined_chars: Vec<char> = combined.chars().collect();
            let next_chars: Vec<char> = trimmed.chars().collect();
            let max_overlap = CHAT_DEDUP_WINDOW
                .min(combined_chars.len())
                .min(next_chars.len());

            let mut overlap = 0usize;
            for len in (1..=max_overlap).rev() {
                if combined_chars[combined_chars.len() - len..] == next_chars[..len] {
                    overlap = len;
                    break;
                }
            }

            if overlap > 0 {
                let remaining: String = next_chars[overlap..].iter().collect();
                combined.push_str(&remaining);
            } else {
                combined.push(' ');
                combined.push_str(trimmed);
            }
        }

        combined
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

    fn extract_string_field(value: &Value, keys: &[&str]) -> Option<String> {
        for key in keys {
            if let Some(raw) = value.get(*key) {
                if let Some(text) = raw.as_str() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                } else if let Some(number) = raw.as_i64() {
                    return Some(number.to_string());
                } else if let Some(number) = raw.as_u64() {
                    return Some(number.to_string());
                }
            }
        }
        None
    }

    fn extract_f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
        for key in keys {
            if let Some(raw) = value.get(*key) {
                if let Some(number) = raw.as_f64() {
                    return Some(number);
                }
                if let Some(text) = raw.as_str() {
                    if let Ok(number) = text.trim().parse::<f64>() {
                        return Some(number);
                    }
                }
            }
        }
        None
    }

    fn extract_segment_results(value: &Value) -> Vec<TranscriptSegmentResult> {
        let segments = if let Some(array) = value.get("segments").and_then(|v| v.as_array()) {
            Some(array)
        } else {
            value.as_array()
        };

        segments
            .into_iter()
            .flatten()
            .filter_map(|segment| {
                let text = Self::extract_string_field(segment, &["text", "transcript", "content"])?;
                Some(TranscriptSegmentResult {
                    text,
                    start_time: Self::extract_f64_field(
                        segment,
                        &["start", "start_time", "startTime", "from"],
                    ),
                    end_time: Self::extract_f64_field(
                        segment,
                        &["end", "end_time", "endTime", "to"],
                    ),
                    speaker: Self::extract_string_field(
                        segment,
                        &[
                            "speaker",
                            "speaker_id",
                            "speakerId",
                            "speaker_label",
                            "speakerLabel",
                        ],
                    ),
                })
            })
            .collect()
    }

    fn collapse_speaker(segments: &[TranscriptSegmentResult]) -> Option<String> {
        let mut speakers = segments
            .iter()
            .filter_map(|segment| segment.speaker.as_deref())
            .filter(|speaker| !speaker.trim().is_empty());

        let first = speakers.next()?.to_string();
        if speakers.all(|speaker| speaker == first.as_str()) {
            Some(first)
        } else {
            None
        }
    }

    fn extract_stream_delta_text(value: &Value) -> Option<String> {
        let choice = value.get("choices")?.get(0)?;

        if let Some(delta) = choice.get("delta") {
            if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                return Some(content.to_string());
            }
            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
        }

        if let Some(text) = choice.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }

        // Fallback for servers that stream full message content
        Self::extract_chat_text(value)
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

    async fn read_chat_streaming_response(
        response: reqwest::Response,
    ) -> Result<String, TranscriptionError> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut collected = String::new();

        loop {
            let next_chunk = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await;
            match next_chunk {
                Ok(Some(Ok(chunk))) => {
                    let chunk_text = String::from_utf8_lossy(&chunk);
                    buffer.push_str(&chunk_text);

                    while let Some(idx) = buffer.find('\n') {
                        let line = buffer[..idx].trim().to_string();
                        buffer = buffer[idx + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        let data = match line.strip_prefix("data:") {
                            Some(payload) => payload.trim(),
                            None => continue,
                        };

                        if data == "[DONE]" {
                            if collected.trim().is_empty() {
                                return Err(TranscriptionError::EngineFailed(
                                    "Chat transcription response missing content".to_string(),
                                ));
                            }
                            return Ok(collected);
                        }

                        if let Ok(value) = serde_json::from_str::<Value>(data) {
                            if let Some(delta) = Self::extract_stream_delta_text(&value) {
                                if delta.starts_with(&collected) {
                                    let new_part = &delta[collected.len()..];
                                    collected.push_str(new_part);
                                } else {
                                    collected.push_str(&delta);
                                }
                            }
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    return Err(TranscriptionError::EngineFailed(e.to_string()));
                }
                Ok(None) => break,
                Err(_) => {
                    if collected.trim().is_empty() {
                        return Err(TranscriptionError::EngineFailed(
                            "Chat transcription stream timed out".to_string(),
                        ));
                    }
                    warn!("Chat transcription stream idle timeout; returning partial output");
                    return Ok(collected);
                }
            }
        }

        if collected.trim().is_empty() {
            Err(TranscriptionError::EngineFailed(
                "Chat transcription response missing content".to_string(),
            ))
        } else {
            Ok(collected)
        }
    }

    async fn read_chat_response(response: reqwest::Response) -> Result<String, TranscriptionError> {
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");

        let is_event_stream = content_type.contains("text/event-stream");

        if is_event_stream {
            return Self::read_chat_streaming_response(response).await;
        }

        let body = response
            .text()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;

        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            if let Some(text) = Self::extract_chat_text(&value) {
                return Ok(text);
            }
        }

        Ok(body)
    }

    async fn transcribe_audio(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
        use_translation_endpoint: bool,
        metadata: Option<&TranscriptionRequestMetadata>,
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
            .text("model", self.model.clone())
            // Ask for structured output so compatible servers include segment metadata.
            .text("response_format", "verbose_json");

        if let Some(lang) = language.as_ref() {
            if !lang.trim().is_empty() {
                form = form.text("language", lang.clone());
            }
        }

        let url = if use_translation_endpoint {
            self.translations_url()
        } else {
            form = self.apply_chunk_metadata(form, metadata);
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
                    .text("model", self.model.clone())
                    .text("response_format", "verbose_json");
                if let Some(lang) = language.as_ref() {
                    if !lang.trim().is_empty() {
                        retry_form = retry_form.text("language", lang.clone());
                    }
                }
                retry_form = self.apply_chunk_metadata(retry_form, metadata);

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

                let value: Value = serde_json::from_str(&retry_body).map_err(|e| {
                    TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e))
                })?;
                let segments = Self::extract_segment_results(&value);
                let text = value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string())
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| Self::extract_json_transcript(&value))
                    .unwrap_or_default();

                return Ok(TranscriptResult {
                    text,
                    confidence: None,
                    is_partial: false,
                    speaker: Self::extract_string_field(
                        &value,
                        &[
                            "speaker",
                            "speaker_id",
                            "speakerId",
                            "speaker_label",
                            "speakerLabel",
                        ],
                    )
                    .or_else(|| Self::collapse_speaker(&segments)),
                    segments,
                });
            }

            return Err(TranscriptionError::EngineFailed(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|e| TranscriptionError::EngineFailed(format!("Invalid JSON: {}", e)))?;
        let segments = Self::extract_segment_results(&value);
        let text = value
            .get("text")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| Self::extract_json_transcript(&value))
            .unwrap_or_default();

        Ok(TranscriptResult {
            text,
            confidence: None,
            is_partial: false,
            speaker: Self::extract_string_field(
                &value,
                &[
                    "speaker",
                    "speaker_id",
                    "speakerId",
                    "speaker_label",
                    "speakerLabel",
                ],
            )
            .or_else(|| Self::collapse_speaker(&segments)),
            segments,
        })
    }

    async fn transcribe_chat_single(
        &self,
        audio: Vec<f32>,
        prompt: &str,
    ) -> Result<String, TranscriptionError> {
        let wav_bytes = Self::encode_wav(&audio);
        let audio_b64 = general_purpose::STANDARD.encode(&wav_bytes);
        let data_url = format!("data:audio/wav;base64,{}", audio_b64.as_str());

        let chat_payloads = vec![
            (
                "audio_url",
                serde_json::json!([
                    { "type": "audio_url", "audio_url": { "url": data_url } },
                    { "type": "text", "text": prompt }
                ]),
            ),
            (
                "input_audio",
                serde_json::json!([
                    { "type": "text", "text": prompt },
                    { "type": "input_audio", "input_audio": { "data": audio_b64.clone(), "format": "wav" } }
                ]),
            ),
            (
                "audio",
                serde_json::json!([
                    { "type": "text", "text": prompt },
                    { "type": "audio", "audio": { "data": audio_b64, "format": "wav" } }
                ]),
            ),
        ];

        let mut last_error: Option<String> = None;

        for (label, content) in chat_payloads {
            for stream in [false, true] {
                let mut request_body = serde_json::json!({
                    "model": self.model.clone(),
                    "messages": [
                        { "role": "user", "content": content.clone() }
                    ],
                    "temperature": 0
                });

                if stream {
                    request_body["stream"] = serde_json::json!(true);
                }

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
                if !status.is_success() {
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    let message = format!("HTTP {}: {}", status, body);
                    if matches!(status.as_u16(), 400 | 415 | 422) {
                        warn!(
                            "Chat transcription payload '{}' failed (stream={}) with {}. Trying fallback.",
                            label, stream, message
                        );
                        last_error = Some(message);
                        continue;
                    }
                    return Err(TranscriptionError::EngineFailed(message));
                }

                let raw_text = match Self::read_chat_response(response).await {
                    Ok(text) => text,
                    Err(e) => {
                        last_error = Some(e.to_string());
                        continue;
                    }
                };

                let text = Self::normalize_chat_output(&raw_text);
                if text.trim().is_empty() {
                    last_error = Some("Chat transcription response missing content".to_string());
                    continue;
                }

                return Ok(text);
            }
        }

        Err(TranscriptionError::EngineFailed(
            last_error.unwrap_or_else(|| "Chat transcription failed".to_string()),
        ))
    }

    async fn transcribe_chat(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
        translate: bool,
    ) -> Result<TranscriptResult, TranscriptionError> {
        let prompt = self.build_chat_prompt(language.as_deref(), translate);
        let chunks = Self::chunk_audio_for_chat(&audio);
        let total_chunks = chunks.len();

        if total_chunks > 1 {
            warn!(
                "Chat transcription chunking enabled: {} chunks (~{:.1}s each)",
                total_chunks, CHAT_CHUNK_SECONDS
            );
        }

        let mut results = Vec::new();
        for chunk in chunks.into_iter() {
            let text = self.transcribe_chat_single(chunk, &prompt).await?;
            results.push(text);
        }

        let merged = Self::merge_transcripts_with_overlap(results);
        if merged.trim().is_empty() {
            return Err(TranscriptionError::EngineFailed(
                "Chat transcription response missing content".to_string(),
            ));
        }

        Ok(TranscriptResult {
            text: merged,
            confidence: None,
            is_partial: false,
            speaker: None,
            segments: Vec::new(),
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
        self.transcribe_with_metadata(audio, language, None).await
    }

    async fn transcribe_with_metadata(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
        metadata: Option<TranscriptionRequestMetadata>,
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
                self.transcribe_audio(audio, language, use_translation_endpoint, metadata.as_ref())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_metadata_fields_empty_when_disabled() {
        let metadata = TranscriptionRequestMetadata {
            meeting_id: Some("meeting-123".to_string()),
            chunk_index: Some(7),
            chunk_start_seconds: Some(12.3456),
        };

        assert!(CustomOpenAIProvider::chunk_metadata_fields(false, Some(&metadata)).is_empty());
    }

    #[test]
    fn chunk_metadata_fields_include_present_values() {
        let metadata = TranscriptionRequestMetadata {
            meeting_id: Some(" meeting-123 ".to_string()),
            chunk_index: Some(7),
            chunk_start_seconds: Some(12.3456),
        };

        assert_eq!(
            CustomOpenAIProvider::chunk_metadata_fields(true, Some(&metadata)),
            vec![
                ("meeting_id", "meeting-123".to_string()),
                ("chunk_index", "7".to_string()),
                ("chunk_start_seconds", "12.346".to_string()),
            ]
        );
    }

    #[test]
    fn chunk_metadata_fields_omit_empty_or_invalid_values() {
        let metadata = TranscriptionRequestMetadata {
            meeting_id: Some("   ".to_string()),
            chunk_index: None,
            chunk_start_seconds: Some(f64::NAN),
        };

        assert!(CustomOpenAIProvider::chunk_metadata_fields(true, Some(&metadata)).is_empty());
    }
}
