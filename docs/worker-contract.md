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
| `WORKER_NET_EXEMPT` | — | Comma-separated `host:port` that **bypass** the proxy (resolved to a single IP at boot, iptables `RETURN`'d). Transparent mode only. IP-pinned — use only for **IP-stable** hosts; CDN/multi-IP hosts will rotate off the exempted IP. |
| `WORKER_NET_ALLOW` | — | Comma-separated `host:port` allowed **through** the proxy by **SNI** (IP-independent). Folded into the seeded `policy.json` and re-applied as `cctui-guard --always-allow` so it survives per-step rewrites. The right tool for CDN/multi-IP SaaS APIs (e.g. a YouTrack host). |
| `WARM_REPO_DIR` | — | Mounted warm-repo dir; becomes the overlayfs lowerdir for `/workspace` (rsync-copy fallback). |
| `TASK_REPO_URL` | — | Repo to shallow-clone into `/workspace` when no warm repo. |
| `TASK_REPO_REF` | — | Branch/tag to clone (`git clone --depth 1 --branch`). |
| `CONTEXT_PACK_URL` | — | Git repo of the context pack. May carry `@<ref>` (path) and `#<subdir>` to pin in one value. Set ⇒ fetch is **fail-closed**. |
| `CONTEXT_PACK_REF` | default branch | Branch/tag/sha. Optional — absent ⇒ the remote's default branch. Pin it in prod. Overrides any `@<ref>` in the URL. |
| `CONTEXT_PACK_TOKEN` | — | HTTPS basic token for a private pack (injected as `https://<token>@host`). Never logged. Falls back to `payload.env.GITHUB_TOKEN` when unset, so a tenant can ship one token for pack-clone + repo clone/push. |
| `CONTEXT_PACK_SUBDIR` | — | Subdirectory within the pack repo to use as the pack root. |
| `CONTEXT_PACK_TOKEN_FROM` | — | Name of an env var holding the pack-clone token, when it differs from the task identity's (e.g. the task identity can't read the pack repo). Resolved before the `GITHUB_TOKEN` fallbacks; keeps a specific identity name out of the image. |
| `GUARD_RULES_FILE` | `/opt/context/guard-rules.md` | Guard rules path; defaults into the fetched pack (the override/extend layer). |
| `GUARD_RULES_BASE` | — | Operator base guard-rules parsed **before** `GUARD_RULES_FILE`. When a pack ships `guard-rules.md`, the entrypoint moves any prior `GUARD_RULES_FILE` here so the pack reuses/extends/overrides it. |

### Tenant plane (from `TASK_PAYLOAD_JSON`)

| Var | Meaning |
| --- | --- |
| `TASK_PROMPT_FILE` | Prompt file; resolves under `/opt/context/prompts/` (absolute path honored as-is). Drives guard activation. When a pack is active (`CONTEXT_PACK_URL` set) and this is unset, the entrypoint derives it from `payload.prompt_file` so the guard engages on the pack's prompt; legacy (no-pack) dispatches are left unguarded as before. |
| `TASK_IDENTITY` | Selects the credential env set (`GITHUB_TOKEN_<ID>`, …). Absent ⇒ image default. |
| `REPLY_URL` | Result-callback target (a bearer capability — never logged). Set ⇒ exit trap installed. Its host is always-allowed. |
| `RESULT_FILE` | Where the session writes its verdict. Default `/tmp/cctui-result.json`. |

### Credential env — identity resolve + scrub

The single secret surface: the pod env carries **every** identity's secrets as
`VAR_<ID>` (Vault env-from-path) plus optional unsuffixed defaults. `TASK_IDENTITY`
(derived from `payload.identity`) selects the active one, `<ID>` = uppercased with
`-` → `_`. The entrypoint then, around `worker-credentials.sh`:

1. **resolve** (before the helper): `VAR = ${VAR_<ID>:-$VAR}` for each secret base
   (`GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_NAME`, `GITHUB_EMAIL`, `GPG_PRIVATE_KEY`,
   `YOUTRACK_TOKEN`, `YOUTRACK_API_TOKEN`, `SLACK_TOKEN`) — so the helper and the
   agent read one canonical var.
