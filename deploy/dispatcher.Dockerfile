# cctui kubernetes dispatcher image. The standalone, enrolled
# dispatcher that connects out to a cctui-server, serves dispatch commands, and
# spawns worker Jobs in-cluster (cloning a suspended source CronJob's template).
#
# It is **enrolled**: the dispatcher key + namespace + source CronJob live in
# its config file (CCTUI_DISPATCHER_CONFIG, default ~/.config/cctui/dispatcher),
# mounted into the pod rather than baked into the image. No credentials here.
#
# ⚠️ This repo is PUBLIC. Keep it free of any private/homelab registries,
# hosts, or namespaces — neutral placeholders only.

# Builder base must match the runtime's glibc: bookworm-slim runtime ships
# glibc 2.36, so build on the bookworm-based rust image (not the default
# trixie one, whose binaries require GLIBC_2.39).
FROM rust:1.90-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/

# sqlx runs in offline mode so no database is needed at build time.
ENV SQLX_OFFLINE=true
RUN cargo build --release -p cctui-dispatcher-kube

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cctui-dispatcher-kube /usr/local/bin/cctui-dispatcher-kube
ENTRYPOINT ["/usr/local/bin/cctui-dispatcher-kube", "run"]
