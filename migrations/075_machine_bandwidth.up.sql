-- CCT-744: last-known per-machine bandwidth, upserted from the daemon heartbeat's
-- per-subsystem byte counters so a runaway upload loop is queryable after the fact.
CREATE TABLE machine_bandwidth (
    machine_id  UUID PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    forward     BIGINT NOT NULL DEFAULT 0,
    retransmit  BIGINT NOT NULL DEFAULT 0,
    backfill    BIGINT NOT NULL DEFAULT 0,
    self_update BIGINT NOT NULL DEFAULT 0,
    blob_put    BIGINT NOT NULL DEFAULT 0,
    heartbeat   BIGINT NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
