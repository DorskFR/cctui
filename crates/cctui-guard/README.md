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
  --always-allow cctui.example.com:8700
```

`--prompt`/`PROMPT_FILE` and `--rules`/`GUARD_RULES_FILE` may be given via env.
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
  hosts), default-deny. Omitting `[network]` writes a default-**allow** policy
  (backwards compatible).
- **`[gate]`** — an optional deterministic completion check: a shell command run
  (in `--gate-cwd`, default `/workspace`) before any numeric transition *out* of
  the step is allowed. A non-zero exit refuses the transition and returns the
  command's output, so a finalize-type transition requires machine-checkable
  proof (tests passed, artifact exists, CI green) instead of the agent's
  assertion of completion. Omitting `[gate]` leaves the transition trusted, as
  before. `Exit` bypasses the gate — bail-out must always work; the agent reports
  the blocked outcome via the result callback rather than finalizing.

Every numeric transition (and the `SessionStart`/compact hook) re-injects the
target step's **prose body verbatim** plus a compact-context directive, so a long
or compacted session re-anchors on the trusted next-step instructions rather than
its own drifting summary. The body is every non-`[...]` line beneath the heading.

Step `0` or an unknown current step means "no guard" (everything allowed). The
engine starts on the lowest-numbered step.

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

## Proxy policy output

On startup and on every transition, the engine writes `--policy-out`:

```json
{ "allowed_hosts": ["api.example.com:443", "github.example.com:443"], "default": "deny" }
```

A step with no `[network]` writes `{"allowed_hosts": [], "default": "allow"}`.
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
