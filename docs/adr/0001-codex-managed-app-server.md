# ADR 0001 — Codex: managed app-server architecture

- **Status:** Proposed
- **Date:** 2026-07-12
- **Ticket:** CCT-645
- **Deciders:** cctui daemon maintainers
- **Supersedes / relates to:** CCT-98 (app-server driver), CCT-263/339/632
  (thread/list inventory), CCT-461/482 (gateway env), CCT-631/635/639/640/641
  (protocol client hardening, native lifecycle, diagnose, model catalog)

## Context

The codex adapter (`crates/cctui-daemon/src/adapters/codex/`) drives every
cctui-owned session by spawning `codex app-server` and speaking newline-delimited
JSON-RPC 2.0 over **stdio**. Codex mints thread ids (rollout `UUIDv7`), and the
protocol supports many threads per connection (`thread/start` per thread). After
CCT-631/635/639/640/641 the protocol client is reliable: correlated
`PendingRpcs`, deferred spawn acks, timeouts, hibernate/resume, native
archive/unarchive, diagnose snapshots, and the model catalog are all in place.

Today's process model spawns codex **five different ways**, none of them shared:

1. **One long-lived stdio child per owned session.** `command_pump`
   (`mod.rs`) spawns a `CodexSession` per `Spawn`/`Fork`; `run_inner`
   (`app_server.rs`) launches `codex app-server`, runs
   `initialize → initialized → thread/start|resume|fork`, then pumps IO for that
   **one** thread until exit. On a clean exit the session *hibernates*: the
   durable `SessionRegistry` record survives, and the next `Send`/`Rename`/
   `SetModel` relaunches a fresh child via `thread/resume`
   (`route_or_prepare_resume` → `spawn_resumed_session`).
2. **Inventory poll — a short-lived child every 15 s.** `thread_list::poll_threads`
   spawns `codex app-server`, runs `initialize → thread/list` (paginated), reads
   the response, and reaps. This is the parity-with-claude machine inventory of
   *every* session (cli/vscode/exec/appServer/subAgent).
3. **Startup rediscovery — one short-lived child at boot.**
   `thread_list::rediscover_owned` runs the same one-shot to re-seed the registry
   for cctui-owned threads after a daemon restart/self-update.
4. **Model catalog poll — a short-lived child every 300 s.**
   `model_list::poll_models` runs `initialize → model/list` and reaps.
5. **Native lifecycle ops — a short-lived child per Archive/Unarchive/Delete.**
   `run_thread_lifecycle` (invoked from `Remove`/`Resume`) runs
   `initialize → thread/{archive,unarchive}` and reaps.

### Costs of the status quo

- **Handshake tax.** Every process — including each 15 s inventory poll, each
  5 min catalog poll, each lifecycle op — pays a fresh `codex app-server` boot
  plus the full `initialize` handshake. `RPC_TIMEOUT` is 30 s and
  `HANDSHAKE_TIMEOUT` is 120 s, so a cold `thread/resume` of a long transcript
  can be slow, and it is re-paid on every hibernation wake.
- **Inventory staleness.** New external sessions (a fresh `codex` TUI, a VS Code
  thread) are invisible for up to the 15 s poll interval; status transitions lag
  by the same window.
- **PID / task sprawl.** With *N* live sessions the daemon holds *N*
  `codex app-server` children plus a stderr-drain task each, and on top of that a
  drumbeat of one-shot children (inventory every 15 s, catalog every 5 min, a
  child per lifecycle op). Each is an independent process boot, DB open, and
  auth/context load.
- **Redundant DB opens.** `thread/list`, `model/list`, and each session all open
  codex's state DB independently rather than sharing one loaded process.

### What codex 0.144.1 actually supports (verified against the local binary)

This is the load-bearing finding, and it changes the option space. `codex
app-server` is **not** stdio-only in 0.144.1 (pinned:
`contract::CODEX_PINNED_VERSION = "0.144.1"`, min `0.142.0`):

- `codex app-server --listen <URL>` accepts `stdio://` (default), **`unix://PATH`**,
  **`ws://IP:PORT`**, or `off`. A long-lived socket-listening app-server is a
  first-class, shipped mode — not something we have to invent.
