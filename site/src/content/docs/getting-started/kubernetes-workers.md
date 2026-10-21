---
title: Run on Kubernetes workers
description: Dispatch agent tasks to throwaway Kubernetes pods with the kube dispatcher and WorkerProfile CRD.
---

A dispatcher turns a task into an ephemeral worker: one Kubernetes Job per dispatch, built from a `WorkerProfile`, with a sandboxed supervisor and an optional guard-proxy sidecar that holds the real credentials. The pod enrolls itself as an ephemeral machine, runs the agent, reports its result and disappears.

:::note[Released dispatchers]
The Kubernetes dispatcher is the one that ships today. Docker and Apple-container dispatchers exist in the repository but are not released.
:::

## Pieces

| Component | Image / resource | Role |
|---|---|---|
| `cctui-dispatcher-kube` | `ghcr.io/dorskfr/cctui-dispatcher-kube` | Enrolled dispatcher: connects out to the server, creates Jobs, reports status and exit diagnosis |
| `WorkerProfile` | CRD `cctui.dev/v1alpha1`, `deploy/workerprofile-crd.yaml` | Operator-authored pod shape: image, command, resources, env, mounts, guard settings |
| `cctui-orchestrator` | `ghcr.io/dorskfr/cctui-orchestrator` | Mutating webhook that injects the sandbox envelope, and a validating webhook that rejects unsafe pods and requests |
| worker image | `ghcr.io/dorskfr/cctui-worker` (`deploy/worker.Dockerfile`) | Implements the worker contract v1; derive your own image `FROM` it |
| `cctui-guard-proxy` | sidecar | Egress policy and phantom-token credential injection |

## Steps

1. **Install the CRD and the orchestrator webhooks.** Apply `deploy/workerprofile-crd.yaml`, then deploy the orchestrator and register its mutating and validating webhooks for pods carrying a profile. The injection webhook fails closed: a profiled pod it cannot process is rejected.

2. **Write a WorkerProfile.** Name the worker container `worker` (or set `spec.workerContainer`). Everything in `containers`, `initContainers` and `volumes` is passthrough app stack the webhook leaves alone.

   ```yaml
   apiVersion: cctui.dev/v1alpha1
   kind: WorkerProfile
   metadata:
     name: acme-default
     namespace: cctui-workers
   spec:
     image: ghcr.io/dorskfr/cctui-worker:latest
     resources:
       limits: { cpu: "2", memory: 4Gi }
     env:
       - name: CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES
         value: "90"
     serviceAccountName: acme-worker
     guard:
       env:
         - name: GUARD_PROXY_DENY_HOSTS
           value: "example.internal"
   ```

   A dispatch request selects a profile by name only; it never sets any field of the spec. Guard-proxy settings are layered over the cluster-wide `guard-proxy-env` ConfigMap; widening private egress requires `guard.allowPrivateEgress: true` so it is explicit in review.

3. **Enroll the dispatcher.** The dispatcher is enrolled like a machine: its key, namespace and default profile live in its config file (`CCTUI_DISPATCHER_CONFIG`, default `~/.config/cctui/dispatcher.toml`). Run the dispatcher image in the workers namespace with a service account that can create Jobs and Secrets. It shows up under **Settings › Dispatchers** once connected.

4. **Dispatch.** In the spawn panel choose the dispatcher instead of a machine, pick a profile, and launch. Agents can do the same through `CctuiAgent`. Failed dispatches land on the **Dispatch failures** page with the exit reason and the last log lines, and a one-click re-run.

## What the sandbox gives you

- A per-dispatch Secret, owned by the Job, carrying the machine key and env; deleted with the Job.
- Landlock, seccomp, dropped capabilities and a non-root exec through `cctui-supervisor`.
- Guard proxy in transparent (IPv4) or forward mode: every outbound connection is checked against policy; metadata endpoints and private ranges are denied by default.
- Phantom tokens: the agent holds placeholders; the proxy swaps in the real GitHub App token, API key or SigV4 signature on the wire. Secrets come from env, Vault/OpenBao, AWS Secrets Manager or Kubernetes and are cached only in memory.
- Remote GPG signing: workers can sign commits without the key ever leaving the sidecar.
- Per-user dispatch policy: admins limit which users may dispatch, and to what.
- Liveness: pods that OOM or crash-loop are ended cleanly and the session is marked failed.

The full environment, mount and callback contract is in `docs/worker-contract.md` in the repository; the CRD field ownership table is in `docs/worker-profile-crd.md`.
