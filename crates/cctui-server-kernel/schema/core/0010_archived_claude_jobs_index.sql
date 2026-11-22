-- Serves the heartbeat lookup of archived claude jobs by 8-char id prefix.
-- Its WHERE clause must keep matching this predicate and expression.
CREATE INDEX IF NOT EXISTS idx_sessions_archived_claude_short
    ON public.sessions USING btree (machine_uuid, left(id, 8))
    WHERE status = 'archived' AND adapter_id = 'claude-code';
