# Core automation on Kubernetes

Use `cctui-core` for an API-only execution host and `cctui-dispatcher-kube`
for Jobs selected from operator-managed `WorkerProfile`s. The full workspace
server is unnecessary for this setup. An external caller supplies the trigger
and dispatches through `POST /api/v1/sessions/dispatch`.

## Bring-up order

1. Provide PostgreSQL, `DATABASE_URL`, `CCTUI_VAULT_KEY`,
   `CCTUI_ADMIN_TOKENS` and `CCTUI_EXTERNAL_URL` to the core server through
   your secret manager. The core host provisions its schema at startup; run
   one replica per database with a `Recreate` strategy. Core still requires
   PostgreSQL features including PL/pgSQL; Aurora DSQL is not a substitute.
2. Run `ghcr.io/dorskfr/cctui-core:0.27.1`, expose port 8700 and configure
   `/health` and `/ready` probes. Set the external URL to an address reachable
   by workers. Put caller ingress and authentication in front of the API.
3. Apply the release's `workerprofile-crd.yaml`. Deploy
   `ghcr.io/dorskfr/cctui-orchestrator:0.27.1` with webhook TLS, its Service
   on 8443, mutating/validating webhook registrations and fail-closed admission.
   Provide the `guard-proxy-env` and `guard-proxy-inject` ConfigMaps consumed
   by the envelope. Keep its sidecar image aligned with the worker release.
4. Create the worker namespace, ServiceAccount and network policies. Workers
   need core/gateway access, DNS and the public endpoints required by their
   tools and callback. Deny node metadata, the Kubernetes API and private
   networks as appropriate; ensure the worker's preflight probes pass with
   these policies. `posture: nonroot` drops worker privileges, but the injected
   `net-init` still needs `NET_ADMIN`: Pod Security `baseline`/`restricted`
   cannot admit that pod. Scope the exception to the worker namespace.
5. Adapt [the scheduling example](../deploy/examples/core-kubernetes/workerprofile.yaml):
   replace the namespace, node label and ServiceAccount with your installation's
   values. Create the named ServiceAccount before dispatching. Pin images by
   digest for production. A toleration permits placement on a tainted node;
   the matching node selector confines the workload to the intended group.
6. Enroll the dispatcher with `cctui-dispatcher-kube enroll`, including
   `--namespace`, `--default-profile` and a worker-reachable
   `--worker-cctui-url`. Persist its returned configuration as a Secret and
   run `ghcr.io/dorskfr/cctui-dispatcher-kube:0.27.1`. Its RBAC needs Jobs,
   Pods (including logs), per-dispatch Secrets and WorkerProfiles; an identity
   that can only create/delete Jobs is insufficient. See the operations in
   `crates/cctui-dispatcher-kube/src/spawn.rs` when scoping verbs.
7. Bootstrap service identity, provider accounts and a scoped caller key via
   the core API. Configure your trigger service with the dispatcher/profile
   and context pack. Test both directions: the caller must reach core, and
   the worker must reach the reply URL. A private callback needs an explicit
   network path and matching guard configuration.

For an upgrade, apply the CRD first, then update orchestrator and dispatcher,
then add `tolerations` to profiles. An older CRD prunes unknown fields and an
older dispatcher does not render them. Previously queued Jobs are rebuilt
when their tolerations drift; Jobs that already own a Pod finish with their
original template.

## Cluster acceptance

These checks require a disposable namespace and explicit access to the target
node group. Unit tests do not establish Bottlerocket or spot compatibility.

- Dispatch a short task with the scheduling example. Inspect its Job template
  and admitted Pod: both must carry the dedicated-node toleration and the Pod
  must land on a node matching the selector. Check that the worker runs as UID
  1000, `net-init` completes and the enforcing preflight passes.
- On Bottlerocket, verify the actual kernel supports the Landlock rules used
  by the worker and the runtime permits `net-init`'s iptables setup. Record
  the OS/kernel/runtime versions and the preflight result.
- For an end-to-end controlled-provider fixture, use
  `scripts/check-core-worker.py --kubeconfig <disposable-kubeconfig>
  --worker-url <fixture-core-url> --namespace <namespace> --profile <profile>
  --context-pack`. The fixture starts a local isolated core and provider;
  its advertised core URL must route back to that fixture, and the kubeconfig
  must be usable inside its dispatcher container. Set
  `CCTUI_TEST_DISPATCHER_IMAGE=ghcr.io/dorskfr/cctui-dispatcher-kube:0.27.1`
  to exercise this release rather than the script's compatibility baseline.
- Run a long disposable task, then interrupt its disposable worker node.
  Verify the run leaves running state, exposes the failure reason and releases
  its profile concurrency slot. Explicitly redispatch and check a fresh Job
  completes. Repeat with Pod eviction. Do not infer success from a node drain
  alone: abrupt node loss exercises different Kubernetes timing.
- Verify callback delivery and context-pack retrieval from an actual worker,
  plus rejection of the private endpoints your policy promises to block.

The dispatcher reports failed Pods (including eviction), failed Jobs and
missing Jobs; Kubernetes node-health detection and eviction timing still govern
when abrupt node loss becomes visible. Node-health `NoExecute` tolerations can
shorten the eviction delay after a taint is applied; they do not detect a lost
node themselves. Jobs use `backoffLimit: 0` and a bounded active deadline.
This does not guarantee exactly-once execution after a partition; make external
side effects idempotent and validate interruption behavior before production.
