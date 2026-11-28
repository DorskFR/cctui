-- Who proved they hold a blob's bytes by uploading it: a daemon event may only
-- link a hash its machine's user uploaded.
CREATE TABLE public.blob_uploaders (
    hash    text NOT NULL REFERENCES public.daemon_blobs(hash) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    PRIMARY KEY (hash, user_id)
);
CREATE INDEX blob_uploaders_user_idx ON public.blob_uploaders (user_id);

-- Hashes already linked stay linkable by the sessions' owners.
INSERT INTO public.blob_uploaders (hash, user_id)
SELECT DISTINCT l.hash, s.user_id
FROM public.session_blob_links l
JOIN public.sessions s ON s.id = l.session_id
JOIN public.daemon_blobs b ON b.hash = l.hash
JOIN public.users u ON u.id = s.user_id
ON CONFLICT DO NOTHING;
