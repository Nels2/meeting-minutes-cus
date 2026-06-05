// Retranscription module - allows re-processing stored audio with different settings

use super::common::{
    create_full_audio_segment, create_transcript_segments, split_segment_at_silence,
    write_transcripts_json, TimedTranscript,
};
use super::constants::AUDIO_EXTENSIONS;
use crate::audio::decoder::decode_audio_file;
use crate::audio::transcription::{
    get_or_init_transcription_engine, CustomOpenAIProvider, TranscriptResult, TranscriptionEngine,
    TranscriptionRequestMetadata,
};
use crate::audio::vad::get_speech_chunks_with_progress;
use crate::config::{DEFAULT_PARAKEET_MODEL, DEFAULT_WHISPER_MODEL};
use crate::parakeet_engine::ParakeetEngine;
use crate::state::AppState;
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Global flag to track if retranscription is in progress
static RETRANSCRIPTION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Global flag to signal cancellation
static RETRANSCRIPTION_CANCELLED: AtomicBool = AtomicBool::new(false);

/// RAII guard for RETRANSCRIPTION_IN_PROGRESS flag
/// Ensures flag is cleared even if retranscription panics or returns early
struct RetranscriptionGuard;

impl RetranscriptionGuard {
    /// Create guard and set flag atomically
    fn acquire() -> Result<Self, String> {
        if RETRANSCRIPTION_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Retranscription already in progress".to_string());
        }
        Ok(RetranscriptionGuard)
    }
}

impl Drop for RetranscriptionGuard {
    fn drop(&mut self) {
        RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// VAD redemption time in milliseconds - bridges natural pauses in speech
/// Batch processing needs longer redemption (2000ms) than live pipeline (400ms)
/// because the entire file is processed at once by VAD, and 400ms fragments
/// speech at every natural sentence/topic pause (500ms-2s)
const VAD_REDEMPTION_TIME_MS: u32 = 2000;

/// Progress update emitted during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionProgress {
    pub meeting_id: String,
    pub stage: String, // "decoding", "transcribing", "saving"
    pub progress_percentage: u32,
    pub message: String,
}

/// Result of retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionResult {
    pub meeting_id: String,
    pub segments_count: usize,
    pub duration_seconds: f64,
    pub language: Option<String>,
}

/// Error during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionError {
    pub meeting_id: String,
    pub error: String,
}

/// Check if retranscription is currently in progress
pub fn is_retranscription_in_progress() -> bool {
    RETRANSCRIPTION_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Cancel ongoing retranscription
pub fn cancel_retranscription() {
    RETRANSCRIPTION_CANCELLED.store(true, Ordering::SeqCst);
}

/// Start retranscription of a meeting's audio
pub async fn start_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    vad_preprocessing_enabled: Option<bool>,
) -> Result<RetranscriptionResult> {
    // Acquire guard - ensures flag is cleared even on panic/early return
    let _guard = RetranscriptionGuard::acquire().map_err(|e| anyhow!(e))?;

    // Reset cancellation flag
    RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);

    let resolved_provider = resolve_requested_provider(&app, provider.as_deref()).await?;
    let vad_preprocessing_enabled =
        resolve_batch_vad_preprocessing_enabled(&app, vad_preprocessing_enabled).await?;
    let use_parakeet = resolved_provider == "parakeet";
    let result = run_retranscription(
        app.clone(),
        meeting_id.clone(),
        meeting_folder_path,
        language,
        model,
        resolved_provider,
        vad_preprocessing_enabled,
    )
    .await;

    // Unload the engine after the batch job (success, failure, or cancellation)
    super::common::unload_engine_after_batch(use_parakeet).await;

    // Guard will automatically clear flag on drop
    // No need for manual: RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);

    match &result {
        Ok(res) => {
            let _ = app.emit(
                "retranscription-complete",
                serde_json::json!({
                    "meeting_id": res.meeting_id,
                    "segments_count": res.segments_count,
                    "duration_seconds": res.duration_seconds,
                    "language": res.language
                }),
            );
        }
        Err(e) => {
            let _ = app.emit(
                "retranscription-error",
                RetranscriptionError {
                    meeting_id: meeting_id.clone(),
                    error: e.to_string(),
                },
            );
        }
    }

    result
}

