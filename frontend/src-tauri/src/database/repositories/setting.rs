use crate::database::models::{Setting, TranscriptSetting};
use crate::summary::CustomOpenAIConfig;
use sqlx::SqlitePool;

#[derive(serde::Deserialize, Debug)]
pub struct SaveModelConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub struct SaveTranscriptConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "vadPreprocessingEnabled")]
    pub vad_preprocessing_enabled: Option<bool>,
}

pub struct SettingsRepository;

// Transcript providers: localWhisper, parakeet, deepgram, elevenLabs, groq, openai, custom-openai
// Summary providers: openai, claude, ollama, groq, added openrouter
// NOTE: Handle data exclusion in the higher layer as this is database abstraction layer(using SELECT *)

impl SettingsRepository {
    async fn column_exists(
        pool: &SqlitePool,
        table: &str,
        column: &str,
    ) -> std::result::Result<bool, sqlx::Error> {
        let query = format!(
            "SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}' LIMIT 1",
            table, column
        );
        let exists: Option<i64> = sqlx::query_scalar(&query).fetch_optional(pool).await?;
        Ok(exists.is_some())
    }

    async fn ensure_custom_openai_config_column(
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        if !Self::column_exists(pool, "settings", "customOpenAIConfig").await? {
            sqlx::query("ALTER TABLE settings ADD COLUMN customOpenAIConfig TEXT")
                .execute(pool)
                .await?;
        }
        Ok(())
    }

    async fn ensure_transcript_custom_openai_config_column(
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        if !Self::column_exists(pool, "transcript_settings", "customOpenAIConfig").await? {
            sqlx::query("ALTER TABLE transcript_settings ADD COLUMN customOpenAIConfig TEXT")
                .execute(pool)
                .await?;
        }
        Ok(())
    }

    async fn ensure_transcript_vad_preprocessing_enabled_column(
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        if !Self::column_exists(pool, "transcript_settings", "vadPreprocessingEnabled").await? {
            sqlx::query(
                "ALTER TABLE transcript_settings ADD COLUMN vadPreprocessingEnabled INTEGER NOT NULL DEFAULT 1",
            )
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn get_model_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<Setting>, sqlx::Error> {
        let setting = sqlx::query_as::<_, Setting>("SELECT * FROM settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
        Ok(setting)
    }

    pub async fn save_model_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
        whisper_model: &str,
        ollama_endpoint: Option<&str>,
    ) -> std::result::Result<(), sqlx::Error> {
        // Using id '1' for backward compatibility
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, ollamaEndpoint)
            VALUES ('1', $1, $2, $3, $4)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                whisperModel = excluded.whisperModel,
                ollamaEndpoint = excluded.ollamaEndpoint
            "#,
        )
        .bind(provider)
        .bind(model)
        .bind(whisper_model)
        .bind(ollama_endpoint)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI uses JSON config (customOpenAIConfig) instead of a separate API key column
        if provider == "custom-openai" {
            return Err(sqlx::Error::Protocol(
                "custom-openai provider should use save_custom_openai_config() instead of save_api_key()".into(),
            ));
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "claude" => "anthropicApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, "{}")
            VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column, api_key_column
        );
        sqlx::query(&query).bind(api_key).execute(pool).await?;

        Ok(())
    }