2. **scrub** (after the helper): unset every `VAR_<…>` suffixed variant, so the
   agent process inherits only the resolved mains — never another identity's
   secret. (Trade-off: the *pod* still holds all secrets; only the agent's env is
   narrowed. Per-payload pod-level scoping is a later step.)

`worker-credentials.sh` then materializes the **canonical** vars (all optional,
skipped silently when absent):

| Var (canonical) | Effect |
| --- | --- |
| `GITHUB_TOKEN` | `GH_TOKEN` + git credential helper `!gh auth git-credential`. |
| `GITHUB_NAME` | `git config user.name`. |
| `GITHUB_EMAIL` | `git config user.email`. |
| `GPG_PRIVATE_KEY` | `gpg --import` + `user.signingkey` + `commit.gpgsign true`. |
| `NPM_TOKEN` | `~/.npmrc` registry auth (identity-independent). |
| `MCP_<NAME>_URL` / `MCP_<NAME>_TOKEN` | `~/.mcp.json` http server entry keyed by lowercase `<NAME>`. |
| `YOUTRACK_URL` + `YOUTRACK_API_TOKEN` (or `YOUTRACK_TOKEN`) | `~/.config/yt/config.json` for the bundled `yt` YouTrack CLI. Reaching the YouTrack host also needs it on the egress allow-list. |

`ANTHROPIC_*` / `OPENAI_*` model auth is **not** handled here — it stays env
passthrough / gateway token, materialized by the platform exactly as before.

### Secret references in `payload.env` (CCT-490)

The dispatch payload's `env` map is the **one secret-injection surface**, and it
carries **references, never secret values**. The kube dispatcher lifts
`payload.env` out of `TASK_PAYLOAD_JSON` and promotes each entry to **pod
container env**, choosing the form by value prefix:

