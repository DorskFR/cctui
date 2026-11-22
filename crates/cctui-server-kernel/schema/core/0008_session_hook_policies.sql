-- Effective hook policy of a session launched without a machine_launches row
-- (dispatched workers): {"rules": [...], "status": "applied"|"unsupported"}.
CREATE TABLE IF NOT EXISTS public.session_hook_policies (
    session_id text PRIMARY KEY,
    policy jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