/// Find audio file in meeting folder
/// Tries common names first, then scans for any file with an audio extension
fn find_audio_file(folder: &Path) -> Result<PathBuf> {
    let candidates = [
        "audio.mp4",
        "audio.m4a",
        "audio.wav",
        "audio.mp3",
        "audio.flac",
        "audio.ogg",
        "recording.mp4",
        "audio.mkv",
        "audio.webm",
        "audio.wma",
    ];

    for name in candidates {
        let path = folder.join(name);
        if path.exists() {
            return Ok(path);
        }
    }

    // Fallback: scan folder for any file with an audio extension
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                    return Ok(path);
                }
            }
        }
    }

    Err(anyhow!("No audio file found in: {}", folder.display()))
}

/// Internal function to run retranscription
async fn run_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: String,
    vad_preprocessing_enabled: bool,
) -> Result<RetranscriptionResult> {
    let folder_path = PathBuf::from(&meeting_folder_path);
    let audio_path = find_audio_file(&folder_path)?;

    // Determine which provider to use (default to whisper)
    let use_parakeet = provider == "parakeet";

    info!(
        "Starting retranscription for meeting {} with language {:?}, model {:?}, provider {:?}",
        meeting_id, language, model, provider
    );

    // Emit progress: decoding
    emit_progress(&app, &meeting_id, "decoding", 5, "Decoding audio file...");

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    // Decode the audio file (CPU-intensive, run in blocking task)
    let path_for_decode = audio_path.clone();
    let decoded = tokio::task::spawn_blocking(move || decode_audio_file(&path_for_decode))
        .await
        .map_err(|e| anyhow!("Decode task panicked: {}", e))??;
    let duration_seconds = decoded.duration_seconds;

    info!(
        "Decoded audio: {:.2}s, {}Hz, {} channels",
        duration_seconds, decoded.sample_rate, decoded.channels
    );

    emit_progress(
        &app,
        &meeting_id,
        "decoding",
        15,
        "Converting audio format...",
    );

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    // Convert to 16kHz mono format (CPU-intensive, run in blocking task)
    let audio_samples = tokio::task::spawn_blocking(move || decoded.to_whisper_format())
        .await
        .map_err(|e| anyhow!("Resample task panicked: {}", e))?;
    info!(
        "Converted to 16kHz mono format: {} samples",
        audio_samples.len()
    );

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    let speech_segments = if vad_preprocessing_enabled {
        emit_progress(&app, &meeting_id, "vad", 20, "Detecting speech segments...");

        // Use VAD to find natural speech boundaries (same approach as live transcription)
        // IMPORTANT: Run VAD in a blocking task to avoid blocking the async runtime
        // For large files (35+ minutes), VAD processing can take several minutes
        let app_for_vad = app.clone();
        let meeting_id_for_vad = meeting_id.clone();

        tokio::task::spawn_blocking(move || {
            get_speech_chunks_with_progress(
                &audio_samples,
                VAD_REDEMPTION_TIME_MS,
                |vad_progress, segments_found| {
                    // Map VAD progress (0-100) to overall progress (20-25)
                    let overall_progress = 20 + (vad_progress as f32 * 0.05) as u32;
                    emit_progress(
                        &app_for_vad,
                        &meeting_id_for_vad,
                        "vad",
                        overall_progress,
                        &format!(
                            "Detecting speech segments... {}% ({} found)",
                            vad_progress, segments_found
                        ),
                    );

                    // Return false to cancel if cancellation requested
                    !RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst)
                },
            )
        })
        .await
        .map_err(|e| anyhow!("VAD task panicked: {}", e))?
        .map_err(|e| anyhow!("VAD processing failed: {}", e))?
    } else {
        info!("Skipping VAD preprocessing for retranscription; transcribing full audio");
        create_full_audio_segment(audio_samples, duration_seconds)
    };

    let total_segments = speech_segments.len();
    if vad_preprocessing_enabled {
        info!(
            "VAD detected {} speech segments (redemption_time={}ms)",
            total_segments, VAD_REDEMPTION_TIME_MS
        );
    } else {
        info!(
            "Created {} full-audio segment(s) with VAD preprocessing disabled",
            total_segments
        );
    }

    // Diagnostic: log segment duration distribution
    if !speech_segments.is_empty() {
        let durations_ms: Vec<f64> = speech_segments
            .iter()
            .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
            .collect();
        let total_speech_ms: f64 = durations_ms.iter().sum();
        let avg_duration = total_speech_ms / durations_ms.len() as f64;
        let min_duration = durations_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_duration = durations_ms
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        info!(
            "VAD segment stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
            avg_duration, min_duration, max_duration,
            total_speech_ms / 1000.0, duration_seconds,
            (total_speech_ms / 1000.0 / duration_seconds) * 100.0
        );
        // Log first 10 segments for detailed inspection
        for (i, seg) in speech_segments.iter().take(10).enumerate() {
            let dur = seg.end_timestamp_ms - seg.start_timestamp_ms;
            debug!(
                "  Segment {}: {:.0}ms-{:.0}ms ({:.0}ms, {} samples)",
                i,
                seg.start_timestamp_ms,
                seg.end_timestamp_ms,
                dur,
                seg.samples.len()
            );
        }
        if total_segments > 10 {
            debug!("  ... and {} more segments", total_segments - 10);
        }
    }

    if total_segments == 0 && vad_preprocessing_enabled {
        warn!("No speech detected in audio");
        return Err(anyhow!("No speech detected in audio file"));
    } else if total_segments == 0 {
        warn!("No audio samples available after decoding");
        return Err(anyhow!("No audio samples available after decoding"));
    }

    emit_progress(
        &app,
        &meeting_id,
        "transcribing",
        25,
        "Loading transcription engine...",
    );

    // Initialize the appropriate engine once (not per-segment)
    let transcription_engine =
        get_retranscription_transcription_engine(&app, Some(provider.as_str()), model.as_deref())
            .await?;

    // Split very long segments at silence boundaries for better transcription quality.
    // Hard cuts at arbitrary sample positions lose words at boundaries. Instead, scan
    // for the lowest-energy window near the target split point and cut there.
    const MAX_SEGMENT_SAMPLES: usize = 25 * 16000; // 25 seconds at 16kHz

    let mut processable_segments: Vec<crate::audio::vad::SpeechSegment> = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            debug!(
                "Splitting large segment ({:.0}ms, {} samples) at silence boundaries",
                segment.end_timestamp_ms - segment.start_timestamp_ms,
                segment.samples.len()
            );

            let sub_segments = split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES);
            debug!("Split into {} sub-segments", sub_segments.len());
            processable_segments.extend(sub_segments);
        } else {
            processable_segments.push(segment.clone());
        }
    }

    let processable_count = processable_segments.len();
    info!(
        "Processing {} segments (after splitting)",
        processable_count
    );

    // Process each speech segment with progress updates
    let mut all_transcripts: Vec<TimedTranscript> = Vec::new();
    let mut total_confidence = 0.0f32;

    for (i, segment) in processable_segments.iter().enumerate() {
        // Check for cancellation before each segment
        if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
            return Err(anyhow!("Retranscription cancelled"));
        }

        // Calculate progress (25% to 80% range for transcription)
        let progress = 25 + ((i as f32 / processable_count as f32) * 55.0) as u32;
        let segment_duration_sec = (segment.end_timestamp_ms - segment.start_timestamp_ms) / 1000.0;
        emit_progress(
            &app,
            &meeting_id,
            "transcribing",
            progress,
            &format!(
                "Transcribing segment {} of {} ({:.1}s)...",
                i + 1,
                processable_count,
                segment_duration_sec
            ),
        );

        // Skip very short segments (< 100ms of audio = 1600 samples at 16kHz)
        if segment.samples.len() < 1600 {
            debug!(
                "Skipping short segment {} with {} samples",
                i,
                segment.samples.len()
            );
            continue;
        }

        let result = transcribe_retranscription_segment(
            &transcription_engine,
            segment.samples.clone(),
            if use_parakeet { None } else { language.clone() },
            i,
            Some(TranscriptionRequestMetadata {
                meeting_id: Some(meeting_id.clone()),
                chunk_index: Some(i as u64),
                chunk_start_seconds: Some(segment.start_timestamp_ms / 1000.0),
            }),
        )
        .await?;

        let confidence = result.confidence.unwrap_or(0.9f32);
        let added_count = append_retranscription_transcripts(
            &mut all_transcripts,
            &result,
            segment.start_timestamp_ms,
            segment.end_timestamp_ms,
            i,
            processable_count,
            segment_duration_sec,
            confidence,
        );

        if added_count == 0 {
            debug!(
                "Segment {}/{}: {:.1}s - empty transcription",
                i + 1,
                processable_count,
                segment_duration_sec
            );
        } else {
            total_confidence += confidence * added_count as f32;
        }
    }

    let transcribed_count = all_transcripts.len();
    let avg_confidence = if transcribed_count > 0 {
        total_confidence / transcribed_count as f32
    } else {
        0.0
    };

    info!(
        "Transcription complete: {} segments transcribed out of {}, avg confidence: {:.2}",
        transcribed_count, processable_count, avg_confidence
    );

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    emit_progress(&app, &meeting_id, "saving", 80, "Saving transcripts...");

    // Create transcript segments with proper timestamps from VAD
    let segments = create_transcript_segments(&all_transcripts);

    // Save to database
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    // Wrap delete+insert+update in a transaction to prevent data loss
    let pool = app_state.db_manager.pool();
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow!("DB error: {}", e))?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to start transaction: {}", e))?;

    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(&meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to delete existing transcripts: {}", e))?;

    for segment in &segments {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&segment.id)
        .bind(&meeting_id)
        .bind(&segment.text)
        .bind(&segment.timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(segment.duration)
        .bind(&segment.speaker)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to insert transcript: {}", e))?;
    }

    tx.commit()
        .await
        .map_err(|e| anyhow!("Failed to commit transaction: {}", e))?;

    info!(
        "Updated {} transcripts for meeting {} in transaction",
        segments.len(),
        meeting_id
    );

    // Write updated transcripts.json and metadata.json to the meeting folder
    emit_progress(
        &app,
        &meeting_id,
        "saving",
        90,
        "Writing transcript files...",
    );

    if let Err(e) = write_transcripts_json(&folder_path, &segments) {
        warn!("Failed to write transcripts.json: {}", e);
    }

    // Find audio filename for metadata
    let audio_filename = audio_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.mp4")
        .to_string();

    if let Err(e) =
        write_retranscription_metadata(&folder_path, &meeting_id, duration_seconds, &audio_filename)
    {
        warn!("Failed to update metadata.json: {}", e);
    }

    emit_progress(
        &app,
        &meeting_id,
        "complete",
        100,
        "Retranscription complete",
    );

    Ok(RetranscriptionResult {
        meeting_id,
        segments_count: segments.len(),
        duration_seconds,
        language,
    })
}

