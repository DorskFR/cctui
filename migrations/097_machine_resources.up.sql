-- Last-known host CPU / memory / disk snapshot per machine, upserted from the
-- daemon heartbeat's `resources` block, so the webui's header resource gauge
-- has a figure to paint before (and between) live WS updates.
CREATE TABLE machine_resources (
    machine_id       UUID PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    cpu_pct          REAL NOT NULL DEFAULT 0,
    mem_pct          REAL NOT NULL DEFAULT 0,
    mem_used_bytes   BIGINT NOT NULL DEFAULT 0,
    mem_total_bytes  BIGINT NOT NULL DEFAULT 0,
    disk_pct         REAL NOT NULL DEFAULT 0,
    disk_used_bytes  BIGINT NOT NULL DEFAULT 0,
    disk_total_bytes BIGINT NOT NULL DEFAULT 0,
    disk_path        TEXT NOT NULL DEFAULT '',
    load1            REAL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
