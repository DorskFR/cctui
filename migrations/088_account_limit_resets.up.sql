-- Every "reset my usage limit" claim made on behalf of a provider credential
-- (Codex "Redeem usage limit reset", Claude Code "/limit-reset"). Audit trail
-- and retry safety: a repeat click finds the prior row and reuses its
-- idempotency key instead of issuing a second consume request upstream.
CREATE TABLE account_limit_resets (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id     UUID        NOT NULL REFERENCES account_providers(id) ON DELETE CASCADE,
    idempotency_key TEXT        NOT NULL,
    -- Codex credit id when the claim named one; NULL for Claude (single program).
    credit_id       TEXT,
    -- Upstream outcome verbatim (reset | already_redeemed | nothing_to_reset |
    -- no_credit | already_used | not_limited | ineligible | unavailable | error).
    outcome         TEXT        NOT NULL,
    requested_by    UUID        REFERENCES users(id) ON DELETE SET NULL,
    at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX account_limit_resets_provider_at ON account_limit_resets (provider_id, at DESC);

-- Anthropic's reset endpoint is addressed by organization; learned from the
-- OAuth token / profile payload and remembered here.
ALTER TABLE account_providers ADD COLUMN IF NOT EXISTS organization_uuid TEXT;
