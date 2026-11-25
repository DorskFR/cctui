ALTER TABLE presets
    ADD COLUMN timezone     TEXT NOT NULL DEFAULT 'UTC',
    ADD COLUMN enabled      BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN overlap      TEXT NOT NULL DEFAULT 'allow' CHECK (overlap IN ('allow', 'skip')),
    ADD COLUMN last_fire_at TIMESTAMPTZ;
