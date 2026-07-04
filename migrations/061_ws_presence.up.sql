-- CCT-567: replica-aware WS routing. Each server replica records which live
-- daemon/dispatcher WebSockets it terminates, so a peer replica receiving an
-- HTTP request that needs that WS can forward it to the owning pod instead of
-- reporting a spurious "offline". Rows are upserted on WS connect, deleted on
-- disconnect (guarded by pod so a cross-pod reconnect race never deletes the
-- new owner's row), and heartbeated per pod; a row is only trusted while its
-- heartbeat is fresh, so rows orphaned by a crashed pod age out.
CREATE TABLE ws_presence (
    -- 'daemon' | 'dispatcher'
    kind TEXT NOT NULL,
    -- machines.id / dispatchers.id
    entity_id UUID NOT NULL,
    -- pod (host) name owning the WS; unique per replica
    pod TEXT NOT NULL,
    -- pod IP peers forward to (server port is shared config)
    pod_ip TEXT NOT NULL,
    connected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, entity_id)
);

-- Per-pod heartbeat + boot cleanup scan.
CREATE INDEX ws_presence_pod_idx ON ws_presence (pod);
