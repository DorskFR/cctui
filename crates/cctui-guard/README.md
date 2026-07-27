# cctui-guard

A markdown-driven workflow guard for Claude Code worker sessions. It parses a
dispatched prompt into numbered **steps**, each declaring which tools and network
destinations are permitted, and serves a localhost HTTP API that Claude Code's
`PreToolUse` hook calls before every tool invocation. Step transitions are a DAG
enforced server-side; each transition rewrites the egress policy consumed by the
guard proxy.

This crate is the Rust port of the original Python `workflow-guard/daemon.py`
and is the **canonical specification** of the prompt-step and guard-rules format.

## Running

```
cctui-guard \
  --prompt <prompt.md> \
  --rules /etc/claude-worker/guard-rules.md \
  --listen 127.0.0.1:9999 \
  --state /var/run/workflow-guard/state \
  --policy-out /var/run/guard-proxy/policy.json \
  --always-allow callback.example.com:443 \
  --always-allow cctui.example.com:8700 \
  --judge-cmd 'accept-judge'
```

`--prompt` accepts either frontend: prompt **markdown** (the default authoring
format), or a machine-authored **`workflow.json`** matching the published IR
schema (detected by a `.json` extension). `--emit-schema` prints that JSON
Schema and exits.

`--prompt`/`PROMPT_FILE`, `--rules`/`GUARD_RULES_FILE`, and
`--judge-cmd`/`GUARD_JUDGE_CMD` may be given via env. `--judge-cmd` is the
command the `[llmjudge]` acceptance judge runs through (see below); leaving it
unset while a prompt declares `[llmjudge]` refuses that step's transition
(fail closed).
`--always-allow host:port` is repeatable; those hosts are appended to every
deny-default policy written (the entrypoint seeds the result-callback and
in-cluster service hosts this way — nothing is hardcoded). The daemon must run
as a uid the worker cannot write as, so the worker cannot tamper with the state
file or the proxy policy.

## HTTP API

| Method | Path          | Purpose                                                            |
| ------ | ------------- | ------------------------------------------------------------------ |
| POST   | `/check`      | `PreToolUse` hook payload → allow/deny decision.                   |
| POST   | `/transition` | Request a step transition. Body: `{"step": N}` or `{"step":"exit"}`. |
| GET    | `/state`      | Current step number, title, and allowed/disallowed strings.        |
| POST   | `/state`      | `SessionStart`/compact hook — returns context text for re-injection. |
| GET    | `/health`     | `{"status":"ok"}`.                                                 |

`/check` request:

```json
{ "tool_name": "Bash", "tool_input": { "command": "git push origin main" } }
```

`/check` response (Claude Code hook shape):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "[Step 2] 'git push' is disallowed in this step"
  }
}
```

## Prompt step format

Steps are headings matching `#`–`######` followed by `Step N` (case-insensitive),
optionally `: Title`. Each step collects the bracketed rule lines beneath it,
until the next step heading:

```markdown
# Step 1: Research the task

Explore; do not modify anything.

[allowed]: all-read
[disallowed]: *
[network]: net-claude, net-github
[transition]: 2, Exit

# Step 2: Implement

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-claude
[transition]: Exit
```

- **`[allowed]` / `[disallowed]`** — comma-separated keywords and/or tool-set
  names (expanded from the rules file). `*` is a wildcard. Empty = unrestricted.
- **`[transition]`** — the valid next steps. A comma-separated list of step
  numbers and/or `Exit`. Numbers are extracted by digit-run, so `Step 9, Step 11`
  and `9, 11` are equivalent. Transitioning to a step not listed is rejected.
  `Exit` is terminal — and is **always** allowed from any step (bail-out must
  always work), regardless of whether it appears in `[transition]`.
- **`[network]`** — comma-separated network-set names; on entry to the step the
  proxy policy is rewritten to allow exactly those hosts (plus the always-allow
  hosts), default-deny. `[network]: *` opens egress fully (default-**allow**).
  Omitting `[network]` on a guarded step is default-**deny** (only the always-allow
  hosts reachable); a document-level `[network-default]: allow` header above the
  first step restores the legacy open behavior for every step that omits
  `[network]`. A prompt with no steps is unguarded and stays default-allow.
- **`[gate]`** — an optional deterministic completion check: a shell command run
  (in `--gate-cwd`, default `/workspace`) before any numeric transition *out* of
  the step is allowed. A non-zero exit refuses the transition and returns the
  command's output, so a finalize-type transition requires machine-checkable
  proof (tests passed, artifact exists, CI green) instead of the agent's
  assertion of completion. Omitting `[gate]` leaves the transition trusted, as
  before. `Exit` bypasses the gate — bail-out must always work; the agent reports
  the blocked outcome via the result callback rather than finalizing.
