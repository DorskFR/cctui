# cctui worker contract — v1

The contract between a **dispatcher** (the docker / kubernetes dispatcher that
spawns a worker per session) and the **worker image**
(`ghcr.io/dorskfr/cctui-worker`, built from `deploy/worker.Dockerfile`). It
defines every environment variable, the optional mounts, the capability
requirements per network mode, the result-callback wire shape, the hardening
report, and the context-pack layout. Derived-image authors
(`FROM ghcr.io/dorskfr/cctui-worker`) and dispatcher operators build against
this document.

Images that honor this contract carry the OCI label `dev.cctui.contract=1`.

## Plane ownership

Every variable belongs to one plane. The boundary is a security boundary, not a
convention:

- **platform** — set by cctui-server / the dispatcher core. The worker's
  identity and the daemon wiring. Never tenant-controlled.
- **operator** — set by the dispatcher operator in pod template / dispatcher
  config (the context pack, network exemptions, warm repos). Defines the
  sandbox the tenant runs inside. **Never** sourced from the task payload —
  whoever controls these controls the sandbox policy.
- **tenant** — carried in the dispatch payload (`TASK_PAYLOAD_JSON`). The tenant
  may select *what* runs (a prompt file, an identity) but never *the rules it
  runs under*.

## The degenerate worker (parity with the thin image)

With **only** the three platform vars set —

```
CCTUI_URL  CCTUI_MACHINE_KEY  SESSION_ID
```

— every entrypoint phase is skipped on absent input and the worker behaves
exactly like the pre-v1 thin image: it falls straight through to
`exec cctui-supervisor … -- cctui-daemon run --no-auto-update`. No phase
hard-fails on missing input. The **one** exception: if `CONTEXT_PACK_URL` is
set, the context-pack fetch must succeed or the boot fails closed (the pack
defines the guard rules; proceeding without it would weaken the sandbox).

## Environment variables

### Platform plane

| Var | Required | Meaning |
| --- | --- | --- |
| `CCTUI_URL` (or `CCTUI_SERVER_URL`) | yes | cctui-server base URL the daemon dials. Its host is always-allowed in the egress policy. |
| `CCTUI_MACHINE_KEY` | yes | Shared machine key; the daemon runs without enroll (`Config::from_env`). |
| `SESSION_ID` | yes | Pre-minted session id to register. |
| `TASK_PAYLOAD_JSON` | no | Opaque dispatch payload (tenant fields live inside it). |
| `TASK_NAME` | no | Human label for the session. |
| `TASK_ID` | no | Task id echoed in the synthesized failure callback. |

### Operator plane

