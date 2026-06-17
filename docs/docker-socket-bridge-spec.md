# Spec: per-session docker socket bridge (CCT-367)

## Problem

`claude --bg` workers spawned by `cctui-daemon` cannot reach the host docker
socket on Linux. The daemon spawns workers as local PTY subprocesses with
`isolation: "none"` (`crates/cctui-daemon/src/adapters/claude_code/control.rs:1096`),
but each worker runs inside a restricted user namespace. Connecting to the
rootless docker **unix** socket from inside that namespace is denied by an LSM
(uid_map stays `1000 1000 1`; `dangerouslyDisableSandbox` and `chmod` do not
fix it). Connecting to a **loopback TCP** port is *not* denied — a manual
`socat`/python relay from `unix:/run/user/1000/docker.sock` to
`tcp://127.0.0.1:2375` lets the worker drive docker via
`DOCKER_HOST=tcp://127.0.0.1:2375`.

We want this automatic and per-session, not a hand-run script: the daemon
should optionally stand up a bridge for a session, gated by an allowlist,
selectable at spawn time.

## Decisive finding: the denial is docker-socket-specific, not cross-namespace

Probing from inside an actual restricted bg-worker namespace
(`uid_map = 1000 1000 1`):

- Connecting to the rootless `docker.sock` is denied (`EACCES`/errno 13) even
  though the worker owns it (ACL: owner `dorsk` rw, worker *is* `dorsk`). So
  it's not DAC — it's an AppArmor/label rule scoped to docker's socket.
- Connecting to **every other** outside-namespace unix socket in
  `$XDG_RUNTIME_DIR` succeeds from inside the namespace: `wayland-0`,
  `pipewire-0`, dbus `bus`, **and `cctui-daemon.sock` itself**.

That last one is the proof: **a cctui-daemon-created unix socket is reachable
from the worker namespace.** So a daemon-hosted unix-socket relay works, and it
is strictly better than a loopback TCP port (owner-only file perms, no exposure
to other local users/processes).

## Proposed design (unix socket, primary)

The daemon lives *outside* the worker's restricted namespace, so it can reach
the real `docker.sock` (same as the manual `docker-tcp-bridge.py`), and it
already owns the spawn lifecycle + env injection.

On spawn, when the session requests the bridge AND the daemon config permits it:

1. Create a per-session relay unix socket, mode `0600`, e.g.
   `$XDG_RUNTIME_DIR/cctui-docker-<short>.sock` (fall back to a private dir if
   `XDG_RUNTIME_DIR` is unset).
2. Relay each accepted connection bidirectionally to the configured docker
   socket path (default `$XDG_RUNTIME_DIR/docker.sock`, fall back to
   `/var/run/docker.sock`).
3. Inject `DOCKER_HOST=unix://<relay path>` into the session env (dispatch
   payload `env`, control.rs:1094) so the worker picks it up transparently.
4. Remove the socket + stop the relay when the session ends (SessionEnded /
   process exit), so sockets and tasks don't leak.

This is the proven bridge mechanism, owned by the daemon, scoped to one session,
over a unix socket the worker is allowed to reach. Unix socket only — no TCP
loopback (loopback would be reachable by any local process/user, and the unix
socket already works).

## Allowlist / gating

Docker access = host root-equivalent for the agent, so this is opt-in at three
layers:

1. **Daemon config** (`~/.config/cctui/daemon.toml`) — new section, default off:
   ```toml
   [docker_bridge]
   enabled = false                       # master switch for this machine
   socket_path = ""                      # "" => $XDG_RUNTIME_DIR/docker.sock
   # optional: restrict which working dirs may request it
   allowed_working_dirs = []             # empty => any, when enabled
   ```
   If `enabled = false`, a bridge request is rejected (or silently ignored with
   a warning in the session log) — the daemon never exposes docker unless the
   machine owner turned it on.

