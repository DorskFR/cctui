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
The mutating webhook, the validating webhook and the dispatcher all resolve it
via `WorkerProfileSpec::worker_container_name()` (explicit `workerContainer`,
else the `worker` default). The worker container's
shape comes from the first-class fields (`image`, `command`, `args`, `resources`,
`env`); everything in `containers`/`initContainers` is the surrounding app stack
and is left untouched.

## Field ownership

| Field | Owner | Notes |
|---|---|---|
| `image`, `command`, `args`, `resources`, `env` | operator | worker-container shape; `env` is non-secret |
| `envFrom`, `volumeMounts` | operator | worker-container env sources / mounts; non-secret config, mounts name operator-owned `volumes` |
| `workerContainer` | operator | overrides the sandboxed container name (default `worker`) |
| `containers`, `initContainers`, `volumes`, `imagePullSecrets`, `nodeSelector`, `tolerations`, `affinity`, `runtimeClassName` | operator | passthrough pod shape; webhook does not sandbox these |
| `podAnnotations` | operator | stamped onto each instantiated pod's template metadata; the dispatcher's own `cctui.dev/*` session annotations win on key conflict |
| `pod` | operator | `labels`, `annotations` (stamped onto the Job **and** its pod template) and `priorityClassName`, for clusters whose admission policies require them (see [Pod labels, annotations and priority](#pod-labels-annotations-and-priority)) |
| `serviceAccountName` | operator | identity / secret scope mapping; the dispatch request never sets this |
| `gpgSigning` | operator | opt-in; webhook wires a gpg-agent socket into the worker container |
| `guard` | operator | guard-proxy sidecar `envFrom`/`env` layered over `guard-proxy-env`; widening needs `allowPrivateEgress` (see worker-contract.md) |
| `posture` | operator | `root` or `nonroot`: the worker container's security posture (see [Posture](#posture)); unset is `root` |
| `preflight` | operator | `mode` (`enforce`/`warn`/`off`) and extra `deny` targets for the worker's pre-flight isolation check, rendered as `CCTUI_PREFLIGHT_*` worker env after the per-run env, so neither the profile's own `env` nor a dispatch can override them (see worker-contract.md) |
| per-run env (session id, reply URL, task payload, ...) | dispatcher | layered onto the worker `env` at Job creation |
| secretless credential envelope, gpg-agent socket | webhook | injected into **only** the worker container at pod admission |

## Scheduling on dedicated nodes

`spec.tolerations` accepts Kubernetes `Toleration` objects (`key`, `operator`,
`value`, `effect`, `tolerationSeconds`) and passes them to the Job's Pod
template. Pair a dedicated-node `NoSchedule` toleration with `nodeSelector`
to both permit and confine placement. See the
[core deployment checklist](core-kubernetes.md) and its example profile.

Tolerations are operator-owned. Admission rejects missing, changed or extra
entries, except the standard Kubernetes node-health and memory-pressure
admission additions. Job templates receive no such exception: changing a
profile's tolerations triggers the existing queued-Job drift repair. Profiles
without the field keep their existing scheduling behavior.

`spec.affinity` takes a Kubernetes `Affinity` (`nodeAffinity`, `podAffinity`,
`podAntiAffinity`) and is passed to the Pod template verbatim. Admission
requires the pod's affinity to equal the profile's, and a change triggers the
same drift repair.

## Pod labels, annotations and priority

Some clusters' admission policies require labels, annotations or a priority
class on every pod. `spec.pod` provides them:

```yaml
spec:
  pod:
    labels:
      team: platform
    annotations:
      policy.example.dev/owner: platform
    priorityClassName: cctui-workers
```

`labels` and `annotations` land on both the Job and its Pod template;
`priorityClassName` on the Pod template. cctui-managed keys always win, and the
validating webhook denies a pod whose profile tries to set one: labels and
annotations under `cctui.dev/` or `batch.kubernetes.io/`, and the
`job-name` / `controller-uid` labels the Job controller selects on. The
dispatcher refuses to create the Job for such a profile, too.

A pod's `priorityClassName` must equal the profile's. When the profile sets
none, an admitted pod may still carry the cluster's global default class, which
priority admission adds. Changing the class triggers the queued-Job drift
repair. A profile without `spec.pod` renders the Job byte-for-byte as before.
`podAnnotations` remains a template-only annotation map where the dispatcher's
own keys win silently.

## Posture

`spec.posture` picks the worker container's `securityContext`. The webhook
renders it; a profile never sets the worker `securityContext` itself.

| | `root` | `nonroot` |
|---|---|---|
| worker user | uid 0 (the supervisor drops the agent to 1000) | `runAsUser`/`runAsGroup: 1000`, `runAsNonRoot: true` |
| worker capabilities | `drop: [ALL]` + `SYS_ADMIN`, `CHOWN`, `DAC_OVERRIDE`, `FOWNER`, `FSETID`, `KILL`, `SETUID`, `SETGID`, `SETPCAP` | `drop: [ALL]`, `allowPrivilegeEscalation: false` |
| seccomp / AppArmor | runtime default / `Unconfined` | `RuntimeDefault` / runtime default |
| `overlay` emptyDir + `/overlay` mount | yes | no |
| `WARM_REPO_DIR` | overlayfs warm cache | stripped from the worker env |
| guard-proxy sidecar | uid/gid 1337, `drop: [ALL]`, no privilege escalation | same, plus `runAsNonRoot` and `RuntimeDefault` seccomp |
| `net-init`, pod `fsGroup: 1000` | unchanged | unchanged |

**Unset is `root`, permanently.** The CRD schema has no default on purpose:
the API server applies structural defaults to stored objects when it reads
them, so a schema default would flip every existing profile. A profile without
the field renders exactly the pod it rendered before `posture` existed.

**New profiles set `posture: nonroot` explicitly.** Start from:

```yaml
apiVersion: cctui.dev/v1alpha1
kind: WorkerProfile
metadata:
  name: example
spec:
  image: ghcr.io/dorskfr/cctui-worker:latest
  posture: nonroot
```

Move an existing profile by adding `posture: nonroot` to it; queued Jobs of
that profile are rebuilt like any other template drift. Set `posture: root`
to opt back in.

The dispatcher stamps `cctui.dev/posture: nonroot` on the pod template of a
`nonroot` profile (a `root` Job carries no annotation, and a `podAnnotations`
copy is dropped); the mutating webhook renders from it and denies an unknown
value. The validating webhook denies a pod whose annotation doesn't match its
profile's posture, and a `nonroot` worker whose `securityContext` is weaker
than the table above, or whose pod sets the legacy
`container.apparmor.security.beta.kubernetes.io/<worker>: unconfined`
annotation.

A `nonroot` profile is denied (every pod it instantiates, with the reason)
when it contradicts the posture:

- `env` sets `WARM_REPO_DIR` (the overlayfs warm cache needs `SYS_ADMIN`; bake
  the warm repo into the image instead);
- `volumes` has one named `overlay`, or `volumeMounts` mounts `overlay` or
  `/overlay`;
- a passthrough `containers`/`initContainers` entry is `privileged: true`,
  adds capabilities or sets `runAsUser: 0` (`net-init` stays the pod's only
  privileged container).

The image must support a non-root start (see worker-contract.md, Non-root
start).

## Regenerating the CRD YAML

`deploy/workerprofile-crd.yaml` is generated — do not hand-edit:

```sh
cargo run -p cctui-orchestrator --bin crdgen > deploy/workerprofile-crd.yaml
```

Each GitHub release attaches it as `workerprofile-crd.yaml`:

```sh
gh release download vX.Y.Z --repo DorskFR/cctui --pattern workerprofile-crd.yaml
kubectl apply -f workerprofile-crd.yaml
```
