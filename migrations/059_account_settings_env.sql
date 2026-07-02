-- CCT-538: per-account defaults + settings/env overrides.
--
-- An account can carry launch defaults and two override blobs applied when a
-- session runs under it:
--
--   * `settings_json`  — a validated subset of harness settings pasted by the
--                        operator (allowlisted server-side: MANAGED/SYSTEM keys
--                        are rejected before persist). Config, not secret →
--                        returned normally over the API. JSONB object.
--   * `env_json`       — extra environment variables for the session. May hold
--                        secrets, so it is stored ENCRYPTED at rest (crate::crypto,
--                        same vault key as the OAuth tokens) and is WRITE-ONLY:
--                        never returned over the API, exactly like the tokens.
--                        Encrypted ciphertext lives in a TEXT column.
--   * `default_model`  — default `--model` code when a session omits one.
--   * `default_effort` — default reasoning effort when a session omits one.
--   * `default_permission_mode` — default permission mode when a session omits one.
--
-- All nullable. NULL/unset ⇒ prior behaviour (no override / no default).

ALTER TABLE oauth_accounts
    ADD COLUMN IF NOT EXISTS settings_json           JSONB,
    ADD COLUMN IF NOT EXISTS env_json                TEXT,
    ADD COLUMN IF NOT EXISTS default_model           TEXT,
    ADD COLUMN IF NOT EXISTS default_effort          TEXT,
    ADD COLUMN IF NOT EXISTS default_permission_mode TEXT;
