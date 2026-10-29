-- Durable host decisions survive worker reconnects and credential recovery.
CREATE TABLE public.execution_stops (
    session_id text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    machine_id uuid REFERENCES public.machines(id) ON DELETE CASCADE,
    end_reason text NOT NULL,
    detail text,
    created_at timestamptz NOT NULL DEFAULT now()
);
