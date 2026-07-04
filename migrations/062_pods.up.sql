-- CCT-573: peer discovery + pod-to-pod auth for the PeerHttp bus transport.
--
-- `pods` mirrors `ws_presence` at the pod level: each replica that knows its
-- routable IP (CCTUI_POD_IP) registers itself here and heartbeats, so peers can
-- fan events out to every live replica. Without a pod IP nothing is written.
CREATE TABLE pods (
    pod TEXT PRIMARY KEY,
    pod_ip TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Cluster-internal shared secrets, minted once at first boot
-- (`INSERT ... ON CONFLICT DO NOTHING`) and read by every replica. Currently a
-- single row (`internal_bus`) authenticating the pod-to-pod /internal/bus/*
-- endpoints. Not a user-facing credential; never leaves the cluster.
CREATE TABLE cluster_secrets (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