fn normalize_provider_alias(provider: &str) -> &str {
    match provider {
        "whisper" => "localWhisper",
        other => other,
    }
}

async fn resolve_requested_provider<R: Runtime>(
    app: &AppHandle<R>,
    requested_provider: Option<&str>,
) -> Result<String> {
    if let Some(provider) = requested_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        return Ok(normalize_provider_alias(provider).to_string());
    }

    let config = crate::api::api::api_get_transcript_config(app.clone(), app.clone().state(), None)
        .await
        .map_err(|e| anyhow!("Failed to load transcript settings: {}", e))?
        .unwrap_or(crate::api::api::TranscriptConfig {
            provider: "parakeet".to_string(),
            model: DEFAULT_PARAKEET_MODEL.to_string(),
            api_key: None,
            vad_preprocessing_enabled: true,
        });

    Ok(normalize_provider_alias(&config.provider).to_string())
}

async fn resolve_batch_vad_preprocessing_enabled<R: Runtime>(
    app: &AppHandle<R>,
    requested: Option<bool>,
) -> Result<bool> {
    if let Some(enabled) = requested {
        return Ok(enabled);
    }

    let config = crate::api::api::api_get_transcript_config(app.clone(), app.clone().state(), None)
        .await
        .map_err(|e| anyhow!("Failed to load transcript settings: {}", e))?;

    Ok(config.map_or(true, |config| config.vad_preprocessing_enabled))
}

