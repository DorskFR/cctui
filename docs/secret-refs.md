# Secret refs

Secret config values can name where the secret lives instead of holding it.
Errors name the variable and the ref, never the value. A ref that does not
resolve at startup stops the process.

| Consumer | Values that accept a ref |
| --- | --- |
| Full server | `DATABASE_URL`, `CCTUI_VAULT_KEY`, `CCTUI_ADMIN_TOKENS`, `CCTUI_GITHUB_TOKEN`, `GH_TOKEN`, `CCTUI_NTFY_TOKEN`, `CCTUI_CLAUDE_LITELLM_TOKEN`, `CCTUI_EMOJI_TOKEN`, `CCTUI_LANGFUSE_SECRET_KEY`, each `token` in `CCTUI_HTTP_DISPATCHERS` and in the `kind:"http"` entries of `CCTUI_DISPATCHERS` |
| Core server | `DATABASE_URL`, `CCTUI_VAULT_KEY`, `CCTUI_ADMIN_TOKENS`, `CCTUI_GITHUB_TOKEN`, `GH_TOKEN`, each `token` in `CCTUI_HTTP_DISPATCHERS` |
| Dispatchers | `dispatcher_key` in `dispatcher.toml` |
| Guard proxy | every secret in the inject config (refs only, no literals) |

`CCTUI_ADMIN_TOKENS` is resolved as a whole: the ref yields the
comma-separated list. A dispatcher token is resolved per entry
(`[{"id":"ext","url":"…","token":"k8s:cctui-dispatch#token"}]`); a failure names
`CCTUI_HTTP_DISPATCHERS[ext].token`.

`CCTUI_VAULT_KEY` is resolved before the key checks run, so a ref to a short,
missing or non-hex key is refused exactly like the literal would be. It is
resolved once per process and never reloaded: everything in the vault is
encrypted under it, so swapping it at runtime would make stored credentials
unreadable. Rotate it by re-encrypting, then restarting.

## Reload

Refs re-resolve without a restart, on `SIGHUP` and on an optional TTL. A
refresh swaps the value atomically; one that fails keeps the previous value
and logs a warning with the variable name and the ref.

| Consumer | Follows a rotation | Picked up |
| --- | --- | --- |
| Full server | every ref above except `CCTUI_VAULT_KEY` | new pool connections; the next request that uses the token |
| Core server | every ref above except `CCTUI_VAULT_KEY` | new pool connections; the next request that uses the token |
| Dispatchers | `dispatcher_key` | the next connection to the server; the kube dispatcher's next Job registration |
| Guard proxy | every inject-config secret | the next request after the refresh |

Open pool connections keep their session until the pool recycles them. A
rotated `CCTUI_CLAUDE_LITELLM_TOKEN` re-runs the litellm shim, so the managed
providers carry the new credential.

A rotated `CCTUI_ADMIN_TOKENS` swaps the whole list at once: the new tokens
work from the swap on, then the tokens dropped from the list have their
`auth_keys` rows revoked and their cached sessions purged on every replica.
A revoked env token stays revoked, so putting it back in the list does not
revive it; add a new token instead.

The TTL is off by default. `CCTUI_SECRETS_TTL=<seconds>` sets it for every
ref of a server or dispatcher; `CCTUI_SECRETS_TTL_<NAME>` sets it for one
(`CCTUI_SECRETS_TTL_DATABASE_URL`, `CCTUI_SECRETS_TTL_DISPATCHER_KEY`); `0`
turns it off. A literal never refreshes.

The guard proxy keeps its own cache TTL (`--secret-ttl-secs`, 120 by default):
it re-reads a secret on the first request after the TTL, and on `SIGHUP` it
re-reads every cached one at once. A backend failure keeps the cached value
and retries after another TTL; a secret that no longer exists (`NotFound`) is
dropped.

## Forms

| Ref | Reads |
| --- | --- |
| `env:<VAR>` | another environment variable |
| `file:<path>` | a file, with one trailing newline (`\n` or `\r\n`) stripped |
| `k8s:[<namespace>/]<secret>#<key>` | a Kubernetes Secret through the API with the pod's service account (no mount); the namespace defaults to the pod's |
| `bao:<mount>/data/<path>#<field>` | an OpenBao/Vault KV v2 field, logging in with the pod's service-account token through Kubernetes auth. `vault:` is the same; prefer `bao:` in pod env, where the bank-vaults webhook rewrites `vault:` values |
| `aws-sm:<secret-id>[#<json-key>]` | AWS Secrets Manager with ambient credentials (see below). With `#<json-key>` the secret must be a JSON object |

The `bao:` engine for the servers and dispatchers is configured by
`CCTUI_SECRETS_VAULT_ADDR` and `CCTUI_SECRETS_VAULT_ROLE`, plus
`CCTUI_SECRETS_VAULT_TOKEN_PATH` (default: the projected service-account
token). The guard proxy keeps its own `--vault-addr`/`--vault-role` flags.

## AWS Secrets Manager

`aws-sm:` signs one `GetSecretValue` call (SigV4) with the first credential
source configured, the same chain the guard proxy uses for ambient `sigv4`
rules:

1. `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`)
2. web identity (IRSA): `AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN`
3. the container endpoint (ECS, EKS Pod Identity):
   `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` or `_FULL_URI`
4. `IMDSv2`, unless `AWS_EC2_METADATA_DISABLED=true`

Temporary credentials are refreshed five minutes before they expire. The
region is the one in the secret ARN when the ref uses an ARN, otherwise
`AWS_REGION`, then `AWS_DEFAULT_REGION`. `AWS_ENDPOINT_URL_SECRETS_MANAGER` or
`AWS_ENDPOINT_URL` override the endpoint. Shared config and credential files
(`~/.aws/config`, `AWS_PROFILE`) are not read.

## Literals

Only the exact prefixes `env:`, `file:`, `k8s:`, `bao:`, `vault:` and
`aws-sm:` make a ref. Anything else is used as written, so
`postgres://user:pw@host/db` and tokens containing `:` need no change. A
malformed ref (`k8s:name` without `#key`) is a startup error, not a literal.
To use a literal that starts with one of those prefixes, prefix it with
`literal:` (`literal:env:x` is the value `env:x`).

## Engines per build

`cctui-secrets` compiles `env:` and `file:` always; `k8s`, `bao` and `aws-sm`
are cargo features. The guard proxy enables all three. The full server
(`cctui-server-workspace`), the core server and the dispatchers
(`cctui-dispatcher-core`) enable `k8s` and `bao`; each exposes an `aws-sm`
feature for builds that need it (for example
`cargo build -p cctui-server --features cctui-server-workspace/aws-sm`). A ref
to an engine that is not compiled in fails at startup.
