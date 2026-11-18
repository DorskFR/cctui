# Worker image with agents

`ghcr.io/dorskfr/cctui-worker` is a barebone base: the cctui binaries, the
entrypoint scripts and the tools they call. It ships no agent CLI, and a
worker without one refuses to start a session (see "No agent installed" in
[docs/worker-contract.md](../../../docs/worker-contract.md)). Build your worker
`FROM` it with the agents you use. This Dockerfile does that with one switch
per section.

```sh
docker build \
  --build-arg BASE_IMAGE=ghcr.io/dorskfr/cctui-worker:<version> \
  --build-arg WITH_OPENCODE=1 \
  -t registry.example.com/my-worker:<version> \
  deploy/examples/worker-agents
```

Pin `BASE_IMAGE` to the cctui release your server runs: the entrypoint and the
daemon in the base must match the server.

## Switches

| Build arg | Default | Adds |
| --- | --- | --- |
| `WITH_CLAUDE` | `1` | Claude Code (`claude`, native binary, floor `CLAUDE_CODE_VERSION`) and `ripgrep` |
| `WITH_CODEX` | `1` | Codex (`codex`, the standalone package under `CODEX_PACKAGE_DIR`, floor `CODEX_VERSION`) and `procps` |
| `WITH_OPENCODE` | `0` | opencode (`opencode`, exact pin `OPENCODE_VERSION`) |
| `WITH_ACP_AGENTS` | `0` | Node (`NODE_VERSION`, with `libatomic1` and `xz-utils`), gemini-cli and qwen-code (`gemini`, `qwen`, exact pins) |
| `WITH_GOOSE` | `0` | goose (`goose`, exact pin `GOOSE_VERSION`) and `libstdc++6` |
| `WITH_TOOLS` | `1` | `gh`, `ripgrep`, `git-lfs`, `openssh-client` |

`1` turns a section on; any other value leaves it out. Every download is
checksum-verified. `scripts/check-worker-image-harness.sh <image>` checks the
agents in a built image against this file's version args; CI runs it on the
all-on build.

## Adding another agent

Add a section that installs the binary, owns its system packages, and checks
`--version`:

```dockerfile
FROM ghcr.io/dorskfr/cctui-worker:<version>
USER root
RUN apt-get update && apt-get install -y --no-install-recommends <deps> \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL <release-url> -o /usr/local/bin/<agent> \
    && echo "<sha256>  /usr/local/bin/<agent>" | sha256sum -c - \
    && chmod 0755 /usr/local/bin/<agent> \
    && <agent> --version
```

The entrypoint and the daemon look an agent up by its adapter's default name
(`claude`, `codex`, `opencode`, `gemini`, `qwen`, `goose`) on `PATH`, then in
`~/.local/bin`, `/usr/local/bin`, `/usr/bin` and `/bin`. Install the binary, or
a symlink, under that name: an agent installed elsewhere or under another name
is not found at boot.

Landlock lets the agent read `/usr`, `/lib`, `/bin`, `/sbin` and `/etc`. An
agent or toolchain installed elsewhere (e.g. `/opt/<agent>`) must be listed in
`CCTUI_WORKER_EXTRA_RO` (colon-separated), or the agent cannot run it.

## Contract rules a derived image keeps

- **User.** The image starts as root: the entrypoint bootstraps the sandbox
  (iptables, overlayfs, the context pack), then `cctui-supervisor` drops to
  uid 1000. End on `USER root` (or no `USER`) and leave the `worker` user
  (uid/gid 1000) and `/workspace`, `/opt/context` ownership as the base sets
  them. A pod that must start as uid 1000 sets it in its `securityContext`
  (docs/worker-contract.md, Non-root start).
- **Entrypoint.** Do not override `ENTRYPOINT`; extend boot with hooks in
  `/opt/worker-entrypoint.d/` instead.
- **Label.** Keep `dev.cctui.contract=1`. It is inherited; do not reset it.
