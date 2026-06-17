# Builder base must match the runtime's glibc: bookworm-slim runtime ships
# glibc 2.36, so build on the bookworm-based rust image (not the default
# trixie one, whose binaries require GLIBC_2.39). See CCT-112.
FROM rust:1.90-slim-bookworm AS builder

ARG CCTUI_GIT_HASH=unknown
ENV CCTUI_GIT_HASH=${CCTUI_GIT_HASH}

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/

RUN cargo build --release -p cctui-server --features github

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cctui-server /usr/local/bin/cctui-server
EXPOSE 8700
CMD ["cctui-server"]
