# cctui worker image — the execution environment dispatchers (the docker /
# kubernetes dispatchers) spawn per session. Bundles:
#   - the claude code CLI
#   - the codex CLI
#   - the cctui-daemon binary
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
# ⚠️ This repo is PUBLIC. Keep it free of any private/homelab registries,
# hosts, or namespaces — neutral placeholders only.

# ── Builder: compile cctui-daemon ──────────────────────────────────────────
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
RUN cargo build --release -p cctui-daemon

# ── Runtime: claude code + codex + cctui-daemon ─────────────────────────────
FROM node:22-bookworm-slim

# git + ca-certificates for repo work and TLS; libssl3 for the daemon's
# rustls/native deps; ripgrep is what claude code shells out to for search.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        libssl3 \
        ripgrep \
    && rm -rf /var/lib/apt/lists/*

# Agent CLIs, version-pinnable at build time. Defaults track latest; CI / a
# local build can pin via --build-arg for reproducibility.
ARG CLAUDE_CODE_VERSION=latest
ARG CODEX_VERSION=latest
RUN npm install -g \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force

COPY --from=builder /app/target/release/cctui-daemon /usr/local/bin/cctui-daemon
COPY deploy/worker-entrypoint.sh /usr/local/bin/cctui-worker-entrypoint

# Run as an unprivileged user with a writable HOME for per-session agent state
# (the node base image ships the `node` user). The daemon writes nothing to a
# config file in this mode — it reads its identity from the environment.
ENV HOME=/home/node
USER node
WORKDIR /home/node

ENTRYPOINT ["/usr/local/bin/cctui-worker-entrypoint"]
