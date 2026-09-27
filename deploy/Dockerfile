# syntax=docker/dockerfile:1
# Local builds (`make image/build`) compile in the builder stage below. Release
# builds pass `--build-context bins=<dir>` with a prebuilt cctui-server
# (scripts/release-build-server.sh), which replaces the `bins` stage.
FROM rust:1.97.1-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS builder

ARG CCTUI_GIT_HASH=unknown
ENV CCTUI_GIT_HASH=${CCTUI_GIT_HASH}

# webauthn-rs (passkey login) links against OpenSSL, so the builder needs the
# headers and the runtime needs the library, both from the distro.
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY keys/ keys/
COPY migrations/ migrations/

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p cctui-server \
    && mkdir -p /out \
    && cp target/release/cctui-server /out/

FROM scratch AS bins
COPY --from=builder /out/ /

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3t64 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=bins /cctui-server /usr/local/bin/cctui-server
RUN useradd --system --uid 10001 --user-group --create-home --home-dir /home/cctui cctui \
    && mkdir -p /skills /archive && chown 10001:10001 /skills /archive
USER 10001
EXPOSE 8700
CMD ["cctui-server"]
