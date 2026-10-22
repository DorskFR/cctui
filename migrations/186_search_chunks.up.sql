-- Transcript search reads the visible text of each stream event, folded and
-- split into overlapping chunks of about 4 KB. The server writes every row
-- (inline on ingest, and the resumable backfill in search_index): this
-- migration only creates the empty tables, so it is instant on any install.
CREATE TABLE IF NOT EXISTS search_chunks (
    event_id   BIGINT  NOT NULL REFERENCES stream_events (id) ON DELETE CASCADE,
    ord        INTEGER NOT NULL,
    session_id TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    body       TEXT    NOT NULL,
    PRIMARY KEY (event_id, ord)
);

-- session_id leads (btree_gin, migration 107) so a probe touches only the
-- chunks of the session being tested, as in migration 108.
CREATE INDEX IF NOT EXISTS idx_search_chunks_session_body_trgm
    ON search_chunks USING gin (session_id, body gin_trgm_ops);

CREATE TABLE IF NOT EXISTS search_index_state (
    name          TEXT PRIMARY KEY,
    last_event_id BIGINT      NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