| Var | Default | Meaning |
| --- | --- | --- |
| `WORKER_NET_MODE` | auto | `transparent` or `forward`. Auto = `transparent` when iptables is usable (CAP_NET_ADMIN), else `forward`. |
| `WORKER_NET_EXEMPT` | — | Comma-separated `host:port` exempt from the proxy (resolved to IPs, RETURN'd). Transparent mode only. |
| `WARM_REPO_DIR` | — | Mounted warm-repo dir; becomes the overlayfs lowerdir for `/workspace` (rsync-copy fallback). |
| `TASK_REPO_URL` | — | Repo to shallow-clone into `/workspace` when no warm repo. |
| `TASK_REPO_REF` | — | Branch/tag to clone (`git clone --depth 1 --branch`). |
| `CONTEXT_PACK_URL` | — | Git repo of the context pack. Set ⇒ fetch is **fail-closed**. |
| `CONTEXT_PACK_REF` | — | **Required when `CONTEXT_PACK_URL` set.** Branch/tag/sha — pin it. |
| `CONTEXT_PACK_TOKEN` | — | HTTPS basic token for a private pack (injected as `https://<token>@host`). Never logged. |
| `CONTEXT_PACK_SUBDIR` | — | Subdirectory within the pack repo to use as the pack root. |
| `GUARD_RULES_FILE` | `/opt/context/guard-rules.md` | Guard rules path; defaults into the fetched pack. |

### Tenant plane (from `TASK_PAYLOAD_JSON`)

| Var | Meaning |
| --- | --- |
| `TASK_PROMPT_FILE` | Prompt file; resolves under `/opt/context/prompts/` (absolute path honored as-is). Drives guard activation. |
| `TASK_IDENTITY` | Selects the credential env set (`GITHUB_TOKEN_<ID>`, …). Absent ⇒ image default. |
| `REPLY_URL` | Result-callback target (a bearer capability — never logged). Set ⇒ exit trap installed. Its host is always-allowed. |
| `RESULT_FILE` | Where the session writes its verdict. Default `/tmp/cctui-result.json`. |

### Credential env (materialized by `worker-credentials.sh`, all optional)

Suffix `<ID>` is `TASK_IDENTITY` uppercased with `-` → `_`. Absent ⇒ skipped
silently.

| Var | Effect |
| --- | --- |
| `GITHUB_TOKEN_<ID>` | `GITHUB_TOKEN`/`GH_TOKEN` + git credential helper `!gh auth git-credential`. |
| `GITHUB_NAME_<ID>` | `git config user.name`. |
| `GITHUB_EMAIL_<ID>` | `git config user.email`. |
| `GPG_PRIVATE_KEY_<ID>` | `gpg --import` + `user.signingkey` + `commit.gpgsign true`. |
| `NPM_TOKEN` | `~/.npmrc` registry auth (identity-independent). |
| `MCP_<NAME>_URL` / `MCP_<NAME>_TOKEN` | `~/.mcp.json` http server entry keyed by lowercase `<NAME>`. |

`ANTHROPIC_*` / `OPENAI_*` model auth is **not** handled here — it stays env
passthrough / gateway token, materialized by the platform exactly as before.

## Optional mounts

| Path | Mode | Purpose |
| --- | --- | --- |
| `WARM_REPO_DIR` (any path) | RO | overlayfs lowerdir / rsync source for `/workspace`. |
| `/workspace` | RW | The task working tree (overlay, clone, or empty). |
| `/opt/context` | RO after fetch | Context pack (prompts, docs, skills, guard rules). |
| `/home/worker` | RW | Per-session agent state (`.claude`, `.codex`, `.mcp.json`, `.npmrc`, `.gnupg`). |
| `/var/run/guard-proxy`, `/var/run/workflow-guard` | RW | Proxy policy + guard state. |
| `/tmp` | RW | Scratch, `RESULT_FILE`, hardening report. |

## Network modes & capability requirements

Egress is always gated by `cctui-guard-proxy` (uid 1337), deny-default. The
policy is seeded at boot to always-allow the `CCTUI_URL` + `REPLY_URL` hosts;
`cctui-guard` rewrites it per workflow step when a guarded prompt runs.

| Mode | Capability at start | Mechanism |
| --- | --- | --- |
| `transparent` (default w/ NET_ADMIN) | **CAP_NET_ADMIN** | iptables REDIRECTs worker-uid (1000) TCP egress to `:15001`. Exempts root, the proxy uid, loopback, DNS, the `CCTUI_URL` host, and `WORKER_NET_EXEMPT`. IPv6 egress denied (proxy is IPv4-only — forces IPv4 fallback). |
| `forward` (no NET_ADMIN) | **none** | No iptables. `HTTP_PROXY`/`HTTPS_PROXY=http://127.0.0.1:15001` exported for the worker tree; `NO_PROXY=127.0.0.1,localhost`. For rootless Docker / gVisor / Apple container. |

The proxy listens on `:15001` (traffic) and `:15002` (`/health`, `/ready`).
Capabilities are dropped entirely before the daemon runs — see hardening below.

## Result callback (tenant-visible wire shape)

When `REPLY_URL` is set, an EXIT/INT/TERM trap POSTs the result exactly once
(`Content-Type: application/json`, IPv4-forced, 30s max). It prefers a valid
`RESULT_FILE` the session wrote; otherwise it synthesizes a failure. This wire
shape is identical to the homelab `automation-contract.md` and must stay so — callers
branch on the structured fields, never on prose.

Common envelope (every flow):

```jsonc
{
  "task_id":   "<from payload, if present>",
  "flow":      "<this flow's name>",
  "status":    "success",   // success | failed | needs_human
  "dedup_key": "<from payload, if present>",
  "error":     null         // one-line reason when status != success
}
```

Synthesized failure (no valid `RESULT_FILE`):

```json
{ "task_id": "<TASK_ID>", "status": "failed", "error": "worker exited (code N) without a valid result" }
```

Flows add their own fields on top (e.g. `review-pr` adds `review_id`, `verdict`,
`review_body`). Do not invent fields a flow does not consume. `REPLY_URL` is a
bearer capability — whoever holds it can forge the verdict — so it is never
logged.

## Hardening report

Two parts, surfaced for the daemon to attach as session metadata:

- **Entrypoint state** — `WORKER_HARDENING_JSON` (env) and
  `WORKER_HARDENING_FILE` (`/tmp/cctui-hardening.json`):

  ```json
  { "net_mode": "transparent", "guard": "on", "supervisor_report": "/tmp/hardening.json" }
  ```

  `net_mode` ∈ `transparent|forward`; `guard` ∈ `on|off`.

- **Supervisor report** — `cctui-supervisor --report /tmp/hardening.json`:

  ```json
  {
    "landlock": "V5 (fully-enforced)",
    "seccomp_applied": true,
    "seccomp_blocked": ["ptrace", "..."],
    "caps_dropped": true,
    "uid": 1000,
    "ro_paths": ["/usr", "..."],
    "rw_paths": ["/tmp", "..."],
    "command": ["cctui-daemon", "run", "--no-auto-update"]
  }
  ```

  `landlock` is `"unavailable"` when the kernel does not enforce the ruleset.

## Entrypoint phases

The container starts as root, runs these phases (each skippable on absent
input), then drops to uid 1000:

1. **Network** — choose mode, seed the deny-default policy, install iptables
   (transparent) or export proxy env (forward), start `cctui-guard-proxy`.
2. **Workspace** — overlayfs/rsync from `WARM_REPO_DIR`, else shallow-clone
   `TASK_REPO_URL`, else empty `/workspace`; chown worker.
3. **Context pack** — fetch the pinned ref to `/opt/context` (fail-closed when
   `CONTEXT_PACK_URL` set); wire `CLAUDE.md`/skills/style/projects into the
   worker home; default `GUARD_RULES_FILE`.
4. **Credentials** — `worker-credentials.sh` materializes the per-identity
   github/gpg/npm/mcp config into the worker home (no-op when absent).
5. **Callback** — install the `REPLY_URL` exit trap.
6. **Guard** — start `cctui-guard` if the resolved prompt has step blocks
   (`# Step N` + `[allowed]`); always-allow the structural hosts.
7. **Hardening** — assemble `WORKER_HARDENING_JSON`.
8. **Drop + run** — `exec cctui-supervisor --ro … --rw … --user 1000
   --report /tmp/hardening.json -- cctui-daemon run --no-auto-update`.

## Context pack

A git-hosted bundle of prompts/docs/skills/guard-rules the worker fetches at
boot, replacing per-cluster ConfigMaps as the universal way to give a dispatched
agent its documentation environment. Anyone can define their own dispatch types
(prompts + skills + rules) with a pack — no image builds, no k8s.

### Layout

```
<pack repo root or CONTEXT_PACK_SUBDIR>/
  CLAUDE.md          # home-level instructions  -> /home/worker/CLAUDE.md
  prompts/           # dispatch prompts; TASK_PROMPT_FILE resolves here
  docs/              # reference docs exposed to the agent
  skills/            # skill dirs (SKILL.md …)   -> ~/.claude/skills/
  projects/          # per-repo CLAUDE.md overlays -> /home/worker/projects/
  style/             # output styles               -> /home/worker/style/
  guard-rules.md     # tool-set + network-set definitions for cctui-guard
```

A neutral fixture pack lives at `deploy/examples/context-pack/`.

### Mechanics

- The fetch is `git clone --depth 1` (or a depth-1 fetch of a SHA) of
  `CONTEXT_PACK_REF` into `/opt/context`, **root-side, before guard lockdown and
  before the agent can write anything**. `/opt/context` is read-only under
  landlock afterwards.
- It happens **pre-lockdown from the root side**, so no egress policy hole is
  needed for the pack host.
- Seams: `TASK_PROMPT_FILE` resolves under `/opt/context/prompts/`;
  `GUARD_RULES_FILE` defaults to `/opt/context/guard-rules.md`; the pack's
  `CLAUDE.md`/skills/style/projects are copied to the locations the agent
  expects.

### Security rationale

Whoever controls the pack controls the sandbox policy (`guard-rules.md` defines
the tool/network sets the guard enforces). Therefore the pack URL+ref is
**operator-plane** config, pinned per dispatcher. The task payload may select a
*prompt file within* the pack but **never the pack itself**. A configured-but-
unfetchable pack fails the boot closed — silently proceeding without it would
weaken the sandbox.

## Intent+Acceptance ratify gate

An implementation prompt should make its **first** step an Intent+Acceptance
ratify gate — a structural pause that surfaces what the agent thinks "done"
means before it writes any code. The pattern lives in the example pack as the
`intent-acceptance` skill plus the first step of `prompts/example-task.md`; a
real pack adapts it to its dispatch types.

After gathering context (task, linked discussion, relevant code), the agent
emits a small **Intent+Acceptance artifact**:

```yaml
intent: >
  <one or two sentences: the outcome that means "done">
acceptance:
  - <checkable success condition>
  - <checkable success condition>
surfaces: [<class>, ...]   # pure-calc | frontend | backend | external-api
                           # | webhook | payments | brand-visible
blast_radius: <low|medium|high>   # max over surfaces
ratify: <auto|human>              # auto only when blast_radius == low
```

The gate is enforced by the guard, not by convention: the early step's
`[transition]` is the only path into the implement step, so the agent cannot
reach implementation without passing through it. Ratification routes back to the
human via the **`needs_human`** result-callback status (see *Result callback*)
carrying the artifact; a corrected intent replaces the artifact and becomes the
spec. Changes whose surfaces are **all** low blast radius (`pure-calc`)
auto-ratify and skip the round-trip.

The artifact is **persisted and reused verbatim** as the acceptance script at
the end of the run — the success condition promised up front is the one the
deliverable is checked against — and is attached to the session + PR.
