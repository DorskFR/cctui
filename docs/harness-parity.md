# Harness parity

cctui drives three agent harnesses from one daemon, plus a fourth adapter
that covers a whole family: **claude-code** (a supervised `claude` daemon
reached over a control socket), **codex** (the `codex app-server` JSON-RPC
protocol), **opencode** (`opencode serve` over HTTP + SSE) and **ACP** (any
Agent Client Protocol agent over stdio JSON-RPC: gemini-cli first, qwen-code
and goose to follow; one `AcpAdapter`, one row per agent, see
`docs/adr/0003-acp-adapter.md`). They are meant to sit behind adapters of the
same shape, so a session-level capability is written once in adapter-neutral
code and each harness contributes only a small primitive.

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
  seam, so the others cannot reuse it even if they wanted to.

The **ACP** column reads for every agent row at once: the adapter is shared,
so a cell differs per agent only where the note says so.

## Command surface

Every adapter implements `SessionDriver` (`adapter_runtime.rs`), and
`dispatch_command` is the single exhaustive `match` over `AdapterCommand`. A
method an adapter does not override answers `Unsupported`, which surfaces as a
failed `CommandResult`. That much of the shape is genuinely shared.

| Capability | claude-code | codex | opencode | ACP | Shape | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `Spawn` | yes | yes | yes | yes | neutral trait, per-harness body | Four unrelated launch bodies; only the trait and the dispatch are shared. ACP: one agent process per session, `initialize` → `session/new` → mode → first `session/prompt`. |
| `Fork` | yes | yes | yes | no | per-harness | claude re-slices a transcript (`fork_slice`), codex calls `thread/fork`, opencode creates a child session. ACP answers `Unsupported` until `session/fork` leaves the spec's unstable set. |
| `SendMessage` / `Reply` | yes | yes | yes | yes | per-harness | ACP: `session/prompt`, text plus image blocks when the agent advertises `promptCapabilities.image`; a reply during a turn is queued. |
| `Interrupt` | yes | yes | yes | yes | per-harness | ACP: `session/cancel`; the agent answers the prompt with `stopReason: cancelled`. |
| `Kill` | yes | yes | yes | yes | per-harness | opencode and ACP signal the whole process group; both children lead their own. ACP sends `session/close` first when the agent advertises it. |
| `Remove` | yes | yes | yes | yes | per-harness | ACP treats it as a kill. |
| `Rename` | yes | yes | no | no | per-harness | opencode and ACP return `Unsupported`; ACP reads the agent's own `session_info_update` title instead. |
| `SetModel` | partial | yes | no | no | per-harness | claude refuses in place and directs the caller to fork. ACP answers `Unsupported` until the model catalog ticket wires `set_config_option` / legacy `set_model`. |
| `Resume` | yes | yes | no | no | per-harness | opencode and ACP sessions do not outlive the daemon (see below). |
| `ResumeMarks` / `AckMarks` | yes | yes | no | no | per-harness | Transcript offset bookkeeping; opencode and ACP stream and keep none. |
| `PermissionResponse` | yes | yes | yes | yes | per-harness | ACP: `session/request_permission` answered with the `allow_once` / `reject_once` option; yolo answers itself. |
| `Diagnose` | yes | yes | yes | yes | per-harness | Report *shape* is shared (`cctui_proto::diagnose`); every field is filled per harness. opencode and ACP answer `n.a.` for the claude-specific facts and carry their own section (`opencode`, `acp`: agent name and version, raw `initialize`, modes, pending permissions, last priced cost). |
| `WatchPty` (live terminal) | yes | yes | yes | yes | neutral channel, per-harness body | `AdapterCtx::pty_watch`, `adapters/pty_watch.rs` and `adapters/ring_view.rs` are neutral; codex, opencode and ACP supply only a traffic snapshot, claude streams its PTY. `AdapterFactory::pty_watch` declares participation. |

## Launch-time capabilities

