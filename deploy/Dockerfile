FROM rust:1.85-slim AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/

RUN cargo build --release -p cctui-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cctui-server /usr/local/bin/cctui-server
EXPOSE 8700
CMD ["cctui-server"]