async fn get_retranscription_transcription_engine<R: Runtime>(
    app: &AppHandle<R>,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Result<TranscriptionEngine> {
    if requested_provider.is_none() && requested_model.is_none() {
        return get_or_init_transcription_engine(app).await.map_err(|e| {
            anyhow!(
                "Failed to initialize transcription engine from settings: {}",
                e
            )
        });
    }

    let provider = resolve_requested_provider(app, requested_provider).await?;

    match provider.as_str() {
        "localWhisper" => Ok(TranscriptionEngine::Whisper(
            get_or_init_whisper(app, requested_model).await?,
        )),
        "parakeet" => Ok(TranscriptionEngine::Parakeet(
            get_or_init_parakeet(app, requested_model).await?,
        )),
        "custom-openai" => {
            let custom_config = crate::api::api::api_get_transcript_custom_openai_config(
                app.clone(),
                app.clone().state(),
            )
            .await
            .map_err(|e| anyhow!("Failed to load Custom OpenAI transcription settings: {}", e))?
            .ok_or_else(|| {
                anyhow!(
                    "Custom OpenAI endpoint is not configured. Please set it in transcription settings."
                )
            })?;

            let transcript_config = crate::api::api::api_get_transcript_config(
                app.clone(),
                app.clone().state(),
                None,
            )
            .await
            .map_err(|e| anyhow!("Failed to load transcript settings: {}", e))?;

            let model_name = requested_model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    transcript_config
                        .as_ref()
                        .filter(|config| {
                            normalize_provider_alias(&config.provider) == "custom-openai"
                                && !config.model.trim().is_empty()
                        })
                        .map(|config| config.model.trim().to_string())
                })
                .or_else(|| {
                    let configured = custom_config.model.trim();
                    if configured.is_empty() {
                        None
                    } else {
                        Some(configured.to_string())
                    }
                })
                .ok_or_else(|| anyhow!("Custom OpenAI transcription model is required."))?;

            Ok(TranscriptionEngine::Provider(Arc::new(
                CustomOpenAIProvider::new(
                    custom_config.endpoint,
                    custom_config.api_key,
                    model_name,
                    custom_config.transcription_api,
                    custom_config.transcription_prompt,
                    custom_config.send_chunk_metadata_fields,
                ),
            )))
        }
        other => Err(anyhow!(
            "Provider '{}' is not supported for retranscription. Please select 'localWhisper', 'parakeet', or 'custom-openai'.",
            other
        )),
    }
}

