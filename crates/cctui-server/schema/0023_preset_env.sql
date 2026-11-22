ALTER TABLE presets
    ADD COLUMN env_keys   TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN sealed_env TEXT;
