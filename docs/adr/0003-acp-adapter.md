# ADR 0003 — One ACP adapter for every harness without a native one

- **Status:** Accepted
- **Date:** 2026-10-08
- **Deciders:** cctui maintainers
- **Builds on:** the adapter contract (`crates/cctui-daemon/src/adapter_runtime.rs`),
  the harness table (`crates/cctui-proto/src/adapter.rs`), the neutral launch
  preamble and preflight, the traffic rings and ring viewer, and the harness
  parity map (`docs/harness-parity.md`)

## Context

cctui drives three harnesses through three bespoke adapters. Every further
agent (gemini-cli, qwen-code, goose, kilo, Mistral Vibe, Copilot CLI, Cursor,
Kimi, …) would mean a fourth, fifth, sixth adapter of the same shape: spawn a
process, speak its protocol, translate its stream into `AdapterEvent`s. Most
of those agents now speak the Agent Client Protocol (ACP) over stdio, and the
spike re-verified against the protocol as of 2026-09-30 found the v1 wire
stable enough to build on: `session/new|prompt|cancel|set_mode|close`,
`session/update` with message, thought, tool call, plan, mode and usage
updates, and `session/request_permission`.

What differs from the spike:

- The Rust SDK is `agent-client-protocol` **2.2.0** (schema 1.9.1), a builder
  API with `Send` futures. Two majors shipped in three months, so it is pinned
  exactly.
- `session/set_model` is gone from the spec in favour of
  `session/set_config_option`, but gemini, auggie, kimi and kiro still speak
  only the legacy `models` / `set_model` pair. The SDK's typed
  `NewSessionResponse` has no `models` field at all.
- Agents only call `session/request_permission` when *their own* policy
  says so. opencode ran a shell command without asking.

## Decision

One `AcpAdapter` type, one factory per **agent row**
(`crates/cctui-daemon/src/adapters/acp/rows.rs`). A row is the launch command
(`gemini --acp`), a permission-mode table, an update command and whether the
agent speaks the legacy model pair. A row becomes a running factory only
when the harness table in `cctui-proto` lists its id, with
`default_enabled = false`; everything in the daemon keys on the adapter id,
so gateway family, spawn capabilities, usage accounting and the webui picker
need no ACP-specific branch.

Boundaries:

- **ACP drives daemon-spawned sessions of harnesses cctui has no native
  adapter for.** claude-code, codex and opencode stay native. ACP has no view
  of sessions started outside cctui, so `ResumeMarks` / `AckMarks` answer
  `Unsupported`, and there is no attach or transcript-file read.
- **All SDK contact lives in `connection.rs`.** Requests go out untyped
  (`UntypedMessage`, built by `protocol.rs`) and notifications and requests
  come in untyped, so the adapter reads a legacy `models` block or a
  `_meta.quota` token count that a typed schema would drop. The SDK owns
  framing, request ids, cancellation and the `method_not_found` answer for
  the `fs/*` and `terminal/*` requests we advertise as unsupported.
- **The agent is spawned by the daemon, not by the SDK's `AcpAgent`**, which
  cannot set a cwd. `tokio::process` with the child in its own process group,
  `ScrubChildEnv`, stderr drained into a traffic ring, and stdio tapped so the
  JSON-RPC frames land in the same ring the diagnose report and the live view
  read.
- **Nothing waits inside a receive callback.** A permission request is
  forwarded on an unbounded inbox with a oneshot the session answers later;
  the responder is parked on `cx.spawn`. A `session/prompt` only returns at
  turn end, so it runs on its own task and the session loop stays free for
  interrupts, permission answers and kills.
- **Normalize in the daemon, in both dialects.** `normalize.rs` turns the
  wire JSON of one `session/update` into the canonical `type`/`content` shape
  and the claude-daemon `role`/`text` shape at once, as the opencode adapter
  does, so the server passes it through and needs no `acp` arm. Message and
  thought chunks are coalesced by `messageId`; a tool call and its completion
  become `tool_call` + `tool_result`; a plan becomes the `update_plan` tool
  call every client already renders; `stopReason` is the turn end, fed to
  `adapters/turn_end.rs` and `childwatch::note_turn_end`.
- **Permission modes are applied at launch and refused when inexpressible.**
  Each row maps cctui's four modes onto the agent's own mode id, a config
  option or a launch flag. A mode the row cannot express fails the spawn with
  a clear error; it is never widened silently. Whip has no agent-side
  equivalent anywhere yet. Yolo auto-selects `allow_once` on every
  `session/request_permission`; Ask and Auto surface it as a
  `PermissionRequest` answered from the UI.
- **Not logged in is a spawn failure, not a crash.** The `-32000` an agent
  answers `session/new` with becomes `SpawnFailed` worded as
  "run `<agent>` once on <machine> to log in".

## Consequences

- Adding an agent is one row here plus one row in the harness table. The
  conformance test (`acp_answers_every_command`) and the fake-agent
  end-to-end scenarios cover every row the same way.
- The fake agent (`crates/cctui-daemon/tests/acp_fake_agent.rs`) is a
  custom-harness test binary that is also the agent, so no fixture binary
  ships. The real-agent probe against `opencode acp` is `#[ignore]`-style:
  `cargo test -p cctui-daemon --test acp_fake_agent -- --ignored`.
- Per-turn token usage comes from `PromptResponse.usage` (still unstable in
  the spec and feature-gated in the SDK), else gemini's `_meta.quota`, else
  the cumulative `usage_update`. Cost is read when the agent prices a turn
  and shown in the diagnose report; the server's cost accounting does not
  yet read it.
- Fork, rename, durable sessions across a daemon re-exec, the `CctuiAgent`
  relay, rich permission answers, model catalogs for the picker and the
  gateway family for OpenAI-wire agents are follow-up tickets of the epic
  and answer `Unsupported` until then.
- Protocol v2 (which drops `fs/*`, `terminal/*`, `session/load` and
  `set_mode`) is not targeted; the adapter sends `protocolVersion: 1` and
  logs when an agent answers otherwise.