fn append_retranscription_transcripts(
    all_transcripts: &mut Vec<TimedTranscript>,
    result: &TranscriptResult,
    segment_start_ms: f64,
    segment_end_ms: f64,
    segment_index: usize,
    total_segments: usize,
    segment_duration_sec: f64,
    confidence: f32,
) -> usize {
    let mut added_count = 0usize;
    let segment_duration_ms = (segment_end_ms - segment_start_ms).max(0.0);

    if !result.segments.is_empty() {
        for (sub_index, transcript_segment) in result.segments.iter().enumerate() {
            let trimmed = transcript_segment.text.trim();
            if trimmed.is_empty() {
                continue;
            }

            let speaker = transcript_segment
                .speaker
                .clone()
                .or_else(|| result.speaker.clone());

            let start_offset_ms = (transcript_segment.start_time.unwrap_or(0.0) * 1000.0)
                .clamp(0.0, segment_duration_ms);
            let next_start_ms = result
                .segments
                .get(sub_index + 1)
                .and_then(|next| next.start_time)
                .map(|value| (value * 1000.0).clamp(start_offset_ms, segment_duration_ms));
            let end_offset_ms = transcript_segment
                .end_time
                .map(|value| (value * 1000.0).clamp(start_offset_ms, segment_duration_ms))
                .or(next_start_ms)
                .unwrap_or(segment_duration_ms);
            let absolute_end_ms = if end_offset_ms > start_offset_ms {
                segment_start_ms + end_offset_ms
            } else {
                segment_end_ms
            };

            debug!(
                "Segment {}/{}: {:.1}s, conf={:.2}, speaker={:?}, text='{}'",
                segment_index + 1,
                total_segments,
                segment_duration_sec,
                confidence,
                speaker.as_deref(),
                preview_text(trimmed)
            );

            all_transcripts.push(TimedTranscript {
                text: trimmed.to_string(),
                start_ms: segment_start_ms + start_offset_ms,
                end_ms: absolute_end_ms,
                speaker,
            });
            added_count += 1;
        }
    }

    if added_count == 0 {
        let trimmed = result.text.trim();
        if !trimmed.is_empty() {
            debug!(
                "Segment {}/{}: {:.1}s, conf={:.2}, speaker={:?}, text='{}'",
                segment_index + 1,
                total_segments,
                segment_duration_sec,
                confidence,
                result.speaker.as_deref(),
                preview_text(trimmed)
            );

            all_transcripts.push(TimedTranscript {
                text: trimmed.to_string(),
                start_ms: segment_start_ms,
                end_ms: segment_end_ms,
                speaker: result.speaker.clone(),
            });
            added_count = 1;
        }
    }

    added_count
}

fn preview_text(text: &str) -> &str {
    if text.len() > 80 {
        let mut end = 80;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    } else {
        text
    }
}

