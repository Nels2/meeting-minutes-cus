-- Add batch VAD preprocessing toggle for import/retranscription.
ALTER TABLE transcript_settings ADD COLUMN vadPreprocessingEnabled INTEGER NOT NULL DEFAULT 1;