- **`[llmjudge]`** — an optional semantic acceptance gate (CCT-516), parallel to
  `[gate]` and enforced independently *after* it. The bare annotation is
  immediately followed by one `- <question>` line per binary acceptance question
  (optionally `- <question> :: <violation example>`), max 12 per step:

  ```markdown
  [llmjudge]
  - Does every acceptance condition have a matching evidence[] entry? :: two of three covered
  - Does the diff implement the change itself, not just a test?
  ```

  On a numeric transition out of the step the guard pipes the question block to
  the configured `--judge-cmd` (env `GUARD_JUDGE_CMD`), run via `sh -c` in
  `--gate-cwd` with a **clean context** — the judge sees the questions plus its
  own working tree (the Intent+Acceptance artifact, the assembled `evidence[]`,
  the diff), never the implementer session's reasoning. The command must print a
  JSON array on stdout, one verdict per question in order:

  ```json
  [{ "question": 1, "answer": 1, "reason": "one line" }, ...]
  ```

  The transition proceeds **only on a perfect score** (every answer `1`). A
  partial score, a malformed verdict, a failing command, or a missing
  `--judge-cmd` all refuse the transition (fail closed), returning the failing
  questions + reasons in `error` (same shape as a gate failure). Either way the
  per-question verdicts are attached to the response as a `kind: "judge"`
  evidence entry (`{"kind","summary","detail","verdicts"}`) for the agent to
  carry into the result callback's `evidence[]` — e.g. "llm judge: 5/6
  acceptance questions verified". A malformed block (inline value, no questions,
  an empty question, a duplicate block, more than 12 questions) is a **parse
  error** at startup. `Exit` bypasses the judge like it bypasses the gate.

### `guard` fenced block (per-transition gates, max-visits)

Structure that does not fit a single bracket line lives in an opt-in
```` ```guard ```` fenced block (info string `guard` or `guard yaml`) inside the
step. Its content is a restricted YAML subset; anything else in the step stays
prose. It compiles into the same IR as the bracket lines.

````markdown
# Step 3: Implement
[gate]: make build
[transition]: 4, 6, Exit
```guard
max-visits: 3
transitions: [{to: 4, gate: "make test"}, {to: 6}]
```
````

- **`transitions:`** — a per-target list, either flow style
  (`[{to: 4, gate: "make test"}, {to: 6}]`) or a block list of `- to: N` items
  with an optional `gate:` continuation. Each `to` **unions** into the step's
  `[transition]` targets (author them in either place). A `gate:` is a
  **per-transition** deterministic gate: it runs **only** when advancing to that
  specific target, **after** the step-level `[gate]` — both must pass (step gate
  first, then the transition gate). Quote a gate command that contains a comma.
- **`max-visits: N`** — the step may be *entered* at most `N` times; a transition
  that would exceed it is denied with a message telling the agent to exit and
  report rather than retry, breaking a two-step ping-pong loop. The initial entry
  counts as visit 1. `Exit` is never blocked by the bound. Visit counts persist
  in the state file (`{"step": N, "visits": {…}}`); a legacy `{"step": N}` file
  reads as zero visits.

Every numeric transition (and the `SessionStart`/compact hook) re-injects the
target step's **prose body verbatim**, so a long or compacted session re-anchors
on the trusted next-step instructions rather than its own drifting summary. The
body is every non-`[...]` line beneath the heading.

A compact-context directive is appended **only** for steps that opt in with a
`[compact]` line (bare ⇒ on; `[compact]: false`/`no`/`off`/`0` ⇒ off). It is off
by default because compaction is lossy and counter-productive on large-context
models — re-injection re-anchors context without discarding it unless a step
explicitly asks to trim.

Step `0` or an unknown current step means "no guard" (everything allowed). The
engine starts on the lowest-numbered step.

## Compiled IR + JSON schema (CCT-619)

Markdown is the authoring frontend; both frontends **compile** into one canonical
typed model, the IR, defined by the serde structs in `src/ir.rs`:

```
Workflow { version, network_default?, rules: [path], sets: [SetDefinition],
           steps: [WorkflowStep] }
SetDefinition { name, members: [..], extend }
WorkflowStep { id, title, body, allowed, disallowed, network,
               transition, gate?, compact, judge[], max_visits? }
