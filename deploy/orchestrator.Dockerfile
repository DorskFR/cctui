# syntax=docker/dockerfile:1
# cctui admission webhook image. Serves the mutating (/mutate) and validating
# (/validate) webhooks over TLS; the /validate handler reads WorkerProfile CRs
# via the in-cluster client. No credentials are baked in — TLS material and the
# sidecar image override arrive at runtime via env and mounted secrets.
#
# ⚠️ This repo is PUBLIC. Keep it free of any private/homelab registries,
# hosts, or namespaces — neutral placeholders only.

# Builder base must match the runtime's glibc: bookworm-slim runtime ships
# glibc 2.36, so build on the bookworm-based rust image (not the default
# trixie one, whose binaries require GLIBC_2.39).
FROM rust:1.97.1-slim-bookworm@sha256:b001fed8c602fe3126bfee18c7afa14fe58dc855ce1d0cdfb4ac3ee7d6361a1c AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY keys/ keys/

# sqlx runs in offline mode so no database is needed at build time.
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p cctui-orchestrator \
    && mkdir -p /out \
    && cp target/release/cctui-orchestrator /out/

# Release builds replace this stage with prebuilt binaries (build context `bins`).
FROM scratch AS bins
COPY --from=builder /out/ /

FROM debian:bookworm-slim@sha256:63a496b5d3b99214b39f5ed70eb71a61e590a77979c79cbee4faf991f8c0783e
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=bins /cctui-orchestrator /usr/local/bin/cctui-orchestrator
RUN useradd --system --uid 10001 --user-group --create-home --home-dir /home/cctui cctui
USER 10001
ENTRYPOINT ["/usr/local/bin/cctui-orchestrator"]
