# syntax=docker/dockerfile:1
# cctui worker image (contract v1): the barebone base dispatchers spawn per
# session and the guard-proxy sidecar runs from. It ships only what cctui's own
# scripts call: cctui-daemon, the sandbox toolchain (cctui-guard-proxy,
# cctui-supervisor, cctui-guard), the entrypoints, and their system tools.
#
# It ships NO agent CLI. A worker needs one: build FROM this image with the
# agents and tooling the deployment uses (deploy/examples/worker-agents). With
# no agent the entrypoint refuses to start a session.
#
# It is non-enrolled: no credentials and no machine identity are baked in; they
# arrive at spawn time as env (docs/worker-contract.md).
#
# This repo is PUBLIC: neutral placeholders only, no private registries or hosts.

# ── Builder: compile the worker binaries ────────────────────────────────────
# Match the runtime's glibc (trixie-slim ships glibc 2.41) by building on the
# trixie-based rust image, same as deploy/Dockerfile.
FROM rust:1.97.1-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY keys/ keys/
# cctui-daemon's service.rs include_str!()s these unit templates at compile time.
COPY packaging/ packaging/

# sqlx runs in offline mode so no database is needed at build time.
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p cctui-daemon -p cctui-guard-proxy -p cctui-supervisor -p cctui-guard \
    && mkdir -p /out \
    && cp target/release/cctui-daemon target/release/cctui-guard-proxy target/release/cctui-supervisor target/release/cctui-guard /out/

# Release builds replace this stage with prebuilt binaries (build context `bins`).
FROM scratch AS bins
COPY --from=builder /out/ /

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
ARG CA_CERTS_REFRESH=2026-10-22

#   curl, jq        — payload unpack, context-pack auth, result callback, health.
#   git             — workspace and context-pack clones.
#   iptables        — transparent-mode egress REDIRECT (entrypoint, net-init).
#   rsync           — warm-repo workspace fallback when overlayfs is off.
#   gnupg           — sidecar gpg-agent and the worker-side signing client.
RUN echo "ca-certificates refresh ${CA_CERTS_REFRESH}" && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        gnupg \
        iptables \
        jq \
        rsync \
    && rm -rf /var/lib/apt/lists/*
RUN sed -i 's#^mozilla/emSign#!mozilla/emSign#' /etc/ca-certificates.conf \
    && update-ca-certificates --fresh >/dev/null \
    && ! ls /etc/ssl/certs | grep -qi emsign

# Worker user (uid 1000). The container starts as root only to bootstrap the
# sandbox (iptables, overlayfs, context pack), then cctui-supervisor setuids to
# this user before exec'ing the daemon. A real home keeps per-session agent
# state (~/.claude, ~/.codex, ~/.gemini, ~/.qwen, ~/.config/goose, ~/.mcp.json,
# ~/.npmrc, ~/.gnupg) writable.
RUN groupadd --gid 1000 worker \
    && useradd --uid 1000 --gid 1000 --home-dir /home/worker --create-home --shell /bin/bash worker \
    && chmod 0755 /home/worker

COPY --from=bins /cctui-daemon       /usr/local/bin/cctui-daemon
COPY --from=bins /cctui-guard-proxy  /usr/local/bin/cctui-guard-proxy
COPY --from=bins /cctui-supervisor   /usr/local/bin/cctui-supervisor
COPY --from=bins /cctui-guard        /usr/local/bin/cctui-guard
# `guard why` lists the recent exec denials an agent hits under exec interception.
RUN ln -s cctui-guard /usr/local/bin/guard && guard why --help >/dev/null
COPY deploy/worker-entrypoint.sh   /usr/local/bin/cctui-worker-entrypoint
# worker-net-init — pod-netns iptables for the k8s sidecar mode: run
# from a NET_ADMIN init container so the worker container needs no privileged.
COPY deploy/worker-net-init.sh     /usr/local/bin/cctui-worker-net-init
# guard-proxy-entrypoint — sidecar boot wrapper: stands up a gpg-agent
# holding the signing key and forwards only its restricted --extra-socket, then
# exec's cctui-guard-proxy. Passthrough when no GPG_PRIVATE_KEY is present.
COPY deploy/guard-proxy-entrypoint.sh /usr/local/bin/cctui-guard-proxy-entrypoint
# Fail-closed pre-tool shims (opencode plugin, codex hook) that the entrypoint
# installs when the guard is on.
COPY --chmod=0755 deploy/guard-shims/ /usr/local/share/cctui/guard-shims/

# Sandbox state dirs the entrypoint and proxy/guard write into. /opt/context is
# the context-pack mount target (read-only after fetch). /workspace and
# /opt/context are worker-owned for a non-root (uid 1000) start; a root start
# re-owns both. The /var/run dirs stay root-owned so the agent can't rewrite
# guard state: a non-root pod mounts them as fsGroup 1000 emptyDirs.
RUN mkdir -p /var/run/guard-proxy /var/run/workflow-guard /opt/worker-entrypoint.d \
    && install -d -o 1000 -g 1000 /workspace /opt/context \
    && chmod +x /usr/local/bin/cctui-worker-entrypoint \
                /usr/local/bin/cctui-worker-net-init \
                /usr/local/bin/cctui-guard-proxy-entrypoint

# Contract marker: derived images and dispatchers can assert the wire contract.
LABEL dev.cctui.contract="1"

# HOME defaults to the worker home; the entrypoint runs as root first and the
# supervisor re-exports HOME=/home/worker for the dropped process.
ENV HOME=/home/worker
WORKDIR /home/worker

# Starts as root to bootstrap the sandbox; drops to uid 1000 via cctui-supervisor.
ENTRYPOINT ["/usr/local/bin/cctui-worker-entrypoint"]
