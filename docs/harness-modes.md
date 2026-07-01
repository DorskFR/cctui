# Claude-code harness modes: `bg`, `oneshot`, `sdk`

The `claude-code` adapter can run `claude` in one of three **modes**. A mode is
an internal driver choice *inside* the `claude-code` adapter — it does **not**
change the `adapter_id`, so switching modes never re-keys or orphans existing
sessions. The server sends the same mode-agnostic
`AdapterCommand`/`AdapterEvent` verbs (Spawn / Reply / Kill / Fork / Interrupt /
SetModel / …) regardless of mode; only the daemon-side driver differs.

The active mode is a **per-user** setting, `user_settings.data.harnessMode` ∈
{`bg`, `sdk`, `oneshot`} (default `bg`, CCT-495). On change it is bridged into
each owned daemon's `DaemonAdapterConfig.config["mode"]` and pushed as a fresh
Reconcile, so every connected daemon picks up the new mode live (CCT-494). A
per-machine `adapters_enabled.config` may still override for machine pinning.

Driver selection lives in `crates/cctui-daemon/src/adapters/claude_code/mode.rs`
(`Mode::from_config`), which maps the string to `Mode::Bg` /
`Mode::Oneshot` / `Mode::Sdk` (plus the legacy UDS listener kept for CCT-87).

---

## 1. What each mode is

### `bg` (default) — `claude daemon` + control-socket PTY worker

Driver: `control.rs` (`control::Driver`). The daemon talks to a long-lived
`claude daemon` over its control socket and spawns/observes background (`--bg`)
PTY workers. Status/tempo/detail come from polling the daemon's `list` plus
reading `~/.claude/jobs/<short>/state.json`; message/tool/token events come from
**tailing the on-disk JSONL transcript** and normalizing each line through
`transcript::parse_line`.

- **Observability / transcript story:** rich, but **disk-coupled**. It depends
  on `state.json` (status, `resumeSessionId`, cwd) and the JSONL transcript
  file being present and tailed. This is the historical strength that made
  CCT-173 pick `--bg` over `-p` (a `-p` run never registered in
  `state.json`, so no transcript surfaced).
- Multi-turn is native: the worker idles awaiting input between turns; no
  respawn per reply.

### `oneshot` — `claude -p … --resume` per turn (CCT-499)

Driver: `oneshot.rs` (`OneshotDriver`). Each **turn** is a fresh, transient
`claude --print --output-format stream-json --verbose` child:

- **Spawn:** `claude -p <prompt> … --session-id <pre-minted uuid>` in the
  session cwd. The pre-minted id flows from `Spawn.session_id` exactly as `bg`
  uses it so the gateway-token binding stays intact (CCT-446/CCT-460).
- **Reply:** re-invoke `claude -p <text> --resume <session_id>` — a **new child
  per turn**, keyed on the same session id.
- On the terminal `result` frame it emits an idle `Status` (not
  `SessionEnded`), so the conversation stays resumable, mirroring how `--bg`
  idles awaiting input. A failed turn surfaces `SessionEnded{Crashed}`.
- **Observability / transcript story:** events are read **directly** from the
  child's stdout stream-json frames via `streamjson::parse_stream_line` — **no
  `state.json` and no transcript-tail dependency**. Resume still relies on
  claude's own on-disk conversation store (`--resume <id>`).

### `sdk` — persistent stream-json child (CCT-500)

Driver: `headless.rs` (`SdkDriver`). The daemon owns **one long-lived** `claude
--print --input-format stream-json --output-format stream-json --verbose` child
**per session**, driven the way the Claude Agent SDK's streaming-input mode is
(direct wire, no TS/Python SDK sidecar):

- **Spawn:** launch the persistent child with the pre-minted `--session-id` and
  send the first user turn on stdin.
- **Reply / SendMessage:** write a `{"type":"user",…}` envelope to the child's
  stdin — **no respawn**. The `result` frame is a *turn boundary*, not process
  exit; the child stays alive awaiting the next stdin turn.
- **Interrupt:** `control_request{subtype:"interrupt"}` on stdin (child stays
  alive).
- **SetModel:** in-place via `control_request{subtype:"set_model"}` when a model
  is given; effort-only changes have no control lever and fall back to "fork to
  change model".
