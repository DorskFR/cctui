CREATE TABLE preset_hooks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    preset_id       UUID NOT NULL REFERENCES presets(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    sealed_secret   TEXT NOT NULL,
    prompt_template TEXT NOT NULL,
    rate_per_minute INTEGER NOT NULL DEFAULT 30 CHECK (rate_per_minute BETWEEN 1 AND 600),
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);

CREATE INDEX preset_hooks_preset ON preset_hooks (preset_id);

CREATE TABLE preset_hook_deliveries (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    hook_id     UUID NOT NULL REFERENCES preset_hooks(id) ON DELETE CASCADE,
    delivery_id TEXT,
    body_sha256 TEXT,
    outcome     TEXT NOT NULL
                CHECK (outcome IN ('dispatched', 'duplicate', 'rejected', 'rate_limited')),
    detail      TEXT,
    event_id    UUID,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX preset_hook_deliveries_hook ON preset_hook_deliveries (hook_id, id DESC);
CREATE UNIQUE INDEX preset_hook_deliveries_once
    ON preset_hook_deliveries (hook_id, delivery_id) WHERE outcome = 'dispatched';
CREATE UNIQUE INDEX preset_hook_deliveries_body_once
    ON preset_hook_deliveries (hook_id, body_sha256) WHERE outcome = 'dispatched';
CREATE INDEX preset_hook_deliveries_received ON preset_hook_deliveries (received_at);
