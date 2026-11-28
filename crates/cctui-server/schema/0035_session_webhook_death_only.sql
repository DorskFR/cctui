-- A completion webhook armed on a dispatch's reply_url only reports a death:
-- the worker's own callback owns every other verdict, and a declared outcome
-- is never posted to it.
ALTER TABLE public.session_webhooks
    ADD COLUMN IF NOT EXISTS death_only boolean NOT NULL DEFAULT false;
