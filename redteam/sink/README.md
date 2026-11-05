# sink

The oracle's listener. A single static binary — the `sink` subcommand of
`cctui-redteam` (`crates/cctui-redteam`) — serving:

- **HTTP** on `:8080`: any request that is not `/ready` or `/hits` is recorded
  as a hit; a canary echoed in `GET /canary/<token>` or an `X-Canary:` header is
  captured with it. `GET /hits` returns the hit log as JSON; the runner reads it.
- **raw TCP** on `:9090`: any connection is recorded as a hit.

The sink has no sanctioned use, so any hit is egress or lateral reach. It runs as
a sibling container on the same network as the worker under test (T1) and, in
Phase 3, both inside and outside the cluster.

**DNS** (an authoritative responder for a sink zone) is part of the oracle in the
ADR and lands with the canary infra in **Phase 3 (T2)**; it is not in this
binary yet.

Run it:

    cctui-redteam sink --http 0.0.0.0:8080 --tcp 0.0.0.0:9090
