-- CCT-429: persist the opaque per-dispatcher handle for a dispatched session so
-- the completion-webhook sweep can later ask the owning dispatcher whether the
-- workload is still alive (Running), finished cleanly (Complete — the worker's
-- own REPLY_URL callback owns the verdict), or died without a conclusion
-- (Failed/Gone — crashloop / OOM / unschedulable / vanished), and fire the
-- server's lifecycle-only death callback only in that last case.
--
-- The dispatcher is the liveness authority; the verdict itself never lands here.
CREATE TABLE IF NOT EXISTS dispatch_handles (
    session_id      TEXT PRIMARY KEY,
    -- Dispatcher name as targeted at dispatch time; re-resolved per-owner in the
    -- sweep via the same lookup the dispatch route uses (owner comes from the
    -- joined session_webhooks row), so no dispatcher_id is stored here.
    dispatcher_name TEXT        NOT NULL,
    -- Opaque per-dispatcher reference, e.g. `jobs/claude-worker-…` /
    -- `container/cctui-worker-…`.
    handle          TEXT        NOT NULL,
    namespace       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
