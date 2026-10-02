# Harness parity

cctui drives three agent harnesses from one daemon: **claude-code** (a
supervised `claude` daemon reached over a control socket), **codex** (the
`codex app-server` JSON-RPC protocol) and **opencode** (`opencode serve` over
HTTP + SSE). They are meant to sit behind adapters of the same shape, so a
session-level capability is written once in adapter-neutral code and each
harness contributes only a small primitive.

This document records where that holds today and where it does not. It is a
map of the code, not a wish list; the ranked gaps at the end are what the shape
is still missing.

## How to read the matrix

- **yes** — works for that harness.
- **partial** — works, but with a caveat named in the notes.
- **no** — not wired up.
- **n.a.** — the harness has no such concept, so there is nothing to wire.

The **Shape** column is the important one:

- **neutral** — one implementation in adapter-neutral code; adapters supply a
  primitive at most.
- **per-harness** — each adapter carries its own implementation of the same
  idea.
- **claude-only** — implemented inside the claude-code adapter with no neutral
  seam, so the other two cannot reuse it even if they wanted to.

## Command surface

Every adapter implements `SessionDriver` (`adapter_runtime.rs`), and
`dispatch_command` is the single exhaustive `match` over `AdapterCommand`. A
method an adapter does not override answers `Unsupported`, which surfaces as a
failed `CommandResult`. That much of the shape is genuinely shared.

| Capability | claude-code | codex | opencode | Shape | Notes |
| --- | --- | --- | --- | --- | --- |
| `Spawn` | yes | yes | yes | neutral trait, per-harness body | Three unrelated launch bodies; only the trait and the dispatch are shared. |
| `Fork` | yes | yes | yes | per-harness | claude re-slices a transcript (`fork_slice`), codex calls `thread/fork`, opencode creates a child session. |
| `SendMessage` / `Reply` | yes | yes | yes | per-harness | |
| `Interrupt` | yes | yes | yes | per-harness | |
| `Kill` | yes | yes | yes | per-harness | opencode must signal the whole process group; `opencode serve` leads its own. |
| `Remove` | yes | yes | yes | per-harness | |
| `Rename` | yes | yes | no | per-harness | opencode returns `Unsupported`. |
| `SetModel` | partial | yes | no | per-harness | claude refuses in place and directs the caller to fork. |
| `Resume` | yes | yes | no | per-harness | opencode sessions do not outlive the daemon at all (see below). |
| `ResumeMarks` / `AckMarks` | yes | yes | no | per-harness | Transcript offset bookkeeping; opencode streams over SSE and keeps none. |
| `PermissionResponse` | yes | yes | yes | per-harness | |
| `Diagnose` | yes | yes | yes | per-harness | Report *shape* is shared (`cctui_proto::diagnose`); every field is filled per harness, and opencode answers `n.a.` for most of them. |
| `WatchPty` (live terminal) | yes | yes | yes | neutral channel, per-harness body | `AdapterCtx::pty_watch`, `adapters/pty_watch.rs` and `adapters/ring_view.rs` are neutral; codex and opencode supply only a traffic snapshot, claude streams its PTY. `AdapterFactory::pty_watch` declares participation. |

## Launch-time capabilities

| Capability | claude-code | codex | opencode | Shape | Notes |
| --- | --- | --- | --- | --- | --- |
| Permission modes | yes | yes | partial | neutral enum, per-harness mapping | `PermissionMode` is neutral; `codex_sandbox_approval()` maps it for codex, claude maps it onto settings + whip phrases, opencode only picks an *agent profile* (`agent_of`) and cannot express the full range. |
| Spawn modes (interactive / one-shot child) | yes | yes | yes | per-harness | opencode derives "one-shot" from `parent_local_id` and ends the session on idle-after-assistant. |
| Gateway / account binding | yes | yes | yes | **neutral** | `adapters/gateway_env.rs` + `ServerClient::gateway_env`, each adapter calling its own `resolve_launch`. Fail-closed everywhere: an account-bound session with empty gateway env refuses to launch. |
| Bootstrap uploads / images | yes | yes | yes | **neutral** | `adapters/uploads.rs::stage_bootstrap` for codex and opencode; claude has its own `stage_uploads` in `control/settings.rs` doing the same job — one duplication. |
| Plugin skills | yes | yes | yes | **neutral** | `plugins::resolve_session_skills`; claude consumes it as `plugin_dirs`, codex as `developerInstructions` + `shell_environment_policy`, opencode as `skills.paths`. A good example of the intended shape. |
| Child env scrub (secrets never reach the agent) | yes | yes | yes | **neutral** | `childenv::ScrubChildEnv` is applied at every process spawn. |
| MCP relay (`CctuiAgent`) declaration | yes | yes | yes | **neutral** | `adapters/agent_mcp.rs::AgentMcp` renders per-harness config (claude `mcp_config`, codex config overrides, opencode config block) from one capability. |
| MCP-readiness launch gate | yes | yes | yes | **neutral** | `preflight.rs` holds the first turn until the session's relay has answered `initialize`, with `mcpready.rs` behind it. claude-code arms the same wait through its `SessionStart` hook instead, because it holds turn 1 in-band; `CCTUI_MCP_READY_WAIT_SECS` is one knob for all three. |
| Usage-limit launch hold | yes | yes | yes | **neutral** | `launchgate.rs` decides, `preflight.rs` waits and reports the hold on the session card. claude-code holds before its control-socket dispatch; codex and opencode hold after the session registers and before turn 1 — the turn is what would eat the 429, and before `SessionStarted` they have no card for the wait to land on. |
| Shared-checkout (cwd neighbours) notice | yes | yes | yes | **neutral** | `neighbours.rs` renders it, `preamble.rs` packages it, and each adapter delivers it: claude folds it into `<session-context>`, codex into `developerInstructions` beside its skill catalog, opencode into the spawn prompt. |
| Spawn preamble / `<session-context>` | yes | partial | partial | neutral block, per-harness delivery | `preamble.rs` is the neutral carrier and every harness has a delivery primitive, but only the shared-checkout notice travels through it today. Name, model·effort, permission mode, env var *names*, attached files and the `CctuiAgent` paragraph are still assembled by claude's `build_session_context` alone. |
| Version gate (harness binary vs running daemon) | yes | yes | n.a. | per-harness | `version_gate.rs` and `codex_version_gate.rs` are separate implementations; opencode only warns on a pinned-version mismatch. |

