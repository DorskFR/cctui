FROM oven/bun:1 AS channel-builder

WORKDIR /app/channel
COPY channel/package.json channel/bun.lock* ./
RUN bun install --frozen-lockfile
COPY channel/src/ src/
RUN bun run build

FROM rust:1.85-slim AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY --from=channel-builder /app/channel/dist/ channel/dist/

RUN cargo build --release -p cctui-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cctui-server /usr/local/bin/cctui-server
EXPOSE 8700
CMD ["cctui-server"]
