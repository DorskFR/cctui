# Release channels

The tag decides the channel. `scripts/release-channel.sh` refuses any other shape.

| Tag             | Channel | GitHub release             | Image tags moved       |
|-----------------|---------|----------------------------|------------------------|
| `vX.Y.Z`        | stable  | full release, marked latest | `:X.Y.Z`, `:latest`   |
| `vX.Y.Z-beta.N` | beta    | pre-release, never latest   | `:X.Y.Z-beta.N`, `:beta` |

Both channels go through the same green-CI gate, and every asset is minisign-signed
and verified the same way.

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
  channel, is compiled into every binary and image, so `vX.Y.Z` is always its own
  build from its own tag. Tag the same commit the beta came from to ship
  identical code.
