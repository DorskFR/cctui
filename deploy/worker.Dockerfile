# cctui worker image (contract v1) — the execution environment dispatchers
# (the docker / kubernetes dispatchers) spawn per session. Bundles:
#   - the claude code CLI
#   - the codex CLI
#   - the cctui-daemon binary (session observability)
#   - the cctui sandbox toolchain: cctui-guard-proxy (egress allow-list),
#     cctui-supervisor (landlock + seccomp + cap-drop exec wrapper), and
#     cctui-guard (markdown-driven workflow guard daemon)
#
# It is **non-enrolled**: NO credentials and NO machine identity are baked in.
# Identity arrives at spawn time as env injected by the dispatcher:
#   CCTUI_URL           — the cctui-server base URL the daemon dials out to
#   CCTUI_MACHINE_KEY   — the shared machine key (the daemon runs without enroll)
#   SESSION_ID          — the pre-minted session to register
#   TASK_PAYLOAD_JSON   — the dispatch payload
#   REPLY_URL, TASK_NAME (optional)
#
# The daemon resolves CCTUI_MACHINE_KEY + CCTUI_URL from the environment
# (Config::from_env) and runs in ephemeral, --no-auto-update mode: the worker
# is short-lived and re-fetched on spawn, so there is nothing to self-update.
#
# This is a LEAN base. Heavy toolchains (Rust, Go, pnpm stores, dockerd) belong
# in **derived org images** (`FROM ghcr.io/<org>/cctui-worker`), never here —
# the base only ships what every worker needs to boot, sandbox, and talk git.
#
# See docs/worker-contract.md for the full env / mount / capability contract.
#
# ⚠️ This repo is PUBLIC. Keep it free of any private/homelab registries,
# hosts, or namespaces — neutral placeholders only.

# ── Builder: compile the worker binaries ────────────────────────────────────
# Match the runtime's glibc (bookworm-slim ships glibc 2.36) by building on the
# bookworm-based rust image, same as deploy/Dockerfile (see CCT-112).
FROM rust:1.90-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
# cctui-daemon's service.rs include_str!()s these unit templates at compile time.
COPY packaging/ packaging/

# sqlx runs in offline mode so no database is needed at build time.
ENV SQLX_OFFLINE=true
RUN cargo build --release \
        -p cctui-daemon \
        -p cctui-guard-proxy \
        -p cctui-supervisor \
        -p cctui-guard

# ── Runtime: claude code + codex + cctui binaries ───────────────────────────
FROM node:22-bookworm-slim

# Base tooling kept deliberately lean:
#   ca-certificates, libssl3 — TLS for the daemon's rustls/native deps.
#   git, git-lfs            — repo work and large-file fetches.
#   ripgrep                 — what claude code shells out to for search.
#   gnupg                   — GPG_PRIVATE_KEY_<ID> import + commit signing.
#   jq                      — payload unpack + result-callback synthesis.
#   curl                    — context-pack token auth, result callback, health.
#   rsync                   — warm-repo workspace fallback when overlayfs is off.
#   iptables                — transparent-mode egress REDIRECT to the proxy.
#   openssh-client          — git over SSH for credentialed clones.
#   gh                      — GitHub CLI for token auth + PR work.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        git-lfs \
        gnupg \
        iptables \
        jq \
        openssh-client \
        ripgrep \
        rsync \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Claude Code — installed via npm, which pulls the per-platform *native* binary;
# the `claude` bin does not invoke node at runtime. Version-pinnable at build.
ARG CLAUDE_CODE_VERSION=latest
RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
    && npm cache clean --force

# Codex — native static-musl binary from the GitHub release, NOT the npm package.
# The npm codex is a node entrypoint, so in derived images that put a node shim
# on PATH (e.g. mise managing the toolchain) launching it resolves `node`
# through the shim and can fail before codex starts (the
# "mise ERROR Permission denied (os error 13)" seen in the acme fat image).
# The standalone binary has no node dependency and sidesteps that entirely.
# Pinned + checksum-verified, mirroring the yt install above.
#
# Model provider: codex IGNORES OPENAI_API_KEY / OPENAI_BASE_URL env and reads
# its provider only from ~/.codex/config.toml. Do NOT bake a static config here —
# it would clobber codex's own runtime writes (trust_level) and pin a stale
# base_url. The entrypoint's phase_codex_config MERGES the cctui gateway provider
# in at runtime from the injected OPENAI_* env (CCT-517).
ARG CODEX_VERSION=0.142.4
RUN arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64) target=x86_64-unknown-linux-musl;  sha=f0ac43751c6d3b29a973a860a8de528ad79cb20cc1296611930a3d5c91ddef95 ;; \
         arm64) target=aarch64-unknown-linux-musl; sha=a546ee05915313fea340f8315b54f43d077f4390afbb5af2de944d48013d447f ;; \
         *) echo "codex: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-${target}.tar.gz" \
        -o /tmp/codex.tar.gz \
    && echo "${sha}  /tmp/codex.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/codex.tar.gz -C /usr/local/bin \
    && mv "/usr/local/bin/codex-${target}" /usr/local/bin/codex \
    && chmod 0755 /usr/local/bin/codex \
    && rm /tmp/codex.tar.gz \
    && codex --version