- `codex app-server daemon` manages a **local managed daemon**: `start`,
  `restart`, `stop`, `version` (CLI + running app-server versions as JSON),
  `bootstrap` ("Install durable local app-server management for SSH-driven use"),
  and `enable-remote-control` / `disable-remote-control`.
- `codex app-server proxy --sock <PATH>` **bridges stdio to the running
  app-server's control socket** — i.e. codex already ships a stdio↔socket shim, so
  a client that only knows how to speak stdio JSON-RPC (like our current driver)
  can attach to the managed daemon with minimal change.

Caveat, to be honest about scope: the socket/daemon path is marked
`[experimental]` in `--help`, and the schema's `port`/`unixSockets`/`transport`
fields are **unrelated** — they belong to sandbox network-approval policy
(`experimental_network`) and the experimental thread-realtime (websocket/webrtc)
transport, not to app-server multiplexing. So the socket transport exists at the
CLI/daemon layer, but it is young.

## Decision drivers (forces)

- **Account/config isolation.** cctui launches sessions under different
  `OPENAI_BASE_URL`/`OPENAI_API_KEY` gateway envs, resolved per-session from the
  server's durable `sessions.account_id` binding (CCT-461/482), and different
  accounts imply different `CODEX_HOME`/auth. A single shared app-server cannot
  carry per-thread gateway credentials — env is a **process-level** input today
  (`cmd.env(...)` in `run_inner`). This is the central constraint.
- **Crash blast radius.** One child per session means a crash kills one session.
  A shared multiplexer means one crash can take down every thread on it.
- **Reconnect/resume semantics.** We already hibernate+resume per thread. A
  managed daemon that outlives our daemon would let a self-update reattach
  instead of cold-resuming N threads — but only if the daemon truly survives our
  cgroup teardown.
- **Memory / footprint.** One loaded process vs N; one DB open vs many.
- **Multi-thread protocol support.** The protocol multiplexes threads over one
  connection (`thread/start` per thread, ids on every frame), so a single
  connection *can* carry all threads — the driver's per-thread state
  (`ActiveTurn`, `ItemAccumulator`, `PendingRpcs`, approvals) would need to be
  keyed by `threadId` instead of living in one per-process loop.
- **Experimental-surface risk.** The socket/daemon mode is new; betting the core
  session path on it before it stabilises is riskier than betting the read-only
  inventory path on it.

## Options

### A. Status quo — per-session stdio processes (+ one-shot inventory/catalog)

Keep exactly today's model. Simple, well-tested, strong isolation (env and crash
are naturally per-process). Costs: handshake tax, up-to-15 s inventory
staleness, PID sprawl, redundant DB opens.

### B. Single long-lived per-machine app-server multiplexing all threads

One `codex app-server --listen unix://…` (or via `daemon start`) for the whole
machine; drive every thread over it with `thread/start`/`thread/resume`, keying
driver state by `threadId`. Immediate inventory (no poll — subscribe to
thread/status), one DB open, one handshake, minimal PID footprint.

**Blocked by account isolation.** A single process has **one** env/`CODEX_HOME`/
auth. cctui's whole point is running threads bound to *different* gateway
accounts; per-turn `model`/`effort` already ride on `turn/start`, but gateway
routing is a process env var. Unless codex grows per-thread auth/base-url in the
protocol, option B cannot serve multi-account use without silently mis-routing —
the exact class of bug CCT-460/461 fixed. Also maximal crash blast radius.

### C. Per-account managed app-servers

