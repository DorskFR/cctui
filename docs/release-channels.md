# Release channels

The tag decides the channel. `scripts/release-channel.sh` refuses any other shape.

| Tag             | Channel | GitHub release             | Image tags moved       |
|-----------------|---------|----------------------------|------------------------|
| `vX.Y.Z`        | stable  | full release, marked latest | `:X.Y.Z`, `:latest`   |
| `vX.Y.Z-beta.N` | beta    | pre-release, never latest   | `:X.Y.Z-beta.N`, `:beta` |

Both channels go through the same green-CI gate, and every asset is minisign-signed
and verified the same way. A release rebuilds only what changed since the
previous release; see [Component-scoped releases](#component-scoped-releases).

## Which machines take a beta

A daemon updates to whatever version its server runs. It asks for the manifest
with `GET /api/v1/manifest/daemon?channel=<its channel>`; a beta server answers
`204 No Content` to any caller that does not ask for `beta`, including daemons
too old to send a channel, so they are never offered the beta. Both sides check:
the daemon also refuses a beta version itself when it follows stable. A machine
follows `stable` unless it opts in:

```toml
# ~/.config/cctui/daemon.toml
channel = "beta"
```

or `CCTUI_DAEMON_CHANNEL=beta` in the service environment. `cctui-daemon status`
prints the channel in effect; `cctui-daemon --version` prints the build's own
channel, e.g. `cctui-daemon 0.21.0-beta.1 (beta)`.

| Machine | Server offers         | Result                    |
|---------|-----------------------|---------------------------|
| stable  | newer stable          | installs                  |
| stable  | any beta              | stays put                 |
| beta    | newer beta or stable  | installs                  |
| any     | older version         | stays put (see rollback)  |

The TUI follows the same rule: a stable build never updates onto a beta server
unless `CCTUI_CHANNEL=beta` is set.

A server running a beta therefore only moves beta machines; stable machines on
it hold their version, logging that nothing is offered, until it returns to
stable. Remote `enroll` installs the channel the target already follows, so a
fresh or stable target cannot be enrolled against a beta server.

## Worker images

The harbor worker bake is keyed by the full version: `make release
VERSION=X.Y.Z-beta.N` builds on `cctui-worker:X.Y.Z-beta.N` and pushes the same
tag, which can never collide with a stable `X.Y.Z`. Never pass a `TAG=` override
for a beta, and point only beta worker profiles at beta tags.

A release that carries `cctui-worker` needs no bake: the worker profiles keep
the previous bake. The deployment repo's `make rollout VERSION=X.Y.Z[-beta.N]`
bakes only when `cctui-worker` is rebuilt. It then bumps
the pins of the rebuilt components, commits, pushes, refreshes Argo and waits
for the rollout.

## Promotion and rollback

- **Beta → its stable.** `X.Y.Z` sorts above `X.Y.Z-beta.N`, so once the server
  runs `X.Y.Z`, every machine on it (beta or stable) updates normally.
- **Beta → an older stable** (abandoning the beta). Point the server back at the
  older release, then on each beta machine run the updater once with the
  downgrade override:

  ```sh
  CCTUI_DAEMON_ALLOW_DOWNGRADE=1 cctui-daemon update
  ```

  Without it the daemon refuses anything older than what it runs. Set `channel`
  back to `stable` if the machine should leave the beta lane.
- **Promoting without a rebuild is not possible.** The version, and so the
  channel, is compiled into every binary, so `vX.Y.Z` never carries anything
  from a beta: the first stable of a line is always a full build from its own tag.
  Tag the same commit the beta came from to ship identical code.

## Component-scoped releases

`scripts/release-scope.sh` diffs the release commit against the previous
published release and decides, per component, whether to rebuild it or carry it
forward. A beta carries from the newest lower release of either channel; a
stable only from a stable, so it never reports a beta version. Version-only edits to `Cargo.toml`, `Cargo.lock`
and `webui/package*.json` do not count as changes.

| Changed since the previous release      | Rebuilt                                    | Carried             |
|-----------------------------------------|--------------------------------------------|---------------------|
| `webui/` only                           | `cctui-ui`                                 | every Rust image, all binaries |
| any Rust input (`crates/`, `Cargo.*`, `migrations/`, `keys/`, `packaging/`, `plugins/`, …) | every binary and every Rust image | `cctui-ui` unless `webui/` changed |
| a Rust image's Dockerfile or entrypoints | that image, around the previous release's binaries | everything else |
| `.dockerignore`, `release.yml`, the scope scripts | every image (Rust images around the previous binaries) | binaries |
| nothing                                 | nothing                                    | everything          |

Carrying a component means:

- **Images** — the previous release's image is tagged `:X.Y.Z[-beta.N]` and the
  channel tag, so every image still exists under every version.
- **Binaries** — the previous release's signed assets (`cctui-*`, `SHA256SUMS`,
  `*.minisig`) are attached to the new release unchanged.

Rust is all or nothing because every binary embeds the workspace version and the
server's daemon manifest serves the assets of the server's own version. A
carried server, daemon or worker therefore still reports the **previous
version**: `/api/v1/version` stays at the release that last changed Rust, and
daemons stay on it, which is correct because their code did not change. Only
`cctui-ui` reports the new version, through its header stamp. A carried server
also keeps the Claude Code `User-Agent` version baked in at its last build.

Each release attaches `release-scope.json` (`version`, `prev_tag`, `rust`, and
`modes` per image: `build`, `prevbins` or `retag`) and states what it rebuilt in
its notes. The infra rollout reads it to bump only the rebuilt components.

To rebuild everything, dispatch the release workflow with `full: true`, e.g.
when the previous release's images are missing or broken.

Builds start as soon as their own inputs exist: `cctui-ui` right after the scope
is resolved, the Rust images once the binaries are built. Every image is pushed
by digest only, untagged. The `ci-green` gate holds back only the GitHub Release
and the tagging, which is a manifest copy that takes seconds.
