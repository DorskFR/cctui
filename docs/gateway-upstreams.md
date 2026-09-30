# Gateway upstreams for compatible endpoints

An `anthropic-compatible`, `openai-compatible` or `fireworks` account can set a
`base_url`. The gateway forwards requests to that URL, so it goes through the
same outbound guard as completion webhooks.

## What is refused

A `base_url` is rejected when an account is created or updated, and again on
every gateway request, if:

- it is not `https`;
- its host is an IP literal in a loopback, private (RFC 1918), link-local
  (including `169.254.169.254`), CGNAT, unique-local or unspecified range;
- its host is a single-label name, or ends in `.svc`, `.cluster.local`,
  `.local`, `.internal` or `.localhost`;
- its name resolves to any of those addresses. Names are resolved again when
  the gateway connects, so a name cannot be rebound onto an internal address
  after it was accepted.

The gateway never follows a redirect from a per-account upstream.

A refused account gets a `400` from the accounts API, and a `502` from the
gateway whose error message points at the allowlist below. The server log
has a matching `gateway refused account base_url` warning.

## Allowing a trusted internal upstream

An admin edits the allowlist in **Settings > Instance > Allowed upstream hosts**;
changes apply immediately (other replicas pick them up within 30 seconds). Hosts
in the env var below are always allowed on top of the saved list and show in
Settings as fixed entries; resetting clears only the saved list. On upgrade to
0.20 the saved list is seeded with the host of every account `base_url` already
stored, so existing upstreams keep working.

| Variable | Default | Meaning |
|---|---|---|
| `CCTUI_UPSTREAM_ALLOWED_HOSTS` | *(unset)* | Always allowed, in addition to the Settings list. Comma-separated hosts the guard lets through, as `host` or `host:port`, e.g. `ollama.llm.svc:11434,192.168.1.50`. An entry with a port allows only that port; a bare host allows every port. Allowed hosts may use plain `http`. |

The host and port of `CCTUI_CLAUDE_LITELLM_ENDPOINT` are always allowed, since the
managed LiteLLM account points at it.

Only allow hosts you trust with arbitrary requests from any user who can create
an account.

## Usage probes for a compatible endpoint

Routing to an `anthropic-compatible` / `openai-compatible` credential works with
no extra configuration, but such a credential is **measurement-blind**: no usage
windows, so no pace, no soft limit, and `account_pick` ranks it
`usage_known: false` — it can neither be trusted nor excluded.

Setting **Usage probe** in the provider drawer (Advanced) names an entry in the
server's probe registry (`crates/cctui-server/src/usage_probe.rs`). The probe
reads that upstream's own quota endpoint and reports it under the existing
canonical window keys, so pace, soft limits and pool election pick it up with no
further configuration. The credential stays generic — a probe is measurement,
not a new provider kind, and the family count in the UI stays at two.

| Probe | Reads | Reports |
|---|---|---|
| `openrouter` | `GET {base}/api/v1/key` with the stored credential as a Bearer | `usd_7d` from `usage_weekly`, resetting Monday 00:00 UTC |
| `litellm` | `GET {base}/key/info` | `usd_5h` or `usd_7d` from the virtual key's `spend`, when its `budget_duration` is one of those lengths |

A probe reports nothing rather than guess. A LiteLLM key on a 30-day budget has
no canonical window of that length, so it stays unmeasured until the canonical
vocabulary in `soft_limit.rs` is extended deliberately — mapping it onto `usd_7d`
would make every reset time downstream a lie.

Probe failure (transport, non-2xx, unexpected shape) falls back to the last
cached reading and, with none, to `usage_known: false`. It never degrades to a
zeroed, wide-open quota.