One long-lived managed daemon **per distinct account/`CODEX_HOME`** (keyed by the
resolved gateway identity), each multiplexing that account's threads. Preserves
isolation (each process has its account's env), while still amortising handshake/
DB/PID within an account. Cost: a small pool of long-lived daemons and a
lifecycle manager (spawn on first use of an account, idle-evict, health-check,
restart-on-crash with per-account blast radius). This is the honest target if we
want multiplexing *and* isolation.

### D. Hybrid — managed long-lived for account-neutral reads, per-session for turns

Keep option A's per-session stdio child for anything that starts a turn (full
isolation, unchanged blast radius), but replace the three one-shot spawners
(`thread/list` inventory, `model/list` catalog, `thread/{archive,unarchive}`
lifecycle) with **one** long-lived read-only app-server. These calls start no
turn and need no gateway env (already true: `poll_threads`/`poll_models`/
`run_thread_lifecycle` pass only `sandbox_mode`, never the gateway env), so they
are account-neutral and safe to share. Wins: kills the 15 s/5 min/​per-op process
churn, enables push-based (near-zero-lag) inventory via `thread/status` events,
one DB open for all reads — with **zero** change to the isolation-critical turn
path and no new multi-account protocol dependency.

## Decision

**Adopt D now; treat C as the eventual target for the turn path; do not do B.**

Rationale:

- D captures most of the real cost (handshake tax on the high-frequency pollers,
  inventory staleness, one-shot PID/DB churn) **without touching the
  isolation-critical path**, and depends on the experimental socket only for
  read-only, non-authed calls where a failure degrades gracefully to today's
  one-shots — matching the existing "probe failure ⇒ silent fallback" contract in
  `thread_list`/`model_list`.
- B is rejected: a single shared process cannot honour per-thread gateway
  accounts (env is process-level), so it would reintroduce the CCT-460/461
  mis-routing class and give the worst crash blast radius.
- C is the right home for multiplexing the turn path *if* we later want it, but
  it is a per-account daemon pool with real lifecycle/health/eviction surface,
  and the socket mode is still `[experimental]`. It should not be the first
  corrective change. Ship D, learn how the managed daemon behaves in production
  on the read path, then revisit C.

## Migration sketch (option D)

1. **Introduce a managed read-only app-server handle.** A small module owning one
   long-lived `codex app-server` (start via `daemon start` + attach through
   `proxy --sock`, or `--listen unix://$XDG_RUNTIME_DIR/cctui-codex-appserver.sock`),
   with health-check + restart-on-exit and the same `sandbox_mode` the one-shots
   pass. Keep it **account-neutral** — never inject a gateway env.
2. **Route the three read paths through it.** Replace `poll_threads`,
   `poll_models`, and `run_thread_lifecycle`'s per-call spawn with a request over
   the shared connection, correlated by id (the same `read_response`/`PendingRpcs`
   pattern). Preserve the current pagination and idempotent-lifecycle logic.
3. **Push-based inventory.** Once attached, subscribe to `thread/status`/
   thread-list change notifications for near-zero-lag inventory instead of the
   15 s tick; keep a slow reconcile tick as a backstop.
4. **Fallback stays.** If the managed server is unavailable (missing binary,
   userns/sandbox, experimental-mode breakage), fall back to the current
   one-shot spawns — no regression, exactly the existing silent-degrade contract.
5. **Leave the turn path (Spawn/Fork/Resume) unchanged.** Per-session stdio
   children, per-session gateway env, per-session crash isolation all stay.

## Non-goals

- **Not** multiplexing the turn/session path in this change (that is C, and it
  needs per-account daemons + protocol-level per-thread auth or per-account
  process pools).
- **Not** adopting `ws://` remote transport or `enable-remote-control` /
  `bootstrap` (SSH-driven remote use) — out of scope; cctui is local-daemon.
- **Not** changing session identity, hibernate/resume, gateway-env resolution
  (CCT-461/482), or the fail-closed account-bound launch contract.
- **Not** restarting or replacing the per-machine `cctui-daemon` process model;
  the codex managed app-server is a child the daemon supervises, not a peer.

## Consequences

- **Positive:** high-frequency process churn removed; inventory becomes push-based
  and fresh; one DB open for reads; smaller PID footprint; isolation and crash
  blast radius of the turn path unchanged.
- **Negative / risk:** a new long-lived supervised child on an `[experimental]`
  codex surface; a shared read server is a (recoverable, fallback-guarded) single
  point for inventory/catalog. Revisiting C later still faces the unsolved
  per-thread-auth question, which is the real gate on multiplexing turns.
