-- CCT-399: first-class "compatible-endpoint" accounts.
--
-- Extend the OAuth account vault so a row can also describe an arbitrary
-- OpenAI-/Anthropic-compatible API (LiteLLM, vLLM, Ollama-via-proxy, OpenRouter,
-- LM Studio, …) instead of only a native subscription account. This retires the
-- server-global `CCTUI_CLAUDE_LITELLM_*` env-var hack (#202): a compatible
-- endpoint is now just another account, reusing the same vault encryption,
-- gateway passthrough, per-account metering, and ownership model.
--
--   * `provider` is widened from {anthropic, openai} to also allow
--     {anthropic-compatible, openai-compatible}. The provider *family*
--     (anthropic vs openai) still drives which harness/env is used; the
--     `-compatible` suffix marks a static-credential, base-url-overridden
--     endpoint that SKIPS the OAuth refresh path.
--   * `base_url`   — the compatible endpoint (NULL for native subscription
--     accounts, which use the built-in upstream).
--   * `models`     — JSON array of {model,label}: the account's selectable
--     models (replaces the global CCTUI_CLAUDE_LITELLM_MODELS).
--   * `auth_scheme`— 'oauth' (refreshing subscription account, the default for
--     existing rows) | 'bearer' | 'api_key' (static credential, no refresh).
--   * `managed`    — TRUE for a server-synthesized account (the one-release
--     back-compat shim for CCTUI_CLAUDE_LITELLM_*). Read-only over the API.

ALTER TABLE oauth_accounts
    ADD COLUMN IF NOT EXISTS base_url    TEXT,
    ADD COLUMN IF NOT EXISTS models      JSONB,
    ADD COLUMN IF NOT EXISTS auth_scheme TEXT NOT NULL DEFAULT 'oauth',
    ADD COLUMN IF NOT EXISTS managed     BOOLEAN NOT NULL DEFAULT FALSE;

-- Existing rows are subscription OAuth accounts: encrypted_refresh_token is NOT
-- NULL today. Compatible accounts store only a static credential (in
-- encrypted_access_token) with no refresh token, so relax that constraint.
ALTER TABLE oauth_accounts
    ALTER COLUMN encrypted_refresh_token DROP NOT NULL;

-- A managed (server-synthesized) account is keyed only by its provider per
-- user, so the back-compat shim is idempotent on restart.
CREATE UNIQUE INDEX IF NOT EXISTS oauth_accounts_managed_user_provider
    ON oauth_accounts (user_id, provider)
    WHERE managed;
