-- Skills sync: one row per skill name (last-write-wins, hashed-on-content).
CREATE TABLE IF NOT EXISTS skill_registry (
    name                 TEXT PRIMARY KEY,
    version              TEXT NOT NULL,
    sha256               TEXT NOT NULL,
    size_bytes           BIGINT NOT NULL,
    uploaded_by_machine  UUID REFERENCES machines(id) ON DELETE SET NULL,
    uploaded_by_user     UUID REFERENCES users(id) ON DELETE CASCADE,
    uploaded_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_type         TEXT NOT NULL DEFAULT 'application/zstd'
);

CREATE INDEX IF NOT EXISTS idx_skill_registry_user ON skill_registry(uploaded_by_user);
