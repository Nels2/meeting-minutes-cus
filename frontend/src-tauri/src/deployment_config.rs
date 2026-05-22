use crate::calendar::{
    apply_deployed_calendar_settings, default_calendar_redirect_uri, default_calendar_scopes,
    O365CalendarSettings,
};
use crate::database::models::TranscriptSetting;
use crate::database::repositories::setting::SettingsRepository;
use crate::summary::CustomOpenAIConfig;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

const DEPLOYMENT_CONFIG_FILE: &str = "deployment_config.json";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DeploymentMode {
    Seed,
    Managed,
}

impl Default for DeploymentMode {
    fn default() -> Self {
        Self::Seed
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentConfig {
    #[allow(dead_code)]
    version: Option<u32>,
    calendar: Option<CalendarDeploymentConfig>,
    transcription: Option<TranscriptionDeploymentConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarDeploymentConfig {
    #[serde(default)]
    mode: DeploymentMode,
    tenant_id: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    scopes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptionDeploymentConfig {
    #[serde(default)]
    mode: DeploymentMode,
    provider: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    custom_openai: Option<TranscriptCustomOpenAIDeploymentConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptCustomOpenAIDeploymentConfig {
    endpoint: Option<String>,
    transcription_api: Option<String>,
    transcription_prompt: Option<String>,
}

fn deployment_config_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {}", e))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create app data directory: {}", e))?;
    Ok(dir.join(DEPLOYMENT_CONFIG_FILE))
}

fn read_deployment_config<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<DeploymentConfig>, String> {
    let path = deployment_config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let config = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    Ok(Some(config))
}

fn required_trimmed(value: &Option<String>, field: &str) -> Result<String, String> {
    let value = value.as_deref().unwrap_or_default().trim().to_string();
    if value.is_empty() {
        Err(format!("deployment_config.json is missing {}", field))
    } else {
        Ok(value)
    }
}

fn optional_trimmed(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn calendar_settings_from_deployment(
    config: &CalendarDeploymentConfig,
) -> Result<O365CalendarSettings, String> {
    Ok(O365CalendarSettings {
        tenant_id: required_trimmed(&config.tenant_id, "calendar.tenantId")?,
        client_id: required_trimmed(&config.client_id, "calendar.clientId")?,
        redirect_uri: optional_trimmed(&config.redirect_uri)
            .unwrap_or_else(|| default_calendar_redirect_uri().to_string()),
        scopes: optional_trimmed(&config.scopes)
            .unwrap_or_else(|| default_calendar_scopes().to_string()),
    })
}

fn should_apply_transcription(existing: Option<&TranscriptSetting>, mode: DeploymentMode) -> bool {
    match mode {
        DeploymentMode::Managed => true,
        DeploymentMode::Seed => existing
            .map(|setting| setting.provider.trim().is_empty() || setting.model.trim().is_empty())
            .unwrap_or(true),
    }
}

fn validate_transcription_provider(provider: &str) -> Result<(), String> {
    match provider {
        "localWhisper" | "parakeet" | "custom-openai" | "deepgram" | "elevenLabs" | "groq"
        | "openai" => Ok(()),
        _ => Err(format!(
            "deployment_config.json transcription.provider '{}' is not supported",
            provider
        )),
    }
}

fn resolve_api_key_from_env(api_key_env: Option<&str>) -> Option<String> {
    let name = api_key_env?.trim();
    if name.is_empty() {
        return None;
    }

    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => {
            log::warn!(
                "deployment_config.json references API key env var '{}' but it is not set",
                name
            );
            None
        }
    }
}

async fn apply_calendar<R: Runtime>(
    app: &AppHandle<R>,
    config: &CalendarDeploymentConfig,
) -> Result<(), String> {
    let settings = calendar_settings_from_deployment(config)?;
    let applied =
        apply_deployed_calendar_settings(app, settings, config.mode == DeploymentMode::Managed)?;

    if applied {
        log::info!(
            "Applied Microsoft Calendar deployment config in {:?} mode",
            config.mode
        );
    }

    Ok(())
}

async fn apply_transcription(
    pool: &SqlitePool,
    config: &TranscriptionDeploymentConfig,
) -> Result<(), String> {
    let provider = required_trimmed(&config.provider, "transcription.provider")?;
    let model = required_trimmed(&config.model, "transcription.model")?;
    validate_transcription_provider(&provider)?;

    if provider == "custom-openai" && config.custom_openai.is_none() {
        return Err(
            "deployment_config.json transcription.customOpenAI is required for custom-openai"
                .to_string(),
        );
    }

    if provider != "custom-openai" && config.custom_openai.is_some() {
        return Err(
            "deployment_config.json transcription.customOpenAI can only be used with custom-openai"
                .to_string(),
        );
    }

    let existing = SettingsRepository::get_transcript_config(pool)
        .await
        .map_err(|e| format!("Failed to read existing transcription config: {}", e))?;
    if !should_apply_transcription(existing.as_ref(), config.mode) {
        return Ok(());
    }

    SettingsRepository::save_transcript_config(pool, &provider, &model)
        .await
        .map_err(|e| format!("Failed to save deployed transcription config: {}", e))?;

    let deployed_api_key = resolve_api_key_from_env(config.api_key_env.as_deref());

    if provider == "custom-openai" {
        let existing_custom = SettingsRepository::get_transcript_custom_openai_config(pool)
            .await
            .map_err(|e| format!("Failed to read existing custom transcription config: {}", e))?;
        let custom = config.custom_openai.as_ref().expect("checked above");
        let endpoint = required_trimmed(&custom.endpoint, "transcription.customOpenAI.endpoint")?;

        SettingsRepository::save_transcript_custom_openai_config(
            pool,
            &CustomOpenAIConfig {
                endpoint,
                api_key: deployed_api_key
                    .or_else(|| existing_custom.as_ref().and_then(|cfg| cfg.api_key.clone())),
                model,
                max_tokens: existing_custom.as_ref().and_then(|cfg| cfg.max_tokens),
                temperature: existing_custom.as_ref().and_then(|cfg| cfg.temperature),
                top_p: existing_custom.as_ref().and_then(|cfg| cfg.top_p),
                transcription_api: optional_trimmed(&custom.transcription_api)
                    .or_else(|| {
                        existing_custom
                            .as_ref()
                            .and_then(|cfg| cfg.transcription_api.clone())
                    })
                    .or_else(|| Some("audio".to_string())),
                transcription_prompt: optional_trimmed(&custom.transcription_prompt).or_else(
                    || {
                        existing_custom
                            .as_ref()
                            .and_then(|cfg| cfg.transcription_prompt.clone())
                    },
                ),
            },
        )
        .await
        .map_err(|e| format!("Failed to save deployed custom transcription config: {}", e))?;
    } else if let Some(api_key) = deployed_api_key {
        SettingsRepository::save_transcript_api_key(pool, &provider, &api_key)
            .await
            .map_err(|e| format!("Failed to save deployed transcription API key: {}", e))?;
    }

    log::info!(
        "Applied transcription deployment config in {:?} mode for provider '{}'",
        config.mode,
        provider
    );
    Ok(())
}

pub async fn apply_deployment_config<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
) -> Result<(), String> {
    let Some(config) = read_deployment_config(app)? else {
        return Ok(());
    };

    if let Some(calendar) = config.calendar.as_ref() {
        apply_calendar(app, calendar).await?;
    }

    if let Some(transcription) = config.transcription.as_ref() {
        apply_transcription(pool, transcription).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{resolve_api_key_from_env, should_apply_transcription, DeploymentMode};
    use crate::database::models::TranscriptSetting;

    fn transcript_setting(provider: &str, model: &str) -> TranscriptSetting {
        TranscriptSetting {
            id: "1".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            whisper_api_key: Some("existing-whisper-key".to_string()),
            deepgram_api_key: None,
            eleven_labs_api_key: None,
            groq_api_key: None,
            openai_api_key: None,
            custom_openai_config: None,
        }
    }

    #[test]
    fn seed_transcription_applies_only_when_missing() {
        let existing = transcript_setting("parakeet", crate::config::DEFAULT_PARAKEET_MODEL);
        let blank_provider = transcript_setting("", crate::config::DEFAULT_PARAKEET_MODEL);

        assert!(!should_apply_transcription(
            Some(&existing),
            DeploymentMode::Seed
        ));
        assert!(should_apply_transcription(None, DeploymentMode::Seed));
        assert!(should_apply_transcription(
            Some(&blank_provider),
            DeploymentMode::Seed
        ));
    }

    #[test]
    fn managed_transcription_always_applies() {
        let existing = transcript_setting("parakeet", crate::config::DEFAULT_PARAKEET_MODEL);

        assert!(should_apply_transcription(
            Some(&existing),
            DeploymentMode::Managed
        ));
    }

    #[test]
    fn blank_or_missing_env_preserves_existing_api_key() {
        std::env::remove_var("MEETILY_TEST_MISSING_KEY");
        std::env::set_var("MEETILY_TEST_BLANK_KEY", "   ");

        assert_eq!(resolve_api_key_from_env(None), None);
        assert_eq!(
            resolve_api_key_from_env(Some("MEETILY_TEST_MISSING_KEY")),
            None
        );
        assert_eq!(
            resolve_api_key_from_env(Some("MEETILY_TEST_BLANK_KEY")),
            None
        );

        std::env::remove_var("MEETILY_TEST_BLANK_KEY");
    }

    #[test]
    fn env_api_key_is_trimmed_when_present() {
        std::env::set_var("MEETILY_TEST_PRESENT_KEY", "  test-key  ");

        assert_eq!(
            resolve_api_key_from_env(Some("MEETILY_TEST_PRESENT_KEY")),
            Some("test-key".to_string())
        );

        std::env::remove_var("MEETILY_TEST_PRESENT_KEY");
    }
}
