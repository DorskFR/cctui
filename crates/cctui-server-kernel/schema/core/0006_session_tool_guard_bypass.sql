-- Who lifted the gateway tool guard for one session. NULL keeps it active; the
-- gateway honours it only while the setter is an admin or owns the account.
ALTER TABLE public.sessions
    ADD COLUMN IF NOT EXISTS tool_guard_bypass_by uuid REFERENCES public.users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS tool_guard_bypass_at timestamp with time zone;