    pub async fn get_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        // Custom OpenAI uses JSON config - extract API key from there
        if provider == "custom-openai" {
            let config = Self::get_custom_openai_config(pool).await?;
            return Ok(config.and_then(|c| c.api_key));
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(None), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "SELECT {} FROM settings WHERE id = '1' LIMIT 1",
            api_key_column
        );
        let api_key = sqlx::query_scalar(&query).fetch_optional(pool).await?;
        Ok(api_key)
    }

    pub async fn get_transcript_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<TranscriptSetting>, sqlx::Error> {
        Self::ensure_transcript_vad_preprocessing_enabled_column(pool).await?;
        Self::ensure_transcript_custom_openai_config_column(pool).await?;

        let setting =
            sqlx::query_as::<_, TranscriptSetting>("SELECT * FROM transcript_settings LIMIT 1")
                .fetch_optional(pool)
                .await?;
        Ok(setting)
    }

    pub async fn save_transcript_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
        vad_preprocessing_enabled: Option<bool>,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::ensure_transcript_vad_preprocessing_enabled_column(pool).await?;

        sqlx::query(
            r#"
            INSERT INTO transcript_settings (id, provider, model, vadPreprocessingEnabled)
            VALUES ('1', $1, $2, COALESCE($3, 1))
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                vadPreprocessingEnabled = COALESCE($3, transcript_settings.vadPreprocessingEnabled)
            "#,
        )
        .bind(provider)
        .bind(model)
        .bind(vad_preprocessing_enabled)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" => return Ok(()), // Parakeet doesn't need an API key, return early
            "custom-openai" => return Ok(()), // Custom OpenAI stores API key in transcript customOpenAIConfig
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO transcript_settings (id, provider, model, "{}")
            VALUES ('1', 'parakeet', '{}', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column,
            crate::config::DEFAULT_PARAKEET_MODEL,
            api_key_column
        );
        sqlx::query(&query).bind(api_key).execute(pool).await?;

        Ok(())
    }

    pub async fn get_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" => return Ok(None), // Parakeet doesn't need an API key
            "custom-openai" => {
                let config = Self::get_transcript_custom_openai_config(pool).await?;
                return Ok(config.and_then(|cfg| cfg.api_key));
            }
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "SELECT {} FROM transcript_settings WHERE id = '1' LIMIT 1",
            api_key_column
        );
        let api_key = sqlx::query_scalar(&query).fetch_optional(pool).await?;
        Ok(api_key)
    }

    pub async fn delete_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI uses JSON config - clear the entire config
        if provider == "custom-openai" {
            sqlx::query("UPDATE settings SET customOpenAIConfig = NULL WHERE id = '1'")
                .execute(pool)
                .await?;
            return Ok(());
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "UPDATE settings SET {} = NULL WHERE id = '1'",
            api_key_column
        );
        sqlx::query(&query).execute(pool).await?;

        Ok(())
    }

    // ===== CUSTOM OPENAI CONFIG METHODS =====

    /// Gets the custom OpenAI configuration from JSON
    ///
    /// # Returns
    /// * `Ok(Some(CustomOpenAIConfig))` - Config exists and is valid JSON
    /// * `Ok(None)` - No config stored
    /// * `Err(sqlx::Error)` - Database error
    pub async fn get_custom_openai_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<CustomOpenAIConfig>, sqlx::Error> {
        use sqlx::Row;

        Self::ensure_custom_openai_config_column(pool).await?;

        let row = sqlx::query(
            r#"
            SELECT customOpenAIConfig
            FROM settings
            WHERE id = '1'
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await?;

        match row {
            Some(record) => {
                let config_json: Option<String> = record.get("customOpenAIConfig");

                if let Some(json) = config_json {
                    // Parse JSON into CustomOpenAIConfig
                    let config: CustomOpenAIConfig = serde_json::from_str(&json).map_err(|e| {
                        sqlx::Error::Protocol(
                            format!("Invalid JSON in customOpenAIConfig: {}", e).into(),
                        )
                    })?;

                    Ok(Some(config))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Saves the custom OpenAI configuration as JSON
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `config` - CustomOpenAIConfig to save (includes endpoint, apiKey, model, maxTokens, temperature, topP)
    ///
    /// # Returns
    /// * `Ok(())` - Config saved successfully
    /// * `Err(sqlx::Error)` - Database or JSON serialization error
    pub async fn save_custom_openai_config(
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::ensure_custom_openai_config_column(pool).await?;

        // Serialize config to JSON
        let config_json = serde_json::to_string(config).map_err(|e| {
            sqlx::Error::Protocol(format!("Failed to serialize config to JSON: {}", e).into())
        })?;

        // Upsert into settings table
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, customOpenAIConfig)
            VALUES ('1', 'custom-openai', $1, 'large-v3', $2)
            ON CONFLICT(id) DO UPDATE SET
                customOpenAIConfig = excluded.customOpenAIConfig
            "#,
        )
        .bind(&config.model)
        .bind(config_json)
        .execute(pool)
        .await?;

        Ok(())
    }

    // ===== TRANSCRIPT CUSTOM OPENAI CONFIG METHODS =====

    /// Gets the transcription Custom OpenAI configuration from JSON
    pub async fn get_transcript_custom_openai_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<CustomOpenAIConfig>, sqlx::Error> {
        use sqlx::Row;

        Self::ensure_transcript_custom_openai_config_column(pool).await?;

        let row = sqlx::query(
            r#"
            SELECT customOpenAIConfig
            FROM transcript_settings
            WHERE id = '1'
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await?;

        match row {
            Some(record) => {
                let config_json: Option<String> = record.get("customOpenAIConfig");

                if let Some(json) = config_json {
                    let config: CustomOpenAIConfig = serde_json::from_str(&json).map_err(|e| {
                        sqlx::Error::Protocol(
                            format!("Invalid JSON in transcript customOpenAIConfig: {}", e).into(),
                        )
                    })?;

                    Ok(Some(config))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Saves the transcription Custom OpenAI configuration as JSON
    pub async fn save_transcript_custom_openai_config(
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::ensure_transcript_custom_openai_config_column(pool).await?;

        let config_json = serde_json::to_string(config).map_err(|e| {
            sqlx::Error::Protocol(
                format!("Failed to serialize transcript config to JSON: {}", e).into(),
            )
        })?;

        sqlx::query(
            r#"
            INSERT INTO transcript_settings (id, provider, model, customOpenAIConfig)
            VALUES ('1', 'custom-openai', $1, $2)
            ON CONFLICT(id) DO UPDATE SET
                customOpenAIConfig = excluded.customOpenAIConfig
            "#,
        )
        .bind(&config.model)
        .bind(config_json)
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_legacy_transcript_settings_table(pool: &SqlitePool) {
        sqlx::query(
            r#"
            CREATE TABLE transcript_settings (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                whisperApiKey TEXT,
                deepgramApiKey TEXT,
                elevenLabsApiKey TEXT,
                groqApiKey TEXT,
                openaiApiKey TEXT
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_save_transcript_config_defaults_vad_enabled() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_legacy_transcript_settings_table(&pool).await;

        SettingsRepository::save_transcript_config(
            &pool,
            "parakeet",
            crate::config::DEFAULT_PARAKEET_MODEL,
            None,
        )
        .await
        .unwrap();

        let config = SettingsRepository::get_transcript_config(&pool)
            .await
            .unwrap()
            .unwrap();

        assert!(config.vad_preprocessing_enabled);
    }

    #[tokio::test]
    async fn test_save_transcript_config_preserves_vad_when_unspecified() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_legacy_transcript_settings_table(&pool).await;

        SettingsRepository::save_transcript_config(
            &pool,
            "parakeet",
            crate::config::DEFAULT_PARAKEET_MODEL,
            Some(false),
        )
        .await
        .unwrap();

        SettingsRepository::save_transcript_config(
            &pool,
            "localWhisper",
            crate::config::DEFAULT_WHISPER_MODEL,
            None,
        )
        .await
        .unwrap();

        let config = SettingsRepository::get_transcript_config(&pool)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(config.provider, "localWhisper");
        assert!(!config.vad_preprocessing_enabled);
    }

    #[tokio::test]
    async fn test_save_transcript_custom_openai_config_preserves_chunk_metadata_toggle() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_legacy_transcript_settings_table(&pool).await;

        let config = CustomOpenAIConfig {
            endpoint: "http://localhost:8000/v1".to_string(),
            api_key: None,
            model: "whisper-1".to_string(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            transcription_api: Some("audio".to_string()),
            transcription_prompt: None,
            send_chunk_metadata_fields: true,
        };

        SettingsRepository::save_transcript_custom_openai_config(&pool, &config)
            .await
            .unwrap();

        let saved = SettingsRepository::get_transcript_custom_openai_config(&pool)
            .await
            .unwrap()
            .unwrap();

        assert!(saved.send_chunk_metadata_fields);
    }
}
