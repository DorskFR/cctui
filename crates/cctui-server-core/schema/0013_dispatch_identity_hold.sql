-- A queued Job's dispatcher holds its registration (and the sealed worker key)
-- alive. `key_lifetime` is the key's original lifetime, re-armed from redeem.
ALTER TABLE public.dispatch_identities ADD COLUMN IF NOT EXISTS held_at timestamp with time zone;
ALTER TABLE public.dispatch_identities ADD COLUMN IF NOT EXISTS key_lifetime interval;