async fn transcribe_retranscription_segment(
    engine: &TranscriptionEngine,
    samples: Vec<f32>,
    language: Option<String>,
    segment_index: usize,
    metadata: Option<TranscriptionRequestMetadata>,
) -> Result<TranscriptResult> {
    match engine {
        TranscriptionEngine::Whisper(whisper_engine) => {
            let (text, confidence, is_partial) = whisper_engine
                .transcribe_audio_with_confidence(samples, language)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Whisper transcription failed on segment {}: {}",
                        segment_index,
                        e
                    )
                })?;
            Ok(TranscriptResult {
                text,
                confidence: Some(confidence),
                is_partial,
                speaker: None,
                segments: Vec::new(),
            })
        }
        TranscriptionEngine::Parakeet(parakeet_engine) => {
            let text = parakeet_engine
                .transcribe_audio(samples)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Parakeet transcription failed on segment {}: {}",
                        segment_index,
                        e
                    )
                })?;
            Ok(TranscriptResult {
                text,
                confidence: None,
                is_partial: false,
                speaker: None,
                segments: Vec::new(),
            })
        }
        TranscriptionEngine::Provider(provider) => provider
            .transcribe_with_metadata(samples, language, metadata)
            .await
            .map_err(|e| {
                anyhow!(
                    "{} transcription failed on segment {}: {}",
                    provider.provider_name(),
                    segment_index,
                    e
                )
            }),
    }
}

/// Emit progress event
fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    stage: &str,
    progress: u32,
    message: &str,
) {
    let _ = app.emit(
        "retranscription-progress",
        RetranscriptionProgress {
            meeting_id: meeting_id.to_string(),
            stage: stage.to_string(),
            progress_percentage: progress,
            message: message.to_string(),
        },
    );
}

/// Get or initialize the Whisper engine, auto-loading the model if needed
/// If `requested_model` is provided, ensures that specific model is loaded
async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<WhisperEngine>> {
    use crate::whisper_engine::commands::WHISPER_ENGINE;

    let engine = {
        let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_whisper_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Whisper model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first (populates the internal cache)
                info!("Discovering available Whisper models...");
                if let Err(discover_err) = e.discover_models().await {
                    warn!(
                        "Error during model discovery (continuing anyway): {}",
                        discover_err
                    );
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Whisper model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!(
                            "Failed to load Whisper model '{}': {}",
                            target_model, load_err
                        );
                        Err(anyhow!(
                            "Failed to load Whisper model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Whisper model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Whisper engine not initialized")),
    }
}

/// Get the configured Whisper model name from the database
async fn get_configured_whisper_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Whisper model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    debug!("Querying transcript_settings table...");

    // Query the transcript settings from the database - get both provider and model
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config: {}", e);
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!(
                "Found transcript config: provider={}, model={}",
                provider, model
            );

            // Check if provider is Whisper-based
            if provider == "localWhisper" || provider == "whisper" {
                Ok(model)
            } else {
                error!(
                    "Retranscription requires Whisper provider, but configured provider is: {}",
                    provider
                );
                Err(anyhow!("Retranscription requires Whisper. Current provider '{}' does not support retranscription with language selection.", provider))
            }
        }
        None => {
            // Default to configured Whisper model if no config exists
            warn!(
                "No transcript config found, using default model '{}'",
                DEFAULT_WHISPER_MODEL
            );
            Ok(DEFAULT_WHISPER_MODEL.to_string())
        }
    }
}

/// Get or initialize the Parakeet engine, auto-loading the model if needed
async fn get_or_init_parakeet<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<ParakeetEngine>> {
    use crate::parakeet_engine::commands::PARAKEET_ENGINE;

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_parakeet_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Parakeet model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first
                info!("Discovering available Parakeet models...");
                if let Err(discover_err) = e.discover_models().await {
                    warn!(
                        "Error during Parakeet model discovery (continuing anyway): {}",
                        discover_err
                    );
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Parakeet model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!(
                            "Failed to load Parakeet model '{}': {}",
                            target_model, load_err
                        );
                        Err(anyhow!(
                            "Failed to load Parakeet model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Parakeet model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Parakeet engine not initialized")),
    }
}

/// Get the configured Parakeet model name from the database
async fn get_configured_parakeet_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Parakeet model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    // Query the transcript settings from the database
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config: {}", e);
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!(
                "Found transcript config: provider={}, model={}",
                provider, model
            );

            if provider == "parakeet" {
                Ok(model)
            } else {
                // Default to configured Parakeet model
                warn!("Configured provider is not Parakeet, using default model");
                Ok(DEFAULT_PARAKEET_MODEL.to_string())
            }
        }
        None => {
            // Default to configured Parakeet model if no config exists
            warn!("No transcript config found, using default Parakeet model");
            Ok(DEFAULT_PARAKEET_MODEL.to_string())
        }
    }
}