- **Crash recovery:** on-demand **cold-resume** — a dead child (crash / daemon
  restart) is relaunched with fresh fail-closed gateway env on the next
  Reply/Resume (`--resume <id>`), not eagerly restarted by a ticker (eager
  restart risks a 401 relaunch loop).
- **Observability / transcript story:** structured events straight off stdout
  via the shared codec — **no `state.json`, no transcript tail**. Resume uses
  claude's on-disk conversation (`--resume`).

Both headless drivers reuse the same ask/permission **hook listener** the `bg`
driver uses (`super::run_hook_listener`): headless runs fire
`PreToolUse`/`AskUserQuestion` hooks via an injected `--settings` file, and the
`PermissionResponse` decision is carried by the hook's bidirectional long-poll.
Ask/Plan forms don't render headless, so they surface as the existing live
cards.

---

## 2. Fidelity matrix

| Capability | `bg` | `oneshot` | `sdk` |
|---|---|---|---|
| Multi-turn without respawn | Yes (idle PTY worker) | **No** — new `claude -p` child per turn (`--resume`) | Yes (persistent child; `result` = turn boundary) |
| Native `TokenUsage` events | Yes (transcript tail → `transcript::parse_line`) | Yes (assistant frames → shared codec) | Yes (assistant frames → shared codec) |
| Native `SessionModel` events | Yes (transcript) | Yes (`system`/init + per-assistant model) | Yes (`system`/init + per-assistant model) |
| Permissions / Ask / Plan | `--settings` hook + PTY keystroke fallback | `--settings` hook only (no PTY fallback) | `--settings` hook only (no PTY fallback) |
| Interrupt | control socket / worker | terminate the in-flight `-p` child (== Kill) | `control_request{interrupt}` (child stays alive) |
| Fork | Yes | `--resume <parent> --fork-session --session-id <child>` | `--resume <parent> --fork-session --session-id <child>` |
| Resume | Yes (from `state.json` / `--resume`) | Yes (`--resume`, no-op empty turn) | Yes (cold-launch `--resume`, no user turn) |
| SetModel in place | No — fork to change model | No — fork to change model | **Yes** (model only; effort-only → fork) |
| Crash recovery | daemon supervises worker | none needed (child is per-turn) | on-demand cold-resume w/ fresh gateway env |
| `state.json` dependency | **Yes** (status + resume + cwd) | **No** | **No** |
| Transcript-file dependency | **Yes** (event source) | No (events from stdout) | No (events from stdout) |

---

## 3. Metering / cost findings

### Why the gateway is what matters

cctui does **not** run workers against Anthropic directly. Every mode launches
`claude` with `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` pointing at cctui's
**own gateway** (OAuth-account passthrough, minted per session via
`resolve_launch_env` → `server.gateway_env`, fail-closed per CCT-460). So how a
run is metered/billed depends on **how the gateway and the bound account treat
the traffic**, not on any consumer-plan page or on whether the CLI was invoked
as `-p` vs `--bg`. The transport shape (PTY worker vs `-p` vs persistent
stream-json) does not by itself change which credit pool is drawn — that is a
property of the account/gateway, and must be confirmed first-hand.

### Historical context (why `bg` was chosen)

CCT-173 moved the worker from `claude -p` to `claude --bg` for two reasons:
1. **Observability** — `-p` never registered in `state.json`, so no transcript
   surfaced. Stream-json output now gives the daemon structured events directly,
   removing this reason for the headless modes.
2. **Billing** — `-p` "billed as API" while `--bg` "rode the subscription." The
   Anthropic headless/SDK metered-billing split announced **2026-06-15 was
   paused**, so reason (2) is currently **moot**.

> **Re-verify if the billing split un-pauses.** If Anthropic re-enables the
> headless/SDK metered-billing split, the `-p`/stream-json paths (`oneshot`,
> `sdk`) could once again be metered differently from the interactive `--bg`
> path even through the gateway. Re-run the measurement below before trusting
> the "no difference" conclusion.

### Measurement method (to run)

The empirical live-metering measurement has **not yet been run**. Do not quote
numbers until it is. Method:

