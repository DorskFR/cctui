# Secret refs

Secret config values can name where the secret lives instead of holding it.
The value is resolved once at startup; errors name the ref, never the value.

| Consumer | Values that accept a ref |
| --- | --- |
| Full server | `DATABASE_URL`, `CCTUI_ADMIN_TOKENS`, `CCTUI_GITHUB_TOKEN`, `GH_TOKEN`, `CCTUI_NTFY_TOKEN`, `CCTUI_CLAUDE_LITELLM_TOKEN`, `CCTUI_EMOJI_TOKEN` |
| Core server | `DATABASE_URL`, `CCTUI_ADMIN_TOKENS`, `CCTUI_GITHUB_TOKEN`, `GH_TOKEN` |
| Dispatchers | `dispatcher_key` in `dispatcher.toml` |
| Guard proxy | every secret in the inject config (refs only, no literals) |

`CCTUI_ADMIN_TOKENS` is resolved as a whole: the ref yields the
comma-separated list. `CCTUI_VAULT_KEY` does not accept a ref yet.

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