/// Write or update metadata.json for retranscription (preserves existing fields, adds retranscribed_at)
fn write_retranscription_metadata(
    folder: &Path,
    meeting_id: &str,
    duration_seconds: f64,
    audio_filename: &str,
) -> Result<()> {
    let metadata_path = folder.join("metadata.json");
    let temp_path = folder.join(".metadata.json.tmp");
    let now = chrono::Utc::now().to_rfc3339();

    // Try to read existing metadata and update it
    let json = if metadata_path.exists() {
        let existing = std::fs::read_to_string(&metadata_path)?;
        let mut value: serde_json::Value = serde_json::from_str(&existing)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("retranscribed_at".to_string(), serde_json::json!(now));
            obj.insert("status".to_string(), serde_json::json!("completed"));
            obj.insert(
                "transcript_file".to_string(),
                serde_json::json!("transcripts.json"),
            );
            obj.remove("detected_summary_language");
        }
        value
    } else {
        serde_json::json!({
            "version": "1.0",
            "meeting_id": meeting_id,
            "created_at": now,
            "completed_at": now,
            "retranscribed_at": now,
            "duration_seconds": duration_seconds,
            "audio_file": audio_filename,
            "transcript_file": "transcripts.json",
            "status": "completed",
            "source": "retranscription"
        })
    };

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &metadata_path)?;

    info!("Wrote metadata.json to {}", metadata_path.display());
    Ok(())
}

// Tauri commands

/// Response when retranscription is started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionStarted {
    pub meeting_id: String,
    pub message: String,
}

// Start retranscription (Beta gated using configContext.betaFeatures)
#[tauri::command]
pub async fn start_retranscription_command<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    vad_preprocessing_enabled: Option<bool>,
) -> Result<RetranscriptionStarted, String> {
    // Check if retranscription is already in progress (guard will be acquired in start_retranscription)
    if RETRANSCRIPTION_IN_PROGRESS.load(Ordering::SeqCst) {
        return Err("Retranscription already in progress".to_string());
    }

    // Clone values for the spawned task
    let meeting_id_clone = meeting_id.clone();

    // Spawn the retranscription in a background task
    tauri::async_runtime::spawn(async move {
        let result = start_retranscription(
            app,
            meeting_id_clone,
            meeting_folder_path,
            language,
            model,
            provider,
            vad_preprocessing_enabled,
        )
        .await;

        // Errors are already emitted as events in start_retranscription
        // so we just log here for debugging
        if let Err(e) = result {
            error!("Retranscription failed: {}", e);
        }
    });

    Ok(RetranscriptionStarted {
        meeting_id,
        message: "Retranscription started".to_string(),
    })
}

#[tauri::command]
pub async fn cancel_retranscription_command() -> Result<(), String> {
    if !is_retranscription_in_progress() {
        return Err("No retranscription in progress".to_string());
    }
    cancel_retranscription();
    Ok(())
}