# yt — token-frugal YouTrack CLI (https://github.com/DorskFR/yt). Lets dispatched
# tasks triage / transition YouTrack issues without an MCP server. Tracks the
# rolling `latest` release (a moving tag the yt CI republishes on every release),
# so the worker always bakes the newest yt. Integrity is still checked against the
# release's own SHA256SUMS rather than a hard-pinned digest (which can't survive a
# moving tag). Reads creds from ~/.config/yt/config.json (materialized by
# a derived image's credential helper, from the YOUTRACK_URL/token env) or env.
ARG YT_VERSION=latest
RUN arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64|arm64) : ;; \
         *) echo "yt: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && base="https://github.com/DorskFR/yt/releases/download/${YT_VERSION}" \
    && curl -fsSL "${base}/yt-linux-${arch}" -o /usr/local/bin/yt \
    && curl -fsSL "${base}/SHA256SUMS" -o /tmp/yt.sums \
    && sha="$(awk -v f="yt-linux-${arch}" '$2==f{print $1}' /tmp/yt.sums)" \
    && [ -n "$sha" ] || { echo "yt: no checksum for yt-linux-${arch}" >&2; exit 1; } \
    && echo "${sha}  /usr/local/bin/yt" | sha256sum -c - \
    && rm /tmp/yt.sums \
    && chmod 0755 /usr/local/bin/yt \
    && yt --version

# scli — token-frugal Slack CLI (https://github.com/dorskFR/scli). Lets dispatched
# tasks read/post Slack without an MCP server. Tracks the rolling `latest` release
# (a moving tag the scli CI republishes on every release), so the worker always
# bakes the newest scli. Integrity is still checked against the release's own
# SHA256SUMS rather than a hard-pinned digest (which can't survive a moving tag).
# Reads creds from the SLACK_TOKEN env var (derived images may also materialize
# ~/.config/scli/config.json via their own credential helper). Release assets use
# linux-amd64/linux-arm64 naming (matching yt), so dpkg's arch maps directly.
ARG SCLI_VERSION=latest
RUN arch="$(dpkg --print-architecture)" \
    && case "$arch" in \
         amd64|arm64) : ;; \
         *) echo "scli: unsupported arch '$arch'" >&2; exit 1 ;; \
       esac \
    && base="https://github.com/dorskFR/scli/releases/download/${SCLI_VERSION}" \
    && curl -fsSL "${base}/scli-linux-${arch}" -o /usr/local/bin/scli \
    && curl -fsSL "${base}/SHA256SUMS" -o /tmp/scli.sums \
    && sha="$(awk -v f="scli-linux-${arch}" '$2==f{print $1}' /tmp/scli.sums)" \
    && [ -n "$sha" ] || { echo "scli: no checksum for scli-linux-${arch}" >&2; exit 1; } \
    && echo "${sha}  /usr/local/bin/scli" | sha256sum -c - \
    && rm /tmp/scli.sums \
    && chmod 0755 /usr/local/bin/scli \
    && scli --version

# Worker user (uid 1000). The container starts as root only to bootstrap the
# sandbox (iptables, overlayfs, context pack), then cctui-supervisor setuids to
# this user before exec'ing the daemon. A real home keeps per-session agent
# state (~/.claude, ~/.codex, ~/.mcp.json, ~/.npmrc, ~/.gnupg) writable.
# The node base image already occupies uid/gid 1000 with a `node` user, so
# rename it to `worker` and relocate its home rather than create a duplicate.
RUN groupmod --new-name worker node \
    && usermod --login worker --home /home/worker --move-home node \
    && chmod 0755 /home/worker

COPY --from=builder /app/target/release/cctui-daemon       /usr/local/bin/cctui-daemon
COPY --from=builder /app/target/release/cctui-guard-proxy  /usr/local/bin/cctui-guard-proxy
COPY --from=builder /app/target/release/cctui-supervisor   /usr/local/bin/cctui-supervisor
COPY --from=builder /app/target/release/cctui-guard        /usr/local/bin/cctui-guard
COPY deploy/worker-entrypoint.sh   /usr/local/bin/cctui-worker-entrypoint
# codex-run — safe one-shot `codex exec` wrapper (model/effort/approvals from
# config.toml; wrapper adds only --skip-git-repo-check + stdin-close + timeout).
COPY deploy/codex-run.sh           /usr/local/bin/codex-run

# Sandbox state dirs the entrypoint and proxy/guard write into. /opt/context is
# the context-pack mount target (read-only after fetch).
RUN mkdir -p /var/run/guard-proxy /var/run/workflow-guard /workspace /opt/context /opt/worker-entrypoint.d \
    && chmod +x /usr/local/bin/cctui-worker-entrypoint \
                /usr/local/bin/codex-run

# Contract marker: derived images and dispatchers can assert the wire contract.
LABEL dev.cctui.contract="1"

# HOME defaults to the worker home; the entrypoint runs as root first and the
# supervisor re-exports HOME=/home/worker for the dropped process.
ENV HOME=/home/worker
WORKDIR /home/worker

# Starts as root to bootstrap the sandbox; drops to uid 1000 via cctui-supervisor.
ENTRYPOINT ["/usr/local/bin/cctui-worker-entrypoint"]
