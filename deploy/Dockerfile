FROM rust:1.89-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/

RUN cargo build --release --bin cctui-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/cctui-server /usr/local/bin/cctui-server
EXPOSE 8700
CMD ["cctui-server"]
