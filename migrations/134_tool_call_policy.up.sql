-- Per-account gateway tool-call policy. Server-side only: never sent to agents
-- or daemons. All arrays empty (or no row) means the scan is off.
CREATE TABLE IF NOT EXISTS account_tool_policies (
    account_id       UUID        PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    terms            TEXT[]      NOT NULL DEFAULT '{}',
    patterns         TEXT[]      NOT NULL DEFAULT '{}',
    protected_owners TEXT[]      NOT NULL DEFAULT '{}',
    exempt_roots     TEXT[]      NOT NULL DEFAULT '{}',
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per tool call the gateway refused to forward. The raw input is never
-- stored, only its sha256.
CREATE TABLE IF NOT EXISTS gateway_blocks (
    id           BIGSERIAL   PRIMARY KEY,
    session_id   TEXT,
    account_id   UUID        REFERENCES accounts(id) ON DELETE CASCADE,
    tool_name    TEXT        NOT NULL,
    rule         TEXT        NOT NULL,
    input_sha256 TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS gateway_blocks_session ON gateway_blocks (session_id, created_at DESC);
CREATE INDEX IF NOT EXISTS gateway_blocks_account ON gateway_blocks (account_id, created_at DESC);
