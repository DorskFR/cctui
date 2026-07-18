# WorkerProfile CRD (`cctui.dev/v1alpha1`)

`WorkerProfile` is the operator-authored description of a worker workload
*shape*. The dispatcher instantiates it into a Job; the injection webhook
augments the resulting pod at admission. Types live in `crates/cctui-orchestrator`
(`WorkerProfile`, `WorkerProfileSpec`) and are importable by the webhook and
dispatcher crates.

## Trust model

The author is **trusted** — the operator owns the cluster and manages profiles
via GitOps/Argo. The schema is for ergonomics and consistency, **not** tenant
isolation, so it is a thin, mostly-passthrough shape close to a
`PodTemplateSpec`. The only adversary is the agent inside the worker container.

A dispatch request may select a profile **only by name**; it never sets any
field of the spec.

## Identifying the worker container

Exactly one container is sandboxed by the webhook. By convention it is the
container named `worker`; a profile overrides the name with `spec.workerContainer`.
Later tickets (CCT-726 mutating webhook, CCT-727 validating webhook, CCT-728
dispatcher) resolve it via `WorkerProfileSpec::worker_container_name()`
(explicit `workerContainer`, else the `worker` default). The worker container's
shape comes from the first-class fields (`image`, `command`, `args`, `resources`,
`env`); everything in `containers`/`initContainers` is the surrounding app stack
and is left untouched.

## Field ownership

| Field | Owner | Notes |
|---|---|---|
| `image`, `command`, `args`, `resources`, `env` | operator | worker-container shape; `env` is non-secret |
| `workerContainer` | operator | overrides the sandboxed container name (default `worker`) |
| `containers`, `initContainers`, `volumes`, `imagePullSecrets`, `nodeSelector`, `runtimeClassName` | operator | passthrough pod shape; webhook does not sandbox these |
| `serviceAccountName` | operator | identity / secret scope mapping; the dispatch request never sets this |
| `gpgSigning` | operator | opt-in; webhook wires a gpg-agent socket into the worker container (CCT-726) |
| per-run env (session id, reply URL, task payload, ...) | dispatcher | layered onto the worker `env` at Job creation |
| secretless credential envelope, gpg-agent socket | webhook | injected into **only** the worker container at pod admission |

## Regenerating the CRD YAML

`deploy/workerprofile-crd.yaml` is generated — do not hand-edit:

```sh
cargo run -p cctui-orchestrator --bin crdgen > deploy/workerprofile-crd.yaml
```
