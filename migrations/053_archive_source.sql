-- Distinguish directly-synced transcripts (uploaded byte-exact by the daemon,
-- CCT-362) from server-rebuilt ones (reconstructed from `stream_events` when no
-- file was ever uploaded, CCT-363). Export (CCT-364) prefers `synced` over
-- `rebuilt` and labels each entry's provenance.
ALTER TABLE archive_index ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'synced';