| Capability | claude-code | codex | opencode | ACP | Shape | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Permission modes | yes | yes | partial | partial | neutral enum, per-harness mapping | `PermissionMode` is neutral; `codex_sandbox_approval()` maps it for codex, claude maps it onto settings + whip phrases, opencode only picks an *agent profile* (`agent_of`) and cannot express the full range. ACP carries a table per agent row (`acp/modes.rs`: mode id, config option or launch flag) and **refuses at spawn** a mode the row cannot express instead of widening it; whip is refused everywhere today. |
| Spawn modes (interactive / one-shot child) | yes | yes | yes | partial | per-harness | opencode derives "one-shot" from `parent_local_id` and ends the session on idle-after-assistant. ACP sessions are interactive only until the `CctuiAgent` relay ticket. |
| Gateway / account binding | yes | yes | yes | partial | **neutral** | `adapters/gateway_env.rs` + `ServerClient::gateway_env`, each adapter calling its own `resolve_launch`. Fail-closed everywhere: an account-bound session with empty gateway env refuses to launch. ACP pulls the launch context but requires no gateway keys until the OpenAI-wire and Google family tickets land. |
| Bootstrap uploads / images | yes | yes | yes | yes | **neutral** | `adapters/uploads.rs` stages for every harness: `stage_bootstrap` at spawn, `stage_files` for mid-chat attachments (claude's `control/settings.rs` delegates to it). ACP sends images as prompt blocks when the agent advertises `promptCapabilities.image` and lists other files by path. |
| Plugin skills | yes | yes | yes | no | **neutral** | `plugins::resolve_session_skills`; claude consumes it as `plugin_dirs`, codex as `developerInstructions` + `shell_environment_policy`, opencode as `skills.paths`. ACP has no instruction channel for them yet. |
| Child env scrub (secrets never reach the agent) | yes | yes | yes | yes | **neutral** | `childenv::ScrubChildEnv` is applied at every process spawn. |
| MCP relay (`CctuiAgent`) declaration | yes | yes | yes | no | **neutral** | `adapters/agent_mcp.rs::AgentMcp` renders per-harness config (claude `mcp_config`, codex config overrides, opencode config block) from one capability. ACP sends `mcpServers: []` until its relay ticket. |
| MCP-readiness launch gate | yes | yes | yes | n.a. | **neutral** | `preflight.rs` holds the first turn until the session's relay has answered `initialize`, with `mcpready.rs` behind it. claude-code arms the same wait through its `SessionStart` hook instead, because it holds turn 1 in-band; `CCTUI_MCP_READY_WAIT_SECS` is one knob for all three. ACP declares no relay yet, so there is nothing to wait for. |
| Usage-limit launch hold | yes | yes | yes | yes | **neutral** | `launchgate.rs` decides, `preflight.rs` waits and reports the hold on the session card. claude-code holds before its control-socket dispatch; codex, opencode and ACP hold after the session registers and before turn 1 — the turn is what would eat the 429, and before `SessionStarted` they have no card for the wait to land on. |
| Shared-checkout (cwd neighbours) notice | yes | yes | yes | yes | **neutral** | `neighbours.rs` renders it, `preamble.rs` packages it, and each adapter delivers it: claude folds it into `<session-context>`, codex into `developerInstructions` beside its skill catalog, opencode and ACP into the first prompt. |
| Spawn preamble / `<session-context>` | yes | partial | partial | partial | neutral block, per-harness delivery | `preamble.rs` is the neutral carrier and every harness has a delivery primitive, but only the shared-checkout notice and attached context travel through it today. Name, model·effort, permission mode, env var *names*, attached files and the `CctuiAgent` paragraph are still assembled by claude's `build_session_context` alone. |
| Version gate (harness binary vs running daemon) | yes | yes | n.a. | no | per-harness | `version_gate.rs` and `codex_version_gate.rs` are separate implementations; opencode only warns on a pinned-version mismatch. ACP reports `agentInfo.version` per session and has no gate yet. |

## Observation and lifecycle

| Capability | claude-code | codex | opencode | ACP | Shape | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Session roster / neighbour registry | yes | yes | yes | yes | **neutral** | `neighbours::LiveDirs::observe` folds in every adapter's events from `supervisor.rs`. |
| Session start time | yes | yes | yes | yes | per-harness stamp, neutral read | Adapters stamp `started_at_ms` (codex, opencode, ACP) or `created_at` (claude) into `SessionMeta::extra`; `neighbours::reported_start` reads either. |
| Start time across a daemon restart | yes | yes | yes | no | per-harness | claude re-reads it from the worker's own `state.json`; codex and opencode persist it in their durable registries (`codex/persist.rs`, `opencode/persist.rs`). |
| Durable session registry | yes | yes | yes | no | per-harness | claude's workers are separate processes discovered by roster; codex snapshots `SessionRegistry` to `codex-sessions.json`; opencode records its sessions to `opencode-sessions.json` and re-attaches them after a re-exec (an in-flight turn is lost). ACP sessions die with the daemon until the durable-sessions ticket (`session/resume` or `load`). |
| Re-announce on reconnect | yes | yes | yes | yes | neutral contract, per-harness body | `AdapterCtx::connected` fires per connection; each adapter re-announces its own live sessions. |
| Turn-end signal to clients | yes | yes | yes | yes | **neutral** | `adapters/turn_end.rs`, gated on the server's `turn_end` capability: claude from its Stop hook, codex from `turn/completed`, opencode from `session.idle`, ACP from the `session/prompt` response (`stopReason`). |
| Turn-end for subagent follow | yes | yes | yes | yes | **neutral** | `childwatch::note_turn_end` is fed through `adapters/turn_end.rs` by every adapter; the precise end reaches a `CctuiAgent` follow whatever the harness. |
| Subagent follow (`CctuiAgent`) | yes | yes | yes | partial | **neutral** | `agenttool.rs` + `childwatch.rs`; adapters contribute only `parent_local_id` on `SessionStarted`. An ACP child can be followed, but cannot spawn children of its own until the relay ticket. |
| Ask / plan / permission prompts | yes | yes | yes | partial | neutral events, per-harness source | `AdapterEvent::Ask`/`Permission` are neutral; claude sources them from `askhook`, codex from app-server notifications, opencode from SSE, ACP from `session/request_permission`. ACP `elicitation/*` → `AskQuestion` is a follow-up. |
| Stop hooks / whip | yes | no | no | no | claude-only | `whipstop.rs` is a claude hook binary; nothing equivalent exists for the others, which is why whip is refused for ACP rows. |
| Diagnose rings + redaction | n.a. | yes | yes | yes | **neutral** | `adapters/traffic_rings.rs`, transport-tagged per entry; codex records JSON-RPC and stderr, opencode records HTTP, SSE and stderr, ACP taps its stdio JSON-RPC and stderr. |
| Harness sandbox health | n.a. | yes | n.a. | n.a. | per-harness | codex probes bubblewrap at adapter start, reports it on the machine and fails `auto` spawns fast; the other harnesses have no sandbox of their own. |
| Transcript backfill | yes | yes | no | no | per-harness | |
| Live status / tempo classification | yes | yes | yes | yes | neutral event, per-harness body | ACP: `active` while a prompt is in flight, `idle` on its answer, `current_mode_update` as a posture change. |

## Gaps and duplications worth fixing, ranked

1. **ACP has no durable session registry.** opencode now records its
   sessions and re-attaches them after a re-exec; ACP is the adapter whose
   sessions cannot survive a daemon self-update. The protocol has
   `session/resume` and `session/load` for it, so this is the epic's
   durable-sessions ticket rather than a design gap.

2. **The spawn preamble carries only the shared-checkout notice.** The neutral
   seam exists — `preamble.rs` plus a delivery primitive per harness — but the
   rest of what claude's `build_session_context` assembles (name,
   model·effort, permission posture, env var names, staged attachments, the
   `CctuiAgent` paragraph) is still built inside the claude-code adapter.
   Moving that assembly onto the neutral block would let codex and opencode
   agents see what a claude agent already sees, for roughly the cost of moving
   one function.

3. **Two version gates, one idea.** `claude_code/version_gate.rs` and
   `codex/codex_version_gate.rs` are independent implementations of "the
   harness binary moved under a running session"; opencode and ACP have
   neither.

4. **Permission modes are not equally expressible.** opencode collapses the
   neutral `PermissionMode` onto an agent profile, so a mode the user picked
   can silently mean something coarser. ACP takes the other stance and
   refuses what its row cannot express; the two should converge, and the
   session card should show the agent-side mode either way.

5. **Diagnose is a shared report shape filled four ways.** The proto type is
   neutral and codex, opencode and ACP share one ring buffer with redaction,
   but the rest of each report is still filled per harness.

6. **ACP agents have no plugin-skill or `CctuiAgent` channel yet.** Both ride
   `session/new`'s `mcpServers` once the relay ticket lands; until then an
   ACP session sees neither.
