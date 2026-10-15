-- `scope`/`scope_ref` were kept in step with `filters` on every write, so
-- dropping `filters` leaves them as the sole, current source.
DROP TABLE IF EXISTS context_tool_saves;

ALTER TABLE session_context DROP COLUMN IF EXISTS rules, DROP COLUMN IF EXISTS scope;

DELETE FROM context_items WHERE kind = 'rule';

ALTER TABLE context_items
    DROP COLUMN IF EXISTS updated_by_session,
    DROP COLUMN IF EXISTS created_by_session,
    DROP COLUMN IF EXISTS pending,
    DROP COLUMN IF EXISTS filters;

ALTER TABLE context_items DROP CONSTRAINT IF EXISTS context_items_kind_check;
ALTER TABLE context_items
    ADD CONSTRAINT context_items_kind_check CHECK (kind IN ('memory', 'prompt'));
