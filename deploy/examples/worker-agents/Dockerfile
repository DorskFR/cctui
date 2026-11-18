# syntax=docker/dockerfile:1
# A cctui worker with agent CLIs and tooling, built FROM the barebone
# cctui-worker. Each WITH_* switch (1 = on) adds one section; see README.md.
#
#   docker build --build-arg WITH_OPENCODE=1 -t my-worker deploy/examples/worker-agents
ARG BASE_IMAGE=ghcr.io/dorskfr/cctui-worker:latest
FROM ${BASE_IMAGE}

ARG WITH_CLAUDE=1
ARG WITH_CODEX=1
ARG WITH_OPENCODE=0
ARG WITH_ACP_AGENTS=0
ARG WITH_GOOSE=0
ARG WITH_TOOLS=1

# The base starts as root so the entrypoint can bootstrap the sandbox before it
# drops to uid 1000; this image keeps that.
USER root

# gh, ripgrep, git-lfs and openssh-client (git over SSH).
RUN [ "${WITH_TOOLS}" = 1 ] || exit 0; \
    apt-get update \
    && apt-get install -y --no-install-recommends git-lfs openssh-client ripgrep \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/* \
    && gh --version && rg --version && git lfs version

# Claude Code: the native binary from Anthropic's release CDN, not the npm
# package, so it never resolves a node from PATH. Checksum-verified against the
# release manifest. ripgrep is what it shells out to for search.
# A concrete x.y.z MINIMUM, not an exact pin: derived images may refetch the
# harness. scripts/check-claude-version-drift.sh enforces the floor, and
# scripts/check-worker-image-harness.sh runs it against a built image.
ARG CLAUDE_CODE_VERSION=2.1.304
RUN [ "${WITH_CLAUDE}" = 1 ] || exit 0; \
    apt-get update \
    && apt-get install -y --no-install-recommends ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && base="https://downloads.claude.ai/claude-code-releases" \
    && case "$(dpkg --print-architecture)" in \
         amd64) platform=linux-x64 ;; \
         arm64) platform=linux-arm64 ;; \
         *) echo "claude: unsupported arch" >&2; exit 1 ;; \
       esac \
    && case "${CLAUDE_CODE_VERSION}" in \
         latest|stable) version="$(curl -fsSL "${base}/${CLAUDE_CODE_VERSION}")" ;; \
         *) version="${CLAUDE_CODE_VERSION}" ;; \
       esac \
    && echo "${version}" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+' \
    && sha="$(curl -fsSL "${base}/${version}/manifest.json" \
        | jq -er --arg p "$platform" '.platforms[$p].checksum')" \
    && curl -fsSL "${base}/${version}/${platform}/claude" -o /usr/local/bin/claude \
    && echo "${sha}  /usr/local/bin/claude" | sha256sum -c - \
    && chmod 0755 /usr/local/bin/claude \
    && claude --version

# Codex: the standalone package from the GitHub release, not the npm package
# (a node entrypoint that breaks under a node shim on PATH). `codex app-server
# daemon` only runs from a complete standalone install, which the entrypoint
# links from CODEX_PACKAGE_DIR; it lives under /usr so Landlock can read it.
# procps provides the `ps` that `codex app-server daemon start` needs.
# No static ~/.codex/config.toml: the entrypoint merges the gateway provider in.
# A concrete x.y.z MINIMUM that must equal contract::CODEX_MIN_VERSION
# (crates/cctui-daemon/src/adapters/codex/contract.rs);
# scripts/check-codex-version-drift.sh enforces both.
ARG CODEX_VERSION=0.163.0
ENV CODEX_PACKAGE_DIR=/usr/local/lib/codex
RUN [ "${WITH_CODEX}" = 1 ] || exit 0; \
    apt-get update \
    && apt-get install -y --no-install-recommends procps \
    && rm -rf /var/lib/apt/lists/* \
    && arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64) target=x86_64-unknown-linux-musl;  sha=582646c6bf0561eb0de540f7fc85b9121b0091a826bc6e96830b7f1722122182 ;; \
         arm64) target=aarch64-unknown-linux-musl; sha=9d8ecf2d19d3e184877983902d6c85de48b3888dcb72c382a8a0c975845927fb ;; \
         *) echo "codex: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-package-${target}.tar.gz" \
        -o /tmp/codex.tar.gz \
    && echo "${sha}  /tmp/codex.tar.gz" | sha256sum -c - \
    && mkdir -p "$CODEX_PACKAGE_DIR" \
    && tar -xzf /tmp/codex.tar.gz -C "$CODEX_PACKAGE_DIR" \
    && ln -s bin/codex "$CODEX_PACKAGE_DIR/codex" \
    && ln -sf "$CODEX_PACKAGE_DIR/bin/codex" /usr/local/bin/codex \
    && rm /tmp/codex.tar.gz \
    && codex --version

# opencode, driven over `opencode serve`. The release carries no SHA256SUMS
# asset, so the digests are recorded per arch and refreshed with the version.
# No model id and no opencode config: the adapter writes a per-session
# opencode.json. Keep in lockstep with client::OPENCODE_PINNED_VERSION
# (crates/cctui-daemon/src/adapters/opencode/client.rs).
ARG OPENCODE_VERSION=1.19.3
RUN [ "${WITH_OPENCODE}" = 1 ] || exit 0; \
    arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64) target=x64;   sha=13a8fb2978302071b86f9d7e00da9c8895f9cd846978379c3bdc6a5333536c9b ;; \
         arm64) target=arm64; sha=7900818a6a98881edbbadab4b7525940eff3b9750040c200481f9f56cacee405 ;; \
         *) echo "opencode: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://github.com/anomalyco/opencode/releases/download/v${OPENCODE_VERSION}/opencode-linux-${target}.tar.gz" \
        -o /tmp/opencode.tar.gz \
    && echo "${sha}  /tmp/opencode.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/opencode.tar.gz -C /usr/local/bin opencode \
    && chmod 0755 /usr/local/bin/opencode \
    && rm /tmp/opencode.tar.gz \
    && opencode --version

# Node, then gemini-cli and qwen-code (crates/cctui-daemon/src/adapters/acp/rows.rs).
# Node comes from the nodejs.org tarball, checksum-verified against the
# release's SHASUMS256.txt; it links libatomic1 and needs xz-utils to unpack.
# Node also lets context packs run npx-based MCP servers.
# The agents are exact pins (scripts/check-worker-image-harness.sh). They live
# in their own npm prefix, and the `gemini` / `qwen` commands are wrappers that
# exec this node by absolute path: an npm bin stub resolves `node` through
# PATH, which a node shim (mise) in a derived image would break.
ARG NODE_VERSION=26.12.1
ARG GEMINI_CLI_VERSION=0.64.0
ARG QWEN_CODE_VERSION=0.25.3
ENV ACP_AGENTS_PREFIX=/usr/local/lib/acp-agents
RUN [ "${WITH_ACP_AGENTS}" = 1 ] || exit 0; \
    apt-get update \
    && apt-get install -y --no-install-recommends libatomic1 xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64) target=linux-x64 ;; \
         arm64) target=linux-arm64 ;; \
         *) echo "node: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && base="https://nodejs.org/dist/v${NODE_VERSION}" \
    && tarball="node-v${NODE_VERSION}-${target}.tar.xz" \
    && curl -fsSL "${base}/${tarball}" -o /tmp/node.tar.xz \
    && curl -fsSL "${base}/SHASUMS256.txt" -o /tmp/node.sums \
    && sha="$(awk -v f="$tarball" '$2==f{print $1}' /tmp/node.sums)" \
    && { [ -n "$sha" ] || { echo "node: no checksum for ${tarball}" >&2; exit 1; }; } \
    && echo "${sha}  /tmp/node.tar.xz" | sha256sum -c - \
    && mkdir -p /usr/local/node \
    && tar -xJf /tmp/node.tar.xz -C /usr/local/node --strip-components=1 \
    && rm /tmp/node.tar.xz /tmp/node.sums \
    # include/ is node-gyp's C++ headers, which need a compiler this image does
    # not ship; share/ is man pages and docs.
    && rm -rf /usr/local/node/include /usr/local/node/share \
    && ln -s /usr/local/node/bin/node /usr/local/bin/node \
    && ln -s /usr/local/node/bin/npm  /usr/local/bin/npm \
    && ln -s /usr/local/node/bin/npx  /usr/local/bin/npx \
    && node --version && npx --version \
    && npm install -g --prefix "$ACP_AGENTS_PREFIX" --no-fund --no-audit --loglevel=error \
        "@google/gemini-cli@${GEMINI_CLI_VERSION}" \
        "@qwen-code/qwen-code@${QWEN_CODE_VERSION}" \
    && npm cache clean --force \
    && rm -rf /root/.npm \
    && for agent in gemini qwen; do \
         entry="$(readlink -f "$ACP_AGENTS_PREFIX/bin/$agent")"; \
         [ -f "$entry" ] || { echo "$agent: no bin entry under $ACP_AGENTS_PREFIX" >&2; exit 1; }; \
         printf '#!/bin/sh\nexec /usr/local/node/bin/node "%s" "$@"\n' "$entry" > "/usr/local/bin/$agent"; \
         chmod 0755 "/usr/local/bin/$agent"; \
       done \
    && gemini --version \
    && qwen --version

# goose: a single binary from the GitHub release that links libstdc++6. No
# SHA256SUMS asset, so the digests are recorded per arch with the version.
ARG GOOSE_VERSION=1.55.0
RUN [ "${WITH_GOOSE}" = 1 ] || exit 0; \
    apt-get update \
    && apt-get install -y --no-install-recommends libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64) target=x86_64-unknown-linux-gnu;  sha=fd3932908328fac850ca7ad2a8289c744e0c2ddaf4c5a2338709fb17a00b3a4c ;; \
         arm64) target=aarch64-unknown-linux-gnu; sha=ea83f59bb165a3d999dd00edf69a58028747f31e404b96f0153452c6b1651188 ;; \
         *) echo "goose: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://github.com/aaif-goose/goose/releases/download/v${GOOSE_VERSION}/goose-${target}.tar.gz" \
        -o /tmp/goose.tar.gz \
    && echo "${sha}  /tmp/goose.tar.gz" | sha256sum -c - \
    && mkdir -p /tmp/goose \
    && tar -xzf /tmp/goose.tar.gz -C /tmp/goose \
    && install -m 0755 /tmp/goose/goose /usr/local/bin/goose \
    && rm -rf /tmp/goose /tmp/goose.tar.gz \
    && goose --version

# The pod is sandboxed by landlock + seccomp + the guard-proxy with
# deny-by-default egress, so an agent's own sandbox fails every command and a
# telemetry exporter only hangs. gemini and qwen must not re-exec through a
# different node; goose has no secret service to keep its keys in.
ENV GEMINI_SANDBOX=false \
    GEMINI_TELEMETRY_ENABLED=false \
    GEMINI_CLI_NO_RELAUNCH=1 \
    QWEN_SANDBOX=false \
    QWEN_TELEMETRY_ENABLED=false \
    QWEN_USAGE_STATISTICS_ENABLED=false \
    QWEN_CODE_NO_RELAUNCH=1 \
    GOOSE_DISABLE_KEYRING=1 \
    GOOSE_TELEMETRY_ENABLED=false
