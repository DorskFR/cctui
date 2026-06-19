-- CCT-406: per-account model alias map.
--
-- The same logical model name resolves to different concrete models depending
-- on the account. Example: a work Anthropic account's "opus" is the 1M-context
-- build (claude-opus-4-8[1m]) while a personal account only has the 200k one.
--
--   * `model_aliases` — JSON object mapping a logical/family name to the concrete
--     `--model` code the harness should launch, e.g.
--       {"opus": "claude-opus-4-8[1m]", "sonnet": "claude-sonnet-4-6"}
--     Applies to ALL providers (native + `-compatible`), unlike `models` (which
--     lists selectable codes for compatible endpoints only). NULL/empty means no
--     remapping — the model string passes through unchanged. Safe to return over
--     the API; model names aren't secret.
--
-- Resolution happens server-side at spawn (routes/spawn.rs): when a session
-- picks a named account, its `model` is looked up in this map before the spec is
-- dispatched, so every client (webui, TUI, admin) benefits without client code.

ALTER TABLE oauth_accounts
    ADD COLUMN IF NOT EXISTS model_aliases JSONB;
