-- Add separate Custom OpenAI config for transcription settings
ALTER TABLE transcript_settings ADD COLUMN customOpenAIConfig TEXT;