#[tauri::command]
pub async fn is_retranscription_in_progress_command() -> bool {
    is_retranscription_in_progress()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_transcript_segments_empty() {
        let transcripts: Vec<TimedTranscript> = vec![];
        let segments = create_transcript_segments(&transcripts);
        assert!(segments.is_empty());
    }

    #[test]
    fn test_create_transcript_segments_single() {
        let transcripts = vec![
            TimedTranscript {
                text: "Hello world".to_string(),
                start_ms: 0.0,
                end_ms: 1500.0,
                speaker: None,
            }, // 0-1.5 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(1.5));
        assert_eq!(segments[0].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_multiple() {
        let transcripts = vec![
            TimedTranscript {
                text: "First segment".to_string(),
                start_ms: 0.0,
                end_ms: 2000.0,
                speaker: None,
            }, // 0-2 seconds
            TimedTranscript {
                text: "Second segment".to_string(),
                start_ms: 3000.0,
                end_ms: 5000.0,
                speaker: None,
            }, // 3-5 seconds
            TimedTranscript {
                text: "Third segment".to_string(),
                start_ms: 6500.0,
                end_ms: 8000.0,
                speaker: None,
            }, // 6.5-8 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 3);

        // First segment
        assert_eq!(segments[0].text, "First segment");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(2.0));
        assert_eq!(segments[0].duration, Some(2.0));

        // Second segment
        assert_eq!(segments[1].text, "Second segment");
        assert_eq!(segments[1].audio_start_time, Some(3.0));
        assert_eq!(segments[1].audio_end_time, Some(5.0));
        assert_eq!(segments[1].duration, Some(2.0));

        // Third segment
        assert_eq!(segments[2].text, "Third segment");
        assert_eq!(segments[2].audio_start_time, Some(6.5));
        assert_eq!(segments[2].audio_end_time, Some(8.0));
        assert_eq!(segments[2].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_trims_whitespace() {
        let transcripts = vec![TimedTranscript {
            text: "  Hello with spaces  ".to_string(),
            start_ms: 0.0,
            end_ms: 1000.0,
            speaker: None,
        }];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello with spaces");
    }

    #[test]
    fn test_create_transcript_segments_generates_unique_ids() {
        let transcripts = vec![
            TimedTranscript {
                text: "Segment one".to_string(),
                start_ms: 0.0,
                end_ms: 1000.0,
                speaker: None,
            },
            TimedTranscript {
                text: "Segment two".to_string(),
                start_ms: 1000.0,
                end_ms: 2000.0,
                speaker: None,
            },
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 2);
        assert_ne!(segments[0].id, segments[1].id);
        assert!(segments[0].id.starts_with("transcript-"));
        assert!(segments[1].id.starts_with("transcript-"));
    }

    #[test]
    fn test_cancellation_flag() {
        // Reset flag to known state
        RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);
        RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);

        assert!(!is_retranscription_in_progress());

        // Test cancellation
        cancel_retranscription();
        assert!(RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst));

        // Reset for other tests
        RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_vad_redemption_time_constant() {
        // Batch processing uses 2000ms to bridge natural pauses in full-file VAD
        assert_eq!(VAD_REDEMPTION_TIME_MS, 2000);
    }

    #[test]
    fn test_find_audio_file_common_candidates() {
        let dir = tempfile::tempdir().unwrap();

        // No audio file → error
        assert!(find_audio_file(dir.path()).is_err());

        // Create audio.mp4 — should be found first
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_non_mp4_extensions() {
        let dir = tempfile::tempdir().unwrap();

        // Create audio.wav (imported as .wav, not .mp4)
        std::fs::write(dir.path().join("audio.wav"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.wav");
    }

    #[test]
    fn test_find_audio_file_fallback_scan() {
        let dir = tempfile::tempdir().unwrap();

        // Create a file with an audio extension but non-standard name
        std::fs::write(dir.path().join("my_recording.flac"), b"fake").unwrap();
        // Also add a non-audio file that should be ignored
        std::fs::write(dir.path().join("notes.txt"), b"text").unwrap();

        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "my_recording.flac");
    }

    #[test]
    fn test_find_audio_file_priority_order() {
        let dir = tempfile::tempdir().unwrap();

        // Create both audio.m4a and audio.mp4 — mp4 should win (listed first in candidates)
        std::fs::write(dir.path().join("audio.m4a"), b"fake").unwrap();
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_audio_file(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No audio file found"));
    }

    #[test]
    fn test_find_audio_file_nonexistent_folder() {
        let result = find_audio_file(Path::new("/nonexistent/path/12345"));
        assert!(result.is_err());
    }

    #[test]
    fn test_audio_extensions_constant() {
        // Verify all expected formats are covered
        assert!(AUDIO_EXTENSIONS.contains(&"mp4"));
        assert!(AUDIO_EXTENSIONS.contains(&"m4a"));
        assert!(AUDIO_EXTENSIONS.contains(&"wav"));
        assert!(AUDIO_EXTENSIONS.contains(&"mp3"));
        assert!(AUDIO_EXTENSIONS.contains(&"flac"));
        assert!(AUDIO_EXTENSIONS.contains(&"ogg"));
        assert!(AUDIO_EXTENSIONS.contains(&"aac"));
        // FFmpeg-backed formats
        assert!(AUDIO_EXTENSIONS.contains(&"mkv"));
        assert!(AUDIO_EXTENSIONS.contains(&"webm"));
        assert!(AUDIO_EXTENSIONS.contains(&"wma"));
        // Non-audio formats
        assert!(!AUDIO_EXTENSIONS.contains(&"txt"));
        assert!(!AUDIO_EXTENSIONS.contains(&"pdf"));
    }
}
