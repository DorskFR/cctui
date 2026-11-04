-- Users and machines whose sessions a viewer keeps out of their own session lists.
CREATE TABLE IF NOT EXISTS public.user_hidden_owners (
    viewer_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('user', 'machine')),
    target_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (viewer_id, kind, target_id)
);
