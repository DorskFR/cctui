-- Pending "Sign in with Claude/ChatGPT" logins: the PKCE verifier between
-- oauth/start and oauth/finish, readable from every replica. Single-use.
CREATE TABLE IF NOT EXISTS public.oauth_pending_logins (
    nonce text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    provider text NOT NULL,
    account_id uuid REFERENCES public.accounts(id) ON DELETE CASCADE,
    code_verifier text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS oauth_pending_logins_created_at_idx
    ON public.oauth_pending_logins (created_at);
