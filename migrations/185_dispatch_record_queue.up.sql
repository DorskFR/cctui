-- A dispatch held in its dispatcher's queue is not late to register; the
-- registration clock starts when the worker does.
ALTER TABLE dispatch_records
    ADD COLUMN IF NOT EXISTS queued BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS started_at TIMESTAMPTZ;
