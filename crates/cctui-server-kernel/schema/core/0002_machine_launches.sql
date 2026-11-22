-- Durable identity for machine launches before the daemon registers a session.
-- Prompts, environment values and bearer credentials are deliberately absent.
CREATE TABLE public.machine_launches (
    command_id uuid PRIMARY KEY,
    session_id text NOT NULL UNIQUE,
    registered_session_id text,
    machine_id uuid NOT NULL REFERENCES public.machines(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    acting_user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    adapter_id text NOT NULL,
    working_dir text NOT NULL,
    name text,
    model text,
    effort text,
    state text NOT NULL DEFAULT 'prepared'
        CHECK (state IN ('prepared','delivered','accepted','refused','delivery_failed','registered')),
    detail text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX machine_launches_owner_created ON public.machine_launches (acting_user_id, created_at);
