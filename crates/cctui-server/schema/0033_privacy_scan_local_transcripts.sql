ALTER TABLE privacy_scan_jobs ADD COLUMN local_transcripts JSONB;

ALTER TABLE machines ADD COLUMN transcript_audit BOOLEAN NOT NULL DEFAULT false;
