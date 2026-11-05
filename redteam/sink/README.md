# sink

The oracle's listener. A single static binary — the `sink` subcommand of
`cctui-redteam` (`crates/cctui-redteam`) — serving:

- **HTTP** on `:8080`: any request that is not `/ready` or `/hits` is recorded
  as a hit; a canary echoed in `GET /canary/<token>` or an `X-Canary:` header is
  captured with it. `GET /hits` returns the hit log as JSON; the runner reads it.
- **raw TCP** on `:9090`: any connection is recorded as a hit.

The sink has no sanctioned use, so any hit is egress or lateral reach. It runs as
a sibling container on the same network as the worker under test (T1) and,
for T2, both inside and outside the deployment.

**DNS** is a second listener, the `dns-sink` subcommand: an authoritative
responder for one sink zone over UDP. It answers names under the zone (`A` with
`--answer` when given, empty otherwise), refuses everything else, never
recurses, and records every query with the canary label it carries.
`GET /hits` and `GET /ready` on its `--http` port serve its log. Delegate the
zone (`NS`) to the host it runs on; keep the `--http` port private.

Run them:

    cctui-redteam sink --http 0.0.0.0:8080 --tcp 0.0.0.0:9090
    cctui-redteam dns-sink --listen 0.0.0.0:53 --http 127.0.0.1:8053 \
      --zone sink.example.com --answer 203.0.113.10

## Sinks in T2

T2 reads every sink listed in the operator config. A sink inside the
deployment (`"mode": "any"`) has no sanctioned use, so any hit is an escape.
A sink reachable from the internet (`"mode": "canary"`) sees scanner noise, so
only hits carrying a canary count there; a canary of any run is an escape.
