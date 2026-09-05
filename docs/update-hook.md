# Updating cctui without an agent

cctui can update itself two ways. This document is about the one you should
prefer.

## Why there are two

There are as many ways to deploy cctui as there are people running it: a
Kubernetes Deployment behind a private registry, a Compose project on a NUC, a
systemd unit next to a reverse proxy, a Nix flake, an Ansible role. The server
cannot know which of those it is living in, and guessing wrong means breaking
the thing it is running on.

The original answer was to hand the problem to a YOLO agent: read this
machine's runbook, work out how this deployment updates, do it. That works, and
it is still the fallback. But it pays a model to re-derive the same answer every
time, it gives an agent write access to your infrastructure on a schedule you
did not choose, and when it goes wrong there is a transcript rather than an
error.

The better answer is that the knowledge already exists — the operator has it.
So write it down once, as one shell command, and cctui will run exactly that.

## The contract

Set `CCTUI_UPDATE_COMMAND` in the environment of the **cctui-daemon** running on
the machine that deploys cctui. That is the whole configuration:

```bash
CCTUI_UPDATE_COMMAND='/opt/cctui/update.sh'
```

The daemon then advertises, on every heartbeat, that this machine can update
this deployment deterministically. The webui's update button stops offering an
agent and offers the hook instead.

When an admin confirms an update, the daemon:

1. runs `CCTUI_UPDATE_COMMAND`, capped by `CCTUI_UPDATE_TIMEOUT_SECS`;
2. polls the health endpoint until it reports the target version, capped by
   `CCTUI_UPDATE_HEALTH_TIMEOUT_SECS`;
3. on any failure, runs `CCTUI_UPDATE_ROLLBACK_COMMAND` if one is set.

Step 2 is not a formality. `kubectl apply` exits 0 against an object that never
rolls out, `docker compose up -d` exits 0 when it pulled a cached image, and a
unit can restart straight back into the old binary. An update that did not move
the served version did not happen, and cctui says so rather than reporting
success.

### Every variable

| Variable | Default | Meaning |
|---|---|---|
| `CCTUI_UPDATE_COMMAND` | *(unset)* | The update command. Unset means this machine has no hook, and the agent fallback applies. |
| `CCTUI_UPDATE_ROLLBACK_COMMAND` | *(unset)* | Put the deployment back. Unset means a failed update reports `failed` and waits for a human, which is the right answer when you have no safe way back. |
| `CCTUI_UPDATE_DIR` | daemon's cwd | Working directory for both commands. |
| `CCTUI_UPDATE_TIMEOUT_SECS` | `900` | Cap on each command. |
| `CCTUI_UPDATE_HEALTH_URL` | `{server}/api/v1/daemon/version` | Polled until it reports the target version. |
| `CCTUI_UPDATE_HEALTH_TIMEOUT_SECS` | `600` | How long to wait for the new version to answer. |

Both commands run under `sh -c`, with three variables of their own:

- `CCTUI_UPDATE_VERSION` — the version to deploy, e.g. `0.7.319`, no `v` prefix.
- `CCTUI_UPDATE_RELEASE_URL` — the GitHub release page.
- `CCTUI_UPDATE_RUN_ID` — this run's id, handy for your own logs.

The last 8 KiB of the merged stdout/stderr is kept and shown in the webui, so
write your script's diagnostics to either stream and they will be readable
where the failure is.

## Recipes

Each of these is a complete `update.sh`. Keep it in your own repository, not in
cctui's: it describes your deployment, not this project.

### Kubernetes

```bash
#!/usr/bin/env bash
set -euo pipefail
NS=cctui
kubectl -n "$NS" set image deployment/cctui-server \
  server="registry.example.com/cctui-server:$CCTUI_UPDATE_VERSION"
kubectl -n "$NS" set image deployment/cctui-webui \
  webui="registry.example.com/cctui-webui:$CCTUI_UPDATE_VERSION"
kubectl -n "$NS" rollout status deployment/cctui-server --timeout=5m
kubectl -n "$NS" rollout status deployment/cctui-webui --timeout=5m
```

with the rollback Kubernetes already has:

```bash
CCTUI_UPDATE_ROLLBACK_COMMAND='kubectl -n cctui rollout undo deployment/cctui-server && kubectl -n cctui rollout undo deployment/cctui-webui'
```

The daemon needs a ServiceAccount that can patch **those** Deployments and read
their rollout status. Nothing else — that is the point. Compare it with what a
YOLO agent on the same machine can reach.

### Docker Compose

```bash
#!/usr/bin/env bash
set -euo pipefail
cd /srv/cctui
# Pin the tag the run asked for rather than following :latest, so the health
# check below is actually checking something.
CCTUI_TAG="$CCTUI_UPDATE_VERSION" docker compose pull
CCTUI_TAG="$CCTUI_UPDATE_VERSION" docker compose up -d
```

Rollback, given you know the previous tag:

```bash
CCTUI_UPDATE_ROLLBACK_COMMAND='cd /srv/cctui && docker compose up -d'
```

(with `CCTUI_TAG` unset, so Compose falls back to the pinned tag in your `.env`
— which you update only once the new version is verified.)

### systemd and a binary

```bash
#!/usr/bin/env bash
set -euo pipefail
cd /opt/cctui
curl -fsSL -o cctui-server.new \
  "https://github.com/DorskFR/cctui/releases/download/v${CCTUI_UPDATE_VERSION}/cctui-server-linux-amd64"
curl -fsSL -o SHA256SUMS \
  "https://github.com/DorskFR/cctui/releases/download/v${CCTUI_UPDATE_VERSION}/SHA256SUMS"
grep "cctui-server-linux-amd64" SHA256SUMS | sed 's|cctui-server-linux-amd64|cctui-server.new|' | sha256sum -c -
cp -f cctui-server cctui-server.old
install -m0755 cctui-server.new cctui-server
systemctl restart cctui-server
```

```bash
CCTUI_UPDATE_ROLLBACK_COMMAND='cd /opt/cctui && install -m0755 cctui-server.old cctui-server && systemctl restart cctui-server'
```

## Better still: do not use this at all

The hook exists because the button exists. If you would rather cctui had no say
in its own updates, that is a legitimate — arguably better — answer, and cctui
supports it by doing nothing:

- **Kubernetes**: point Flux or Argo CD image automation at the registry tags,
  or let Renovate raise the PR against your manifests.
- **Compose**: Watchtower, or `docker compose pull && up -d` on a timer.
- **Nix / Ansible / Puppet**: you already have a deployment pipeline; cctui is
  one more version pin in it.

Set `CCTUI_UPDATE_CHECK=0` and cctui stops probing for releases entirely, which
is also what an air-gapped deployment wants.

Every one of those is more auditable than anything cctui can do from the
inside, because the change lands as a commit before it lands on the cluster.

## What the fallback still costs you

With no hook on the target machine, the update button spawns a YOLO agent on it,
under the clicking admin's own accounts, on an Opus-class model (see
`launch_profile()` in `crates/cctui-server/src/routes/self_update.rs`). It reads
that machine's `CLAUDE.md` / `AGENTS.md` / notes and works the deployment out.

It is a real fallback for a deployment nobody has written a hook for yet, and it
is deliberately not the default answer for one that has. If you find yourself
watching an agent do the same six commands every release, that is your
`update.sh` — you have already written it, six commands at a time.