| `payload.env` value | becomes | resolved by |
| --- | --- | --- |
| `vault:<path>#<field>` | literal env var | the in-cluster **vault-env** webhook, at exec — before the entrypoint |
| `k8s:[<ns>/]<secret>#<key>` | `valueFrom.secretKeyRef` (namespace prefix dropped; must exist in the pod's ns) | the kubelet, at pod start |
| anything else | literal env var | passthrough (plain value / test override) |

So by the time the entrypoint runs, every var holds a **real value** — the
dispatcher never reads the secret, and a `vault:`/`k8s:` value never lands in the
Job spec or etcd. The boundary is the worker's Vault role scope (tenant prefix)
and the namespace's k8s secrets, not the reference string (which is untrusted).
The entrypoint stays product-aware (wires `gh`/`scli`/`yt`, `gpg --import
$GPG_PRIVATE_KEY`); the dispatcher stays secret-agnostic. `env` is stripped from
`TASK_PAYLOAD_JSON` so the daemon cannot re-apply an unresolved reference over
the resolved pod env.

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

An implementation flow that opens a PR carries a required **`evidence[]`** array
alongside the envelope — the proof that backs a `success` verdict (see *Evidence-
required done gate*). Each entry is `{ kind, surface, summary, detail }`:

```jsonc
{
  "task_id": "…",
  "flow":    "implement",
  "status":  "success",
  "evidence": [
    { "kind": "test-run", "surface": "pure-calc",
      "summary": "all 14 parser tests pass",
      "detail":  "$ cargo test -p cctui-guard\n… 14 passed; 0 failed" },
    { "kind": "diff", "surface": "pure-calc",
      "summary": "adds evidence kind to the parser",
      "detail":  "@@ … @@\n+…" }
  ]
}
```

`evidence[]` is **required to be non-empty for a `success` finalize** on a flow
that touches code — a `success` callback with an empty (or absent) `evidence[]`
is a contract violation; the gate must instead report `needs_human` with what is
blocked. The array is consumed by the PR renderer (one section per surface) and,
longer-term, by `cctui-github` (diff viewer + review-draft store + evidence
attachments) — the not-yet-built crate is the natural home for richer rendering.

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
  rules/             # always-on guidance (push) -> ~/.claude/rules/ (auto-loaded)
  docs/              # on-demand reference (pull) -> ~/.claude/docs/
  prompts/           # dispatch prompts; TASK_PROMPT_FILE resolves here
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
  `CLAUDE.md`/rules/docs/skills/style/projects are copied to the locations the
  agent expects.
- **Home isolation:** `/home/worker` is a ReadWriteMany volume shared by
  concurrent workers, so when a pack is active the entrypoint bind-mounts a
  per-pod dir (under the `/overlay` emptyDir) over the paths the pack overwrites
  (`~/CLAUDE.md`, `~/projects`, `~/style`) before copying — the pack's writes
  stay private to the pod and never mutate the shared copy. `~/.claude` is
  already a per-pod emptyDir.
- **Guard-rules layering:** a pack's `guard-rules.md` is parsed as a layer on top
  of `GUARD_RULES_BASE` (the operator base). `[name]: …` overrides a base set;
  `[name]+: …` extends it (appends); a new name adds a set. So a pack reuses
  common sets (`net-dev`, …) and only states its deltas.
- `rules/` vs `docs/` (CCT-490): `rules/` is **push** — copied to
  `~/.claude/rules/`, which Claude Code auto-loads as instructions on every task
  (always-on guardrails/conventions). `docs/` is **pull** — copied to
  `~/.claude/docs/` and referenced on demand by a prompt that needs it
  (`@~/.claude/docs/<x>.md`); it is not auto-loaded.

### Security rationale

Whoever controls the pack controls the sandbox policy (`guard-rules.md` defines
the tool/network sets the guard enforces). Therefore the pack URL+ref is
**operator-plane** config. A configured-but-unfetchable pack fails the boot
closed — silently proceeding without it would weaken the sandbox.

Precedence (entrypoint `phase_context_pack`):

1. **Pod template env** — the true operator plane. If the worker pod template
   sets `CONTEXT_PACK_URL` (etc.), that value wins and the payload cannot
   override it. Pin the pack here when the dispatcher must not be trusted to
   choose it.
2. **Dispatch payload `env`** — fallback, used only for a `CONTEXT_PACK_*` var
   the template leaves unset. This lets an **operator-controlled** dispatcher
   (e.g. the automation flows, which set `payload.env.CONTEXT_PACK_URL/REF/TOKEN`)
   select the pack per dispatch without baking it into the template. Because the
   pack defines the guard fence, this is only safe when the dispatch channel
   itself is operator-controlled — do **not** expose it to an untrusted tenant.
   A template that pins the pack (step 1) closes this door.

The task payload may also select a *prompt file within* the pack
(`TASK_PROMPT_FILE`).

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

## Conditional classifier pipeline

The pipeline is effectively linear, but **which** oracles run, **how** autonomous
the run may be, and **whether** a human must sign off all depend on what the task
touches. Between the Intent+Acceptance gate and implementation, an implementation
prompt should run a **classifier** that turns the ratified `surfaces[]` into a
pipeline plan. The pattern lives in the example pack as the `classify-surface`
skill plus the second step of `prompts/example-task.md`.

The plan is a **pure, deterministic function of the surface set** — same surfaces
always yield the same plan, so the routing is auditable and the agent cannot talk
itself into a softer gate:

```yaml
plan:
  oracles: [<skill>, ...]        # the oracle (verification) skills the surfaces demand
  autonomy: <auto-merge|human-gate>
  brand_gate: <true|false>
  required_evidence: [<kind>, ...]   # union over surfaces; feeds the evidence gate
```

Each surface class maps to a fixed row (oracles, autonomy, brand gate, required
evidence); a multi-surface change takes the **union** of the oracle/evidence
columns and the **strictest** autonomy. The `classify-surface` skill carries the
full table. Two routing decisions matter:

- **Autonomy** — `auto-merge` is granted **only** when *every* surface is
  `pure-calc`: a deterministic change backed by golden tests can merge on green
  without a human, because the oracle is the reviewer (this mirrors the ratify
  gate's `blast_radius: low` ⇔ auto path). `payments`, auth, and migrations are
  **always** `human-gate` regardless of how green the oracles are — a silent
  auto-merge there is the most dangerous case.
- **Brand / taste gate** — `brand_gate: true` whenever a `brand-visible` surface
  is present. Taste (copy, layout, pricing, naming, emails) cannot be oracle'd,
  only routed: no test asserts that wording is on-brand, so the change routes to
  a human for a sign-off that is explicitly *not* a correctness check. The brand
  gate is independent of and additional to the autonomy gate.

The plan conditions the rest of the run: implementation runs exactly the
`oracles[]` selected; the evidence gate demands at least the `required_evidence`
union; and the finalize step grants the `auto-merge` capability **only** when the
plan says so — otherwise the finalized PR routes to a human via the `needs_human`
callback for the merge decision (the PR is opened, the merge is not auto). A
`brand_gate: true` plan additionally routes the rendered copy/layout to a human
taste sign-off, independent of the merge decision. The guard enforces the
capability split, not convention.

## Per-surface oracle skills

Tasks are novel, but the **surfaces** they verify against are a small fixed set,
so the verification harness can be reused per surface instead of hand-built each
time. The classifier names an `oracles[]` set per surface (see the table in the
`classify-surface` skill); each name resolves to an **oracle skill** in the pack
that exercises that surface in-pod against test-mode / replay and produces the
evidence the gate later demands. The example pack ships one oracle skill per
surface class:

| Surface | Oracle skill | Exercises | Evidence |
| --- | --- | --- | --- |
| `pure-calc` | `golden-tests` | unit + property tests + golden-file diffs (no I/O) | `test-run` |
| `frontend` / `brand-visible` | `render-check` | headless-browser render of the changed UI in-pod | `screenshot` / `video` |
| `backend` | `endpoint-tests` | route/job against a throwaway test instance in-pod | `test-run` |
| `external-api` | `roundtrip-check` | third party against test-mode / VCR cassette + contract tests | `transcript` |
| `webhook` | `contract-check` | recorded payload fixtures replayed at the handler (sig / idempotency) | `transcript` |
| `payments` | `roundtrip-check` + `contract-check` | outbound charge (test mode) + inbound event (replayed) | `transcript` |

Three properties make the oracles safe to run unattended in the pod:

- **Test-mode / replay, never prod.** No oracle touches a third party's
  production endpoint. `roundtrip-check` replays a recorded cassette (or hits the
  provider's sandbox); `contract-check` replays checked-in payload fixtures at
  the local handler. The cassettes/fixtures/goldens are reviewed code — a
  re-recorded one shows up in the `diff`, and unexplained churn is a red flag.
- **Net-allow scoped per surface.** The guard grants each oracle only the
  network its surface needs: `pure-calc`/`webhook` get no third-party host at
  all (local goldens / replayed fixtures), `frontend`/`backend` reach only the
  loopback dev server, and `external-api`/`payments` get the provider's
  **sandbox** host — never its production host. `guard-rules.md` carries the
  per-surface `net-*-sandbox` sets. A change that needs a host its surface's set
  does not grant was mis-classified — stop and re-run `intent-acceptance`.
- **Oracle ⇒ autonomy.** `golden-tests` is the only oracle whose green run
  unlocks `auto-merge` (the surface is deterministic, the oracle *is* the
  reviewer). Every other oracle proves the behaviour *works* but not that it is
  *wanted*, so its surface stays `human-gate` regardless of how green the run is.

Step 3 of an implementation prompt runs **exactly** the `oracles[]` the
classifier selected — no more, no fewer — and the evidence they emit is what the
done gate consumes. A multi-surface change runs the union of oracles.

## Evidence-required done gate

The mirror of the ratify gate at the *other* end of the run. The Intent+
Acceptance gate catches a misread before code is written; the evidence gate stops
a confident-but-wrong deliverable from being finalized on an **assertion** —
"evidence, not assertions". An implementation prompt should make its **last** step
an evidence gate that refuses the finalize / open-PR transition until the result
carries a populated `evidence[]` for the surfaces the change touched. The pattern
lives in the example pack as the `evidence-gate` skill plus the final step of
`prompts/example-task.md`.

After the change is made and the deployed change is run against the Acceptance
script (verbatim, from Step 1), the agent assembles `evidence[]` — typed,
self-contained artifacts that *show* each acceptance condition holds:

- `test-run` — the command + its full output (exit status visible).
- `diff` — the unified diff / key hunks (required for every surface; bounds what
  was touched).
- `screenshot` / `video` — UI behaviour for `frontend` / `brand-visible`.
- `transcript` — the real round-trip for `external-api` / `webhook` / `payments`.
- `coverage` — the coverage delta where a `test-run` is required (never a
  substitute for one).

The required kind(s) are keyed per surface class (`pure-calc` → `test-run`+`diff`,
`frontend` → `screenshot`+`diff`, `external-api`/`webhook`/`payments` →
`transcript`+`diff`, etc.); the `evidence-gate` skill carries the full table.

The gate is enforced by the guard, not by convention: `remote-write` (push /
open-PR) is granted **only** in the final step, so the agent cannot finalize from
an earlier step — the evidence step is the only door to a PR. A `success` callback
on a code-touching flow **must** carry a non-empty `evidence[]` (see *Result
callback*); a deliverable that cannot produce its required evidence reports
`needs_human` instead. The evidence is rendered on the PR body (one section per
surface) so human review is a glance at the proof, not a re-run of the app.

## Guard hardening: re-injection + deterministic transition gates

Long sessions drift. Past the halfway mark, the agent's working context dilutes
(the "dumb zone") and any ticket, comment, or web page it fetched is sitting in
that context as if it were an instruction — a prompt-injection surface. And every
step transition so far has trusted the agent's *claim* that the step is done.
CCT-440 hardens both ends of a transition; both are enforced by the guard, not by
convention.

**Re-inject the step on every transition.** A numeric transition (and the
`SessionStart`/compact hook) returns the **authoritative next-step prompt body
verbatim** — the trusted instructions from the pack, not the agent's own summary.
The agent re-anchors on the trusted spec instead of a diluted or injected one.
The body is captured from the prompt step's prose lines; no annotation is needed.

**Compaction is opt-in (`[compact]`).** A step may add a `[compact]` line to also
emit a directive to compact the working context to `{plan, current diff, the step
instructions}` and to treat any fetched ticket/comment/web content as untrusted
input rather than instructions. It is **off by default** (CCT-450): compaction is
lossy and counter-productive on large-context models, so re-injection re-anchors
context without discarding it unless the step explicitly asks for it. Bare
`[compact]` turns it on; `[compact]: false` (or `no`/`off`/`0`) keeps it off so a
template can carry the line and toggle it per task.

**Deterministic transition gates.** Where completion is machine-checkable, the
step carries a `[gate]: <command>` annotation — a deterministic check the guard
runs (in the worker's `/workspace`) before it will allow the transition *out* of
that step. A non-zero exit **refuses** the transition and returns the command's
output; the agent cannot advance past a finalize-type step on an assertion of
"done". This is the structural form of "evidence, not assertions": prefer a gate
(`cargo test`, an artifact check, CI status) wherever the outcome is
deterministically checkable, and reserve the adversarial-agent validator only for
transitions that are *not* — a judge sharing the agent's blind spots is a weaker
gate than a green test. `Exit` always bypasses the gate (bail-out must work; the
agent reports the blocked outcome via the `needs_human` callback rather than
finalizing).

The two compose with the existing gates: the Intent+Acceptance ratify gate
(Step 1) catches a misread before code; the per-surface oracles (Step 3) produce
evidence; a `[gate]` makes the *transition into finalize* refuse to fire unless
that evidence is real; and the re-injection keeps the agent anchored on the
ratified spec the whole way. The example pack wires a `[gate]` on the implement
step of `prompts/example-task.md`.

## Consistency gates + discoverability

The oracles prove the change *works* and the evidence gate proves it is
finalized on proof — but neither catches the change that is **correct yet
inconsistent**: a banned API, a crossed layering boundary, logic re-implemented
that already exists. That class is ~80% **human-caught** at review (the
review-mining finding); the bot passes correctness but is blind to "we already
have this" and "we don't do it that way here". These are not verification
failures — they are **retrieval** (the agent didn't know the helper existed) and
**codification** (the convention lived in a reviewer's head) failures. CCT-441
closes both, and the close lives in the repo so it protects humans too.

**Deterministic consistency gates (codification + detection).** Three
repo-resident gates run in the implement step and chain into its `[gate]`:
structural lint (`ast-grep` / Semgrep — banned APIs, house patterns), layering
boundaries (`dependency-cruiser`), and copy-paste detection (`jscpd`). A
correct-but-inconsistent change cannot transition to finalize until they are
green; a violation is fixed (not suppressed), and a **recurring** one is codified
as a **one-rule change in the same diff** — the *ratchet* that turns a recurring
review comment into a mechanical gate. The pattern is the `consistency-gates`
skill plus the implement step's `[gate]` (a real pack chains `make
consistency-check` into the oracle gate). A suppressed rule is a `judgment`
decision, visible in the `diff` evidence.

**Prior-art search (retrieval, up front).** jscpd catches a *paste* after the
fact but cannot catch a *reinvention* — code written from scratch because the
agent never knew the helper existed. So a **generated helper/utility index**
(`docs/helper-index.md`, regenerated in CI from an export/doc-comment scan, never
hand-maintained) makes the repo's reusable surface discoverable, and the
implement step requires a **cite-prior-art** sub-step: before introducing any new
util/type/component, the agent searches the index and either cites the existing
one it will reuse or justifies a new one (`prior_art:` block). The pattern is the
`prior-art` skill. The two bracket reinvention from both sides — the citation
makes the agent reuse before it writes, the gate catches it if it pasted anyway.

The acceptance for the epic: a new mechanical convention becomes a one-rule PR,
not a recurring review comment, and the implement step cites prior art before
introducing a new utility.

## Comment handling (classify, defend-don't-cave)

The run does not end when the PR opens — reviewers comment, and the agent has to
act on those comments without **capitulating** to them. The dangerous failure
mode is not ignoring a comment but rewriting good, evidence-backed code because a
comment was *raised*: LLMs flip a correct answer to an incorrect one under mild
pushback at a measurable rate (~15% in published sycophancy evals), and the
*format* of the objection (confident tone, authority framing) drives the cave
more than its substance. The pattern lives in the example pack as the
`comment-handling` skill; a real pack adapts it to its review tooling.

The skill is the inbound mirror of the evidence gate — the gate proved the
deliverable right when the PR opened; this keeps it right while the PR is
reviewed. It runs **per inbound comment** and is deterministic on the comment's
class, so the agent cannot talk itself into "just make the change" for a judgment
comment any more than it could into a softer merge gate:

```yaml
comment:
  class: <mechanical|judgment|unclear>   # pure function of WHAT is asked, not WHO
  action: <auto-fix|defend-or-propose|escalate>
```

- **`mechanical`** (objective, one right answer — lint, rename, dead code, a
  *demonstrated* bug) → **auto-fix**, re-run the surface's oracle so the fix stays
  evidence-backed, reply with the commit, resolve the thread. A bug *asserted*
  without a failing case is `judgment` (ask for the repro), not `mechanical`.
- **`judgment`** (taste, scope, architecture, "is this worth it") → **propose or
  defend, never silently comply**. Reply with a reason that ties back to the
  ratified Intent/Acceptance and the evidence; the default for a judgment comment
  on code that already passed the gate is to **hold**. A proposal that alters
  scope/surfaces re-opens the `intent-acceptance` gate — it is not silently
  absorbed.
- **`unclear`** / contradicts the ratified Acceptance → **ask or `needs_human`**;
  a spec dispute is resolved by a human, not by changing code.

Two structural guards keep this safe: the **human merge gate is never bypassed**
(the skill answers and fixes; it does not merge — a capitulating agent still
cannot ship, so the only thing caving can damage is quality), and an unresolved
judgment thread **escalates** rather than churns — after one reasoned round-trip
the agent reports `status: "needs_human"` carrying both positions instead of
entering an edit/revert loop under repeated pushback.

> **Auto-address loop (server-side, not yet wired).** The webhook ingress
> (`crate::webhook` in `cctui-github`) already parses + stores
> `pull_request_review` / `pull_request_review_comment` events but does **not**
> dispatch a worker from them — there is no event→dispatch mechanism in the
> connector today. Wiring the review-comment event to dispatch this skill is a
> separate server change tracked outside this contract; the worker-side behaviour
> (classify + defend-don't-cave) is what the `comment-handling` skill specifies
> and is reusable the moment that loop exists.

## Deliverable-acceptance agent + preview environments

The evidence gate proves the deliverable on artifacts the **implementer** assembled
— necessary, but the implementer shares its own blind spots and an incentive to
declare success, and its evidence is built from the source tree, not a running
deployment. The one oracle neither the guard, the cross-model review, nor the
implementer's own evidence covers is *does the change actually do the intended
thing, end to end, in a running deployment*. Today a human fills that gap by
checking out the branch and exercising it in the dev stack — the primary
bottleneck in both flows. This step moves that check into the pipeline as an
attached **independent verdict**, so the human reviews a result instead of
re-running the app.

It is the **deployed-end mirror** of the bracketing gates: `intent-acceptance`
states the success condition up front, the evidence gate makes the implementer
prove it at finalize, and the **deliverable-acceptance agent** re-confirms it
against the live deployment from a clean context. The pattern lives in the example
pack as the `acceptance-agent` skill.

**Clean context — it cannot mark its own homework.** The acceptance run is
executed in a context that **never saw the implementation**: it is handed the
ratified Intent+Acceptance artifact only (the `acceptance[]` conditions and the
`surfaces[]`), **not** the diff, the implementer's transcript, or the implementer's
`evidence[]`. It re-derives a concrete test plan from each acceptance condition by
itself and drives the **deployed preview** — Playwright for a `frontend` /
`brand-visible` surface, HTTP for a `backend` / `external-api` / `payments`
surface, a payload replay for a `webhook` — never the source tree. It can only
emit a verdict + evidence; it cannot push, edit code, or merge. Separating the
grader from the author is the whole point — that is the structural reason this
verdict is worth more than the implementer's self-assertion.

It runs **when the change has observable deployed behaviour** (any medium/high
blast-radius surface — exactly the classes a human used to check out and test). A
`pure-calc`-only change is skipped: there is nothing deployed to drive and the
deterministic golden test already is the end-to-end proof. The output is an
`acceptance_run` block — a per-condition `pass|fail` plus the screenshot /
transcript / driver-output that backs it, in the same `{kind, surface, summary,
detail}` evidence shape so it renders on the PR body alongside the implementer's.
A `fail` blocks the merge regardless of what the implementer asserted; the agent
**never** edits code to make an assertion pass. The independent verdict and the
implementer's evidence gate are **composed** — both must be green for an
unattended merge.

> **Per-PR preview environments (infra — not the context pack).** The skill
> assumes a deployed preview exists at a `PREVIEW_BASE_URL` and drives whatever it
> is handed; **standing up that environment is infra, not part of the pack.** It
> needs the cluster (k8s + ArgoCD) to deploy *this PR's* build to an isolated,
> ephemeral namespace on PR open and tear it down on close (e.g. an ArgoCD
> `ApplicationSet` over a PR generator), plus the dispatch wiring that injects the
> resulting URL into a fresh, clean-context acceptance run. That provisioning +
> dispatch loop lives in the infra repo / the dispatch layer and is tracked
> outside this contract; the repo-resident half — the clean-context driver and the
> verdict contract — is what the `acceptance-agent` skill specifies and is reusable
> the moment a preview URL is provided.
