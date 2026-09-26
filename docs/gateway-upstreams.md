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
changes apply immediately (other replicas pick them up within 30 seconds). The
env var below only seeds it: a list saved in Settings wins over the env value,
and resetting it in Settings falls back to the env value (or to an empty list).
On upgrade to 0.20 the list is seeded with the host of every account `base_url`
already stored, so existing upstreams keep working.

| Variable | Default | Meaning |
|---|---|---|
| `CCTUI_UPSTREAM_ALLOWED_HOSTS` | *(unset)* | Seed for the Settings list. Comma-separated hosts the guard lets through, as `host` or `host:port`, e.g. `ollama.llm.svc:11434,192.168.1.50`. An entry with a port allows only that port; a bare host allows every port. Allowed hosts may use plain `http`. |

The host and port of `CCTUI_CLAUDE_LITELLM_ENDPOINT` are always allowed, since the
managed LiteLLM account points at it.

Only allow hosts you trust with arbitrary requests from any user who can create
an account.
