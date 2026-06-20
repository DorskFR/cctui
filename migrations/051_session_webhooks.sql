-- CCT-294: server-emitted completion webhooks.
--
-- When a dispatch carries `notify_url`, the server registers a pending webhook
-- here (one row per dispatched session). A background sweep (in `reaper_task`)
-- detects when the session reaches a TERMINAL state — `ended` (SessionEnded
-- from the daemon: completed/killed/crashed), `failed` (dispatch never
-- launched), or `archived` (silence past the TTL, i.e. connection lost > grace
-- or the daemon never connected) — builds the automation-contract payload, and POSTs
-- it with exponential backoff. This is the crash-coverage path: the worker's
-- REPLY_URL exit trap can miss an OOM/SIGKILL, but the server-side terminal
-- detection cannot. REPLY_URL remains additive during migration.
--
-- A row is a single delivery attempt-set: `state` walks pending -> sent | dead.
-- `next_attempt_at` gates the backoff; `attempts` bounds retries before
-- dead-lettering. `secret` (when present) keys the HMAC-SHA256 signature header
-- so receivers verify origin. The body of the delivered payload is captured in
-- `payload` once the session goes terminal so it survives a server restart.

CREATE TABLE IF NOT EXISTS session_webhooks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      TEXT NOT NULL,
    -- The owning user (for scoping / audit). Dispatched sessions always carry
    -- one; admin-token dispatches do not register a webhook (no owning user).
    user_id         UUID REFERENCES users(id) ON DELETE CASCADE,
    -- Delivery target. A bearer-ish capability URL (e.g. an automation resume URL).
    notify_url      TEXT NOT NULL,
    -- Optional per-target HMAC secret. When set the delivery carries an
    -- `X-CCTUI-Signature: sha256=<hex>` header over the raw JSON body.
    secret          TEXT,
    -- Task id echoed back in the payload so the receiver correlates the run.
    -- Defaults to the session id when the dispatch payload carried no task_id.
    task_id         TEXT NOT NULL,
    -- Delivery lifecycle: 'pending' (awaiting terminal state or a retry),
    -- 'sent' (delivered 2xx), 'dead' (exhausted retries — dead-letter).
    state           TEXT NOT NULL DEFAULT 'pending',
    -- The frozen payload, captured when the session went terminal. NULL while
    -- still waiting for the session to finish.
    payload         JSONB,
    attempts        INTEGER NOT NULL DEFAULT 0,
    -- Backoff gate: the sweep only attempts delivery once now() >= this.
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at         TIMESTAMPTZ,
    -- One webhook registration per session (idempotent re-dispatch).
    UNIQUE (session_id)
);

-- The sweep query filters on undelivered rows due for an attempt.
CREATE INDEX IF NOT EXISTS idx_session_webhooks_pending
    ON session_webhooks (state, next_attempt_at)
    WHERE state = 'pending';
