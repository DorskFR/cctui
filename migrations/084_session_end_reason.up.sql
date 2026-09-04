-- Why a session ended, persisted on the row so list/detail reads do not have
-- to dig it out of stream_events. `end_reason` is one of completed | killed |
-- crashed | daemon_lost | machine_offline | reaped_inactive | resume_failed |
-- other; `end_detail` is the adapter's diagnostic (exit status, stderr tail),
-- capped at 2 KiB by the writer.
ALTER TABLE sessions
    ADD COLUMN ended_at   TIMESTAMPTZ,
    ADD COLUMN end_reason TEXT,
    ADD COLUMN end_detail TEXT;