2. **Spawn request flag** — the session must explicitly ask:
   - `SpawnRequest.docker_bridge: bool` (`crates/cctui-proto/src/api.rs:359`)
   - `SessionSpec.docker_bridge: bool` (`crates/cctui-proto/src/adapter.rs`)
   - mapped server-side at `crates/cctui-server/src/routes/spawn.rs:146`

3. **(optional, later)** server-side per-account/role gate, if we ever want to
   stop arbitrary accounts from requesting it even on an enabled machine.

The allowlist is therefore: *daemon must enable the capability* AND *the spawn
must request it* AND (optionally) *the working dir must be in
`allowed_working_dirs`*.

## Implementation sketch

- **proto**: add `docker_bridge: bool` (default false, `#[serde(default)]`) to
  `SpawnRequest` and `SessionSpec`.
- **server**: copy the flag through `spawn.rs:146` (no validation beyond
  boolean). Keep the env-key regex untouched — the bridge injects `DOCKER_HOST`
  daemon-side, not via the user `env` map.
- **daemon**:
  - New module e.g. `crates/cctui-daemon/src/docker_bridge.rs` — a small tokio
    task: bind a per-session `UnixListener` (mode 0600), `accept()` loop,
    per-conn bidirectional copy (`tokio::io::copy_bidirectional`) between the
    accepted `UnixStream` and `UnixStream::connect(docker_socket_path)`.
  - In the spawn path (control.rs around 1081–1103): if `spec.docker_bridge`
    and config `enabled`, start a bridge, get its socket path, and add
    `DOCKER_HOST=unix://<relay path>` to the dispatch `env` (and `reattachEnv`
    so respawn keeps it).
  - Track the bridge handle keyed by session/short; abort it + unlink the socket
    on SessionEnded / worker exit. Re-establish on respawn (a fresh socket path
    per `short` is stable, so reattachEnv re-injection stays valid).
- **webui**: add a checkbox "Bridge docker socket" to `SpawnModal.svelte` form
  (`spawn/types.ts:8`, assembled at `SpawnModal.svelte:346`). Show it only when
  the selected adapter/machine supports it; disabled with a tooltip otherwise.
  Surface a clear warning that this grants the session host-docker (root) access.

## Open questions

- **Capability advertisement**: should the daemon advertise `docker_bridge`
  support (config enabled + docker reachable) in its manifest/registration so
  the webui can show/hide the checkbox per machine, rather than always showing
  it? Recommended — avoids offering a toggle that silently no-ops.
- **Daemon's own docker access**: confirmed implicitly (the manual bridge runs
  un-sandboxed and reaches `docker.sock`); the daemon systemd user service runs
  outside the restricted namespace so it inherits the same access. Verify on
  hosts where the daemon itself might be sandboxed.
- **Respawn / reattach**: use a stable per-`short` socket path so `reattachEnv`
  re-injection keeps `DOCKER_HOST` valid across the daemon-WS-cutover restart;
  re-create the listener on respawn if the socket was unlinked.
- **Dispatcher path**: docker/kube dispatchers already support bind mounts
  (`crates/cctui-dispatcher-docker/src/spawn.rs:155`). For containerized
  sessions the "bridge" is just mounting the socket — out of scope for v1
  (local PTY workers are the ones that actually need this), but the spawn flag
  could later drive a socket bind there too.
- **Security hardening**: the unix socket is owner-only (0600), so no extra
  local exposure beyond what the worker already has as the same user.

## Acceptance

- With `[docker_bridge] enabled = true` and a spawn requesting the bridge, the
  worker can run `docker ps` (via injected `DOCKER_HOST=unix://…`) without any
  manual relay.
- With `enabled = false`, the same spawn gets no `DOCKER_HOST` and a logged
  rejection.
- The relay socket is unlinked + the task stopped after the session ends (no
  leaked sockets/ports).
- WebUI checkbox spawns a working bridged session and carries the documented
  root-access warning.