```

The enums are the spec: `allowed`/`disallowed` are a `Rule`
(`unrestricted` | `wildcard` | `{ list: [..] }`), `transition` is
`{ to: [N..], exit: bool, gates: {N: "cmd"} }` (per-target gates). This replaces
spec-by-implementation — the
**published JSON Schema** (`schema/workflow.v1.json`, regenerate with
`cargo run -p cctui-guard -- --emit-schema`) is the versioned contract, and a
lint (separate ticket) validates the IR, covering both frontends for free.

- **Versioning** — a `[guard]: vN` header line above the first step (or the
  `version` field of a `workflow.json`) selects the IR version; missing ⇒ `v1`.
  An unsupported version fails loudly at startup.
- **`workflow.json` frontend** — machine writers can skip markdown and hand the
  daemon a `workflow.json` that deserializes straight into `Workflow`.
- **No behavior change** — the IR lowers back into the same internal step model
  the engine enforces, so its allow/deny decisions are identical to the markdown
  path. `tests/ir_parity.rs` builds one engine from the markdown parser and one
  from the compiled IR and asserts they agree on every check, transition, and
  egress policy (the security-boundary parity guarantee).

## Rule evaluation

- **Bash** commands are split on `&&`, `||`, `;`, and `|` (quotes respected);
  **every** segment must pass. A segment is matched as the text `Bash <segment>`
  after normalizing away git's global flags (`git -C <path> fetch` → `git fetch`,
  `git -c k=v --no-pager log` → `git log`) so allowlist phrases like `git fetch`
  match however the working dir/config is passed.
- **Built-in tools** (`Read`, `Edit`, `Write`, `Grep`, …) are matched by tool
  name. These built-in names are *stripped* from the allow/disallow lists when
  evaluating Bash, so the bare keyword `Write` can't substring-collide with shell
  text like "URL rewrite" (and `Edit` can't match "edited").
- **MCP tools** (`mcp__*`) match against `mcp <tool-name> <json-input>`.
- Matching is case-insensitive substring matching. Deny is checked first
  (deny-first); a specific `[allowed]` keyword overrides a `[disallowed]: *`.
- `ToolSearch` and `TodoWrite` are always allowed; so is any Bash command
  targeting the guard daemon itself (`127.0.0.1:9999` / `localhost:9999`).

## Guard-rules format

The shared rules file (`--rules`) defines reusable **tool sets** and **network
sets**. Each definition is `[name]: member, member, …`; blank lines and `#`
comments are ignored. `name` matches `[a-zA-Z0-9_-]+`. Sets may reference other
sets and are expanded recursively (circular references are broken safely).

```markdown
# Tool sets
[code-read]: Read, Grep, Glob, LSP, WebFetch, WebSearch
[code-write]: Edit, Write
[git-read]: git log, git diff, git status, git fetch
[git-write]: git checkout, git commit, git push
[github-read]: gh pr list, gh pr view, gh api
[github-write]: gh pr create, gh pr edit, git push

# Composites
[all-read]: code-read, git-read, github-read
[all-write]: code-write, git-write, github-write
[remote-write]: git push, github-write
[review-only]: code-read, git-read, github-read, circleci-read, slack-read

# Network sets (host:port, used by the proxy policy). Use host:* for all ports.
[net-claude]: api.example.com:443, downloads.example.com:443
[net-github]: github.example.com:443, github.example.com:22, api.github.example.com:443
```

> Note: `Bash` is deliberately excluded from `code-read`/`code-write`. Git/GitHub
> keywords match Bash commands by substring, so read-only steps that omit `Bash`
> only permit Bash commands that match a specific keyword. Add `Bash` explicitly
> to `[allowed]` when a step needs arbitrary shell (e.g. `npm test`, `tsc`).

### Inline sets + `[rules]` imports (prompt owns its control surface)

A set need not live in a shared `--rules` file: it can be **defined in the prompt
itself**, in the document prelude (above the first heading, alongside `[guard]` /
`[network-default]`), using the same `[name]: a, b` / `[name]+: a, b` syntax. A
prompt can also **import** shared rules files explicitly with a `[rules]: <path>`
directive (repeatable), resolved relative to the prompt file — the dependency is
visible in the file instead of injected out-of-band by `--rules`.

```markdown
[rules]: ./net-common.md
[net-yt]: yt.example.com:443
[code-read]+: mcp__yt

# Step 1: Triage
[allowed]: code-read
[network]: net-yt, net-callback
[transition]: Exit
```

Sets are layered lowest-to-highest precedence: `--rules-base` < `--rules` <
each `[rules]` import in authored order < inline prompt definitions. A later
layer's `[name]:` replaces a set and `[name]+:` extends it, so the prompt author's
inline definitions always win — the prompt is a self-contained artifact to
review, hash, and edit. `--rules` is retained for backward compatibility. An
unreadable `[rules]` import is a lint error and refuses startup (fail closed);
`lint --explain` prints each set's effective source (`inline`, `[rules] <path>`,
`--rules`, `--rules-base`).

## Proxy policy output

On startup and on every transition, the engine writes `--policy-out`:

```json
{ "allowed_hosts": ["api.example.com:443", "github.example.com:443"], "default": "deny" }
```

A guarded step with no `[network]` writes `{"allowed_hosts": [<always-allow>],
"default": "deny"}` unless a document `[network-default]: allow` header opts back
into `{"allowed_hosts": [], "default": "allow"}`. `[network]: *` writes
`default: "allow"`; a prompt with no steps stays `default: "allow"`.
On `Exit`, the policy is narrowed to the `net-claude` set plus the always-allow
hosts (so Claude can finish its response and the result callback can fire),
default-deny. If the proxy policy directory does not exist, policy writes are
skipped.

## Tests

```
cargo test -p cctui-guard
```

`tests/daemon.rs` ports the reference Python `test_daemon.py` (parser, splitter,
rule evaluation, tool-set expansion, engine allow/deny patterns).
`tests/integration.rs` drives the live axum server through a full
allow → transition → deny scenario with a neutralized guard-rules fixture
(no homelab hostnames; `example.com` throughout).
