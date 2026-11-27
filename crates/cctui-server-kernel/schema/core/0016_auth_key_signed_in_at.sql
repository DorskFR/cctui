-- When the holder of this key last signed in interactively (a passkey assertion
-- or a token typed into the web UI). Adding a passkey needs a recent one.
ALTER TABLE public.auth_keys ADD COLUMN IF NOT EXISTS signed_in_at timestamptz;
UPDATE public.auth_keys SET signed_in_at = created_at WHERE kind = 'passkey' AND signed_in_at IS NULL;
