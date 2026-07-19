# Validating admission webhook (`cctui-orchestrator`)

The `cctui-orchestrator` binary also serves a **validating** admission webhook
(`POST /validate`) alongside the mutating one (`POST /mutate`). These are
**guardrails, not adversarial defense**: the profile author (operator) is
trusted. The checks catch accidental sandbox breakage, malformed profiles, and
the one agent-influenced surface — the dispatch request, which must never be
able to reshape its own sandbox by smuggling raw pod-spec overrides.

Validation logic lives in `crates/cctui-orchestrator/src/validate.rs` as pure
functions plus a small `ProfileSource` trait; the axum route is in `src/main.rs`.

## Ordering

Kubernetes runs validating webhooks **after** all mutating webhooks, so the pod
`/validate` inspects already has the envelope injected. A profiled pod that
reaches validation *without* the `cctui.dev/envelope-injected: "true"` marker is
therefore denied (fail-closed pairing with `/mutate`).

## Scope

Only pods carrying the `cctui.dev/worker-profile` label are checked; any other
pod is allowed untouched. Operator-added **other** containers (the app stack) are
trusted and not policed — only the worker container and pod-level fields are.

## Checks

1. **Structural** — exactly one worker container (named per
   `cctui.dev/worker-container`, default `worker`) and the
   `cctui.dev/envelope-injected: "true"` marker present.
2. **Sandbox footguns** (operator protection) — denied:
   - worker `securityContext.privileged: true`;
   - pod-level `hostPID` / `hostIPC` / `hostNetwork`;
   - a `hostPath` volume mounted into the worker container;
   - worker running as uid `1337` (the guard-proxy identity);
   - worker `capabilities.add` beyond the sanctioned set `[SYS_ADMIN, CHOWN,
     DAC_OVERRIDE, FOWNER, FSETID, KILL, SETUID, SETGID, SETPCAP]`.
3. **Profile conformance** (the critical "reject raw pod overrides" check) — the
   named `WorkerProfile` is fetched and the pod must be exactly what the
   dispatcher + mutating webhook would produce from it:
   - `serviceAccountName` equals the profile's (dispatch may not override
     identity / secret scope);
   - worker image / command / args equal the profile's;
   - container + initContainer **names** = the profile's declared set + the
     worker + the envelope's (`net-init`, `guard-proxy`) — nothing extra;
   - volumes = the profile's + the envelope's (`home`, `overlay`, `guard-state`,
     `proxy-policy`, `guard-proxy-inject`, `guard-proxy-ca`, plus `gpg-agent`
     when the profile requests signing) — no extras;
   - `nodeSelector` / `runtimeClassName` equal the profile's;
   - worker env: payload env is agent-influenced by design, so env **names** are
     not allowlisted; instead a `valueFrom` reference not declared by the
     profile's own env is denied (no mounting cluster secrets via env), and a
     secret-ref-shaped literal (`vault:` / `bao:` / `k8s:` prefix) is denied.

   A missing profile is denied. Every denial carries a human-readable
   `.status.message`.

## Architecture

Profile lookup is behind the `ProfileSource` trait so unit tests use an
in-memory map and only the `main` wiring uses `Api<WorkerProfile>`
(`KubeProfileSource`). The namespace is taken from the AdmissionRequest (falling
back to the pod's own namespace, then `default`).

## Deployment requirements

Routes and env vars are shared with the mutating webhook (same binary, same TLS
config — see [`mutating-injection-webhook.md`](./mutating-injection-webhook.md)).
Additional route: `POST /validate` (AdmissionReview v1).

- **RBAC** — the webhook now needs `get` on `workerprofiles` (group
  `cctui.dev`, resource `workerprofiles`) in the namespaces it validates. Add
  this verb to the orchestrator's `ClusterRole`/`Role`; without it every profiled
  pod is denied fail-closed.
- **ValidatingWebhookConfiguration** — pair with the existing
  `MutatingWebhookConfiguration`, pointing at the same Service on `/validate`:
  - `rules`: `CREATE` (and `UPDATE`) on `pods`;
  - `failurePolicy: Fail` (guardrails must not be bypassable by an unreachable
    webhook);
  - `sideEffects: None`, `admissionReviewVersions: ["v1"]`;
  - scope it (namespace selector / object selector) to the namespaces where
    worker pods run, so unrelated pods never hit it.
