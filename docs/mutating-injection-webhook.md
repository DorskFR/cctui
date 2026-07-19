# Mutating injection webhook (`cctui-orchestrator`)

The `cctui-orchestrator` binary is a mutating admission webhook. On pod
`CREATE`, if the pod was instantiated from a `WorkerProfile` it injects the
secretless-worker **envelope** so every profiled pod is sandboxed identically,
without the operator hand-wiring it. Only the agent's worker container is
sandboxed; every other container and initContainer is passthrough (threat model:
the operator is trusted, the agent inside the worker container is not).

Envelope construction lives in `crates/cctui-orchestrator/src/envelope.rs` as
pure functions (`mutate_pod`, `inject`); the axum server is `src/main.rs`.

## Trigger and cross-crate contract

Pods instantiated from a profile carry (set by the dispatcher, CCT-725/728):

| Key | Kind | Meaning |
|---|---|---|
| `cctui.dev/worker-profile: <name>` | label | **trigger** — a pod without it is admitted unchanged (no patch) |
| `cctui.dev/worker-container: <name>` | annotation | which container to sandbox (default `worker` if absent) |
| `cctui.dev/guard-identity: <identity>` | annotation | optional; sets `GUARD_PROXY_IDENTITY` on the sidecar, overriding the ConfigMap default |
| `cctui.dev/gpg-signing: "true"` | annotation | optional; request gpg-agent wiring (mirrors profile `gpgSigning`) |

After injection the webhook stamps `cctui.dev/envelope-injected: "true"`. These
strings are exported as `pub const`s from the crate lib (`LABEL_WORKER_PROFILE`,
`ANNOTATION_*`) so later tickets reuse them.

## Fail-closed / idempotency

- A pod without the worker-profile label is admitted **unchanged**.
- Re-invocation is a no-op: if the `envelope-injected` marker annotation is
  present, or a `guard-proxy` initContainer already exists, `mutate_pod` returns
  no patch — never double-injects.
- `failurePolicy: Fail` is deployment config (a later ticket); the handler
  itself is safe under re-invocation and on unlabeled pods.

## What gets injected

The sidecar/init image defaults to `ghcr.io/dorskfr/cctui-worker:<version>`
(the binary's own version), overridable via `CCTUI_ORCH_SIDECAR_IMAGE`.

1. **initContainer `net-init`** — `cctui-worker-net-init`, `runAsUser 0`, caps
   drop `ALL` add `NET_ADMIN` (iptables REDIRECT of worker egress into the
   proxy; proxy uid `1337` RETURNs).
2. **initContainer `guard-proxy`** (native sidecar, `restartPolicy: Always`) —
   `cctui-guard-proxy-entrypoint --mode=transparent --listen=0.0.0.0:15001
   --health-listen=0.0.0.0:15002 --policy=/var/run/guard-proxy/policy.json`;
   `envFrom` the `guard-proxy-env` ConfigMap; `runAsUser/runAsGroup 1337`,
   `allowPrivilegeEscalation false`, caps drop `ALL`; startupProbe
   `GET /health:15002`. When `guard-identity` is set, `GUARD_PROXY_IDENTITY` is
   upserted over the ConfigMap default.
3. **Pod-level** — `securityContext.fsGroup: 1000` if unset; emptyDir volumes
   `home`, `overlay`, `guard-state`, `proxy-policy`, `guard-proxy-ca` (plus
   `gpg-agent` when gpg requested) and the `guard-proxy-inject` ConfigMap volume,
   added only if missing.
4. **Worker container** (the only one mutated) — securityContext overwritten with
   the sanctioned shape: `runAsUser 0`, AppArmor `Unconfined` (the default
   profile denies `mount(2)`), caps drop `ALL` add `[SYS_ADMIN, CHOWN,
   DAC_OVERRIDE, FOWNER, FSETID, KILL, SETUID, SETGID, SETPCAP]` (never
   privileged); env `WORKER_NET_MODE=transparent-external` upserted; envelope
   volume mounts added.

The envelope **template** is baked into the release binary. Environment-specific
guard-proxy settings (vault addr/role, gpg secret ref, default identity) are
**not** baked in — they arrive at runtime via the `guard-proxy-env` ConfigMap.

## Deployment requirements (later ticket)

The webhook process reads these env vars:

| Env var | Purpose | Default |
|---|---|---|
| `CCTUI_ORCH_TLS_CERT` / `CCTUI_ORCH_TLS_KEY` | PEM cert/key paths for HTTPS. If unset, serves plain HTTP (tests/local only) | — (plain HTTP) |
| `CCTUI_ORCH_LISTEN` | listen address | `0.0.0.0:8443` |
| `CCTUI_ORCH_SIDECAR_IMAGE` | override the injected sidecar/init image | `ghcr.io/dorskfr/cctui-worker:<version>` |

Per-namespace, operator/Argo-managed resources the injected envelope references
(`optional: false` — the pod fails to start if absent):

- ConfigMap **`guard-proxy-env`** — guard-proxy runtime env (`envFrom`): vault
  addr/role, gpg secret ref, default `GUARD_PROXY_IDENTITY`.
- ConfigMap **`guard-proxy-inject`** — mounted read-only at `/etc/guard-proxy`
  (the worker must not define inject rules).

Routes: `POST /mutate` (AdmissionReview v1), `GET /healthz`, `GET /readyz`.
