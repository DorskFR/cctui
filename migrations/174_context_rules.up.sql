-- Rules: always-on items injected inline into every matching launch.
DO $$
DECLARE c text;
BEGIN
    FOR c IN
        SELECT conname FROM pg_constraint
         WHERE conrelid = 'context_items'::regclass AND contype = 'c'
           AND pg_get_constraintdef(oid) LIKE '%kind%'
    LOOP
        EXECUTE format('ALTER TABLE context_items DROP CONSTRAINT %I', c);
    END LOOP;
END $$;
ALTER TABLE context_items
    ADD CONSTRAINT context_items_kind_check CHECK (kind IN ('memory', 'prompt', 'rule'));

-- `filters` is the wire's `match`: AND across keys, OR within one, empty = any.
-- `scope`/`scope_ref` stay readable and are kept in step for one release.
ALTER TABLE context_items
    ADD COLUMN IF NOT EXISTS filters JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS pending JSONB,
    ADD COLUMN IF NOT EXISTS created_by_session TEXT,
    ADD COLUMN IF NOT EXISTS updated_by_session TEXT;

UPDATE context_items SET filters = CASE scope
    WHEN 'machine' THEN jsonb_build_object('machines', jsonb_build_array(scope_ref))
    WHEN 'path'    THEN jsonb_build_object('paths', jsonb_build_array(scope_ref))
    WHEN 'label'   THEN jsonb_build_object('labels', jsonb_build_array(scope_ref))
    ELSE '{}'::jsonb
END;

-- `scope` is the launch coordinates rules are re-resolved against on every
-- launch; `rules` is what the latest launch delivered.
ALTER TABLE session_context
    ADD COLUMN IF NOT EXISTS scope JSONB,
    ADD COLUMN IF NOT EXISTS rules JSONB NOT NULL DEFAULT '[]'::jsonb;

-- One row per agent `save`, for the per-session hourly rate limit.
CREATE TABLE IF NOT EXISTS context_tool_saves (
    id         BIGSERIAL PRIMARY KEY,
    session_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS context_tool_saves_session_idx
    ON context_tool_saves (session_id, created_at);