## Observation and lifecycle

| Capability | claude-code | codex | opencode | Shape | Notes |
| --- | --- | --- | --- | --- | --- |
| Session roster / neighbour registry | yes | yes | yes | **neutral** | `neighbours::LiveDirs::observe` folds in every adapter's events from `supervisor.rs`. |
| Session start time | yes | yes | yes | per-harness stamp, neutral read | Adapters stamp `started_at_ms` (codex, opencode) or `created_at` (claude) into `SessionMeta::extra`; `neighbours::reported_start` reads either. |
| Start time across a daemon restart | yes | yes | yes | per-harness | claude re-reads it from the worker's own `state.json`; codex and opencode persist it in their durable registries (`codex/persist.rs`, `opencode/persist.rs`). |
| Durable session registry | yes | yes | yes | per-harness | claude's workers are separate processes discovered by roster; codex snapshots `SessionRegistry` to `codex-sessions.json`; opencode records its sessions to `opencode-sessions.json` and re-attaches them after a re-exec (an in-flight turn is lost). |
| Re-announce on reconnect | yes | yes | yes | neutral contract, per-harness body | `AdapterCtx::connected` fires per connection; each adapter re-announces its own live sessions. |
| Turn-end signal to clients | yes | yes | yes | **neutral** | `adapters/turn_end.rs`, gated on the server's `turn_end` capability: claude from its Stop hook, codex from `turn/completed`, opencode from `session.idle`. |
| Turn-end for subagent follow | yes | no | no | claude-only | `childwatch::note_turn_end` is called only from `claude_code/mod.rs`; codex and opencode children fall back to `childwatch`'s `observe` heuristics. |
| Subagent follow (`CctuiAgent`) | yes | yes | yes | **neutral** | `agenttool.rs` + `childwatch.rs`; adapters contribute only `parent_local_id` on `SessionStarted`. |
| Ask / plan / permission prompts | yes | yes | yes | neutral events, per-harness source | `AdapterEvent::Ask`/`Permission` are neutral; claude sources them from `askhook`, codex from app-server notifications, opencode from SSE. |
| Stop hooks / whip | yes | no | no | claude-only | `whipstop.rs` is a claude hook binary; nothing equivalent exists for the other two. |
| Diagnose rings + redaction | n.a. | yes | yes | **neutral** | `adapters/traffic_rings.rs`, transport-tagged per entry; codex records JSON-RPC and stderr, opencode records HTTP, SSE and stderr. |
| Harness sandbox health | n.a. | yes | n.a. | per-harness | codex probes bubblewrap at adapter start, reports it on the machine and fails `auto` spawns fast; the other harnesses have no sandbox of their own. |
| Transcript backfill | yes | yes | no | per-harness | |
| Live status / tempo classification | yes | yes | yes | neutral event, per-harness body | |

## Gaps and duplications worth fixing, ranked

1. **opencode has no durable session registry.** It is the only adapter whose
   sessions cannot survive a daemon self-update; `opencode serve` is killed on
   re-exec and the `LiveRegistry` is dropped. Everything downstream of that
   (resume, resume marks, start time across a restart, `Rename` on a cold
   session) is missing for one reason, not four. This is now the largest
   single gap.

2. **The spawn preamble carries only the shared-checkout notice.** The neutral
   seam exists — `preamble.rs` plus a delivery primitive per harness — but the
   rest of what claude's `build_session_context` assembles (name,
   model·effort, permission posture, env var names, staged attachments, the
   `CctuiAgent` paragraph) is still built inside the claude-code adapter.
   Moving that assembly onto the neutral block would let codex and opencode
   agents see what a claude agent already sees, for roughly the cost of moving
   one function.

3. **Upload staging is written twice.** `adapters/uploads.rs::stage_bootstrap`
   and `claude_code/control/settings.rs::stage_uploads` do the same job with
   the same contract. Claude should call the neutral one.

4. **Subagent follow knows the precise turn end only for claude.** Clients
   now get a turn-end signal from all three harnesses, but `childwatch`'s
   `note_turn_end` is still fed only by claude; codex and opencode children
   are followed by heuristics. Feeding it from `adapters/turn_end.rs` would
   close this.

5. **Two version gates, one idea.** `claude_code/version_gate.rs` and
   `codex/codex_version_gate.rs` are independent implementations of "the
   harness binary moved under a running session"; opencode has neither.

6. **Permission modes are not equally expressible.** opencode collapses the
   neutral `PermissionMode` onto an agent profile, so a mode the user picked
   can silently mean something coarser. Worth documenting on the session card
   rather than pretending the mapping is total.

7. **Diagnose is a shared report shape filled three ways.** The proto type is
   neutral and codex and opencode now share one ring buffer with redaction,
   but the rest of each report is still filled per harness.