1. Pick a dedicated **test** OAuth account bound through the gateway.
2. Run an **identical** short task (same prompt, same cwd, same model/effort)
   three times, once under each `harnessMode` (`bg`, `oneshot`, `sdk`), each
   against that test account through the gateway.
3. For each run capture the gateway/account signal: gateway request logs, the
   account usage-window delta (the same windows CCT-444 surfaces), and any
   provider-side usage counter — before vs after.
4. Compare: confirm `oneshot`/`sdk` do **not** silently draw from a different
   pool than `bg` today, and record the per-mode delta.

### Results (to be measured)

| Mode | Metered pool observed | Usage-window delta | Notes |
|---|---|---|---|
| `bg` | _to be measured_ | _to be measured_ | baseline |
| `oneshot` | _to be measured_ | _to be measured_ | |
| `sdk` | _to be measured_ | _to be measured_ | |

---

## 4. TokenUsage / SessionModel parity (from the code)

The soft-limit (CCT-431) and usage windows (CCT-444) depend on each mode
emitting `AdapterEvent::TokenUsage` and `AdapterEvent::SessionModel`. All three
modes converge on the **same normalizer**, so parity holds:

- **`bg`** — tails the on-disk JSONL transcript and runs each line through
  `transcript::parse_line`. That function emits `TokenUsage` from an assistant
  message's `usage` block (`input_tokens`, `output_tokens`,
  `cache_read_input_tokens`, `cache_creation_input_tokens`) — one row per
  message, skipped when all four are zero — and `SessionModel` from the
  message's `model`.
- **`oneshot`** — each `-p` child's stdout goes through
  `streamjson::parse_stream_line`. `assistant`/`user` frames carry the same
  `message` shape as transcript lines and are handed to the **same**
  `transcript::parse_line`, so `TokenUsage` maps identically; `SessionModel`
  comes both from the `system`/init frame's `model` and per-assistant model.
- **`sdk`** — identical: the persistent child's stdout runs through the same
  `streamjson::parse_stream_line` → `transcript::parse_line`, plus the init
  frame's `SessionModel`.

Verdict: **all three modes emit `TokenUsage` and `SessionModel`**, so the
soft-limit and usage windows keep working across modes.

### Follow-ups (flagged, not fixed here)

- **`result`-frame usage is not parsed.** `streamjson`'s handling of the
  `result` frame reads only `subtype`/`is_error` for the end reason; it ignores
  the cumulative `usage` / `total_cost_usd` the CLI reports there. This is
  consistent with `bg` (per-assistant-message accounting, no double count), but
  if a future assistant frame ever omits `usage` while the `result` frame
  carries the only totals, headless modes would under-count. Worth a reconcile
  check once live metering is measured.
- **Stale `#![allow(dead_code)]` note in `streamjson.rs`.** The module header
  still says the codec is "not yet wired into a live driver," but `oneshot` and
  `sdk` now consume it. Cosmetic — update the comment.
- **`stream_event` deltas are intentionally dropped** (the coalesced
  `assistant` frame is the source of truth). No token impact, but note that any
  usage that ever appeared *only* on a delta would be missed.
- **`sdk` `can_use_tool` stdio channel** is not wired — permissions go through
  the shared `--settings` hook path instead. No token/model parity impact;
  noted for completeness.

---

## 5. When to pick which

- **`bg` (default, recommended).** The safe default. Most battle-tested,
  supports the PTY keystroke permission fallback and the full observability
  story. Choose it unless you have a specific reason not to. Its cost is the
  `state.json` + transcript-file coupling.
- **`sdk`.** Best headless option when you want persistent multi-turn without
  disk coupling: structured events straight off stdout, in-place `set_model`,
  interrupt without teardown, on-demand crash cold-resume. Prefer over
  `oneshot` for interactive, long-lived conversations.
- **`oneshot`.** Simplest, most stateless: one `claude -p` per turn, no
  long-lived child to supervise. Good for short, one-and-done or scripted
  tasks. Every reply pays a fresh process launch, and there is no PTY keystroke
  permission fallback (permissions must be answered via the hook path).

For anything where metering certainty matters, keep `bg` until the §3
measurement is done — and re-check if the billing split un-pauses.
