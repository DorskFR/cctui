# cctui-guard-proxy

Egress allow-list proxy for the cctui worker. A hostname allow-list gates all
outbound connections; anything not on the list is dropped. Fail-closed: with no
policy loaded, everything is denied.

Two modes share one policy engine and one health endpoint:

- **transparent** (default) — the iptables `REDIRECT` target. Connections arrive
  as raw bytes (a TLS `ClientHello` or a plaintext HTTP request). The proxy
  recovers the original destination via `SO_ORIGINAL_DST`, peeks the SNI / `Host`
  header to recover the intended hostname, evaluates policy on `host:port`, and
  splices bytes through if allowed. Requires `NET_ADMIN` on the worker to install
  the iptables rules.
- **forward** — a standard HTTP proxy (`CONNECT` for TLS, absolute-URI for plain
  HTTP) on the same port. For the no-`NET_ADMIN` path (rootless Docker, Apple
  container, gVisor): the worker exports `HTTP_PROXY`/`HTTPS_PROXY` pointing at
  this proxy instead of installing iptables rules. Same policy evaluation, on the
  requested `host:port`.

IPv4 only — IPv6 egress is denied at the iptables layer by the worker entrypoint.

## CLI flags

| Flag              | Env                        | Default                          | Description                                  |
| ----------------- | -------------------------- | -------------------------------- | -------------------------------------------- |
| `--mode`          | `GUARD_PROXY_MODE`         | `transparent`                    | `transparent` or `forward`.                  |
| `--listen`        | `GUARD_PROXY_LISTEN`       | `0.0.0.0:15001`                  | Address the proxy listens on.                |
| `--health-listen` | `GUARD_PROXY_HEALTH_LISTEN`| `0.0.0.0:15002`                  | Address the health endpoint listens on.      |
| `--policy`        | `GUARD_PROXY_POLICY`       | `/var/run/guard-proxy/policy.json` | Path to the JSON policy file.              |

Log level is controlled by `RUST_LOG` (defaults to `info`).

## Policy file

JSON, hot-reloaded on change (mtime poll, ~1s). A missing or invalid file means
deny-all.

```json
{
  "allowed_hosts": ["example.com:443", "api.example.com:*"],
  "default": "deny"
}
```

- `allowed_hosts` — `host:port` entries. A `host:*` entry matches the host on any
  port. Matching is exact otherwise (no host globbing). A bare IP target never
  matches a hostname entry.
- `default` — `"deny"` (recommended) or `"allow"`. `"allow"` permits anything not
  explicitly listed; use only for debugging.

## Health endpoint

On `--health-listen`:

- `GET /health` — liveness; always `200 OK` while the process is up.
- `GET /ready` — readiness; `200 OK` once a policy is loaded, `503` otherwise
  (deny-all but not yet ready to allow traffic).

## Tests

```sh
cargo test -p cctui-guard-proxy
cargo clippy -p cctui-guard-proxy --all-targets
```
