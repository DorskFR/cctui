ALTER TABLE presets
    ADD COLUMN template      JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(template) = 'object'),
    ADD COLUMN sealed_prompt TEXT;
