# Machine-readable usage export

Two surfaces expose the numbers the webui shows, for dashboards, status bars and
alerts:

- `GET /metrics` on the server — Prometheus text exposition.
- `cctui-admin usage --json` — the same figures as a JSON envelope.

**The names below are a contract.** A third party's dashboard or status bar pins
to them, and a rename breaks it silently. Add metrics and fields freely; never
rename or repurpose one.

## Authentication

`/metrics` sits at the root, outside `/api/v1`, because that is where a scrape
config looks for it — so it authenticates itself:

- **Default:** the same Bearer token scheme as `/api/v1` (any user or admin
  token; the browser auth cookie also works). No token ⇒ `401`.
- **`CCTUI_METRICS_PUBLIC=1`:** no authentication, for the usual
  scrape-inside-the-cluster case where the endpoint is not reachable from
  outside. Opt-in only — the output names accounts and their consumption, so it
  must never become public by accident.

```yaml
scrape_configs:
  - job_name: cctui
    static_configs: [{ targets: ["cctui:8700"] }]
    authorization:
      credentials_file: /etc/prometheus/cctui-token
```

## A scrape never reaches an upstream

Window figures are read from the server's per-credential usage cache only. A
credential with nothing cached exports `cctui_account_usage_known 0` and **no**
window series — not zeros, which would read as a fresh, wide-open quota. This is
deliberate: a 15-second scrape interval must not drive a rate-limited provider
usage endpoint.

A scrape with zero accounts configured is still valid: every metric family emits
its `# HELP`/`# TYPE` header, with no samples under it.

## Metrics

Labels: `account` (identity name), `provider` (`anthropic`, `openai`,
`anthropic-compatible`, …), `credential` (the provider-row uuid — the stable key;
`account` is renameable), `window` (canonical window key: `session`,
`weekly_all`, `weekly_model:<id>`, `session_usd`, `usd_5h`, `usd_7d`).

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `cctui_build_info` | gauge | `version` | Always `1`; carries the running server version. |
| `cctui_account_usage_known` | gauge | account | `1` when a usage reading is available, `0` when the credential is unmeasured. |
| `cctui_account_usage_age_seconds` | gauge | account | Age of the cached reading. Absent when nothing is cached. |
| `cctui_account_window_utilization_percent` | gauge | + `window` | Percent windows only. May exceed 100 on overage. |
| `cctui_account_window_spend_usd` | gauge | + `window` | Dollar windows only. |
| `cctui_account_window_seconds_to_reset` | gauge | + `window` | Floored at 0; absent when the window has no known reset. |
| `cctui_account_window_pace_ratio` | gauge | + `window` | Burn rate as a multiple of an even spend: `<1` under pace, `>1` too fast. |
| `cctui_account_window_projected_wall_timestamp_seconds` | gauge | + `window` | Unix timestamp at which the window hits 100% at the current rate. |
| `cctui_account_window_cap_percent` | gauge | + `window` | Configured soft-limit cap, when one is set. |
| `cctui_account_window_cap_usd` | gauge | + `window` | Configured soft-limit dollar cap, when one is set. |
| `cctui_account_tokens_total` | counter | account | Tokens attributed to the credential across all its sessions. |
| `cctui_account_cost_usd_estimate` | gauge | account | Blended-rate estimate, **not** a bill. A pay-per-token credential's real per-model spend is on the accounts API, priced from its own catalog. |
| `cctui_sessions` | gauge | `status` | Sessions by persisted status. |
| `cctui_sessions_live` | gauge | — | Sessions currently in the live registry. |
| `cctui_machines_live` | gauge | — | Machines whose daemon was seen in the last 5 minutes. |

## `cctui-admin usage`

`cctui-admin usage` prints a table; `--json` prints the envelope below. Auth
follows the usual precedence: `--token`, then `~/.config/cctui/machine.json`,
then `~/.config/cctui/user.json`.

```json
{
  "accounts": [
    {
      "account": "prod",
      "provider": "anthropic",
      "credential": "0c8e…",
      "usage_known": true,
      "age_seconds": 12,
      "windows": [
        {
          "key": "session",
          "label": "5h",
          "utilization_pct": 42.5,
          "spend_usd": null,
          "resets_at": "2026-09-30T14:00:00Z",
          "seconds_to_reset": 7200,
          "pace_ratio": 0.85,
          "projected_wall_at": "2026-09-30T18:00:00Z"
        }
      ]
    }
  ]
}
```

`spend_usd` is set on dollar windows and null on percent ones; `pace_ratio` and
`projected_wall_at` are null for a window with no known length or reset. An
instance with no credentials returns `{"accounts": []}`, never an error.
