-- jsonb_ops GIN never matched a query: every reader uses ->> extraction, not
-- containment. It only cost write amplification over multi-MB payloads.
DROP INDEX IF EXISTS ghreview.idx_documents_payload_gin;
