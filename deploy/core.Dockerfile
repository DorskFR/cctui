# syntax=docker/dockerfile:1
# Release builds replace `bins` with the static release binary. Local builds
# compile the same opt-in server without workspace services or OpenSSL.
FROM rust:1.97.1-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS builder
ARG CCTUI_GIT_HASH=unknown
ENV CCTUI_GIT_HASH=${CCTUI_GIT_HASH}
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY keys/ keys/
COPY migrations/ migrations/
COPY plugins/ plugins/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p cctui-core-server \
    && mkdir -p /out \
    && cp target/release/cctui-core-server /out/

FROM scratch AS bins
COPY --from=builder /out/ /

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
ARG CA_CERTS_REFRESH=2026-10-22
RUN echo "ca-certificates refresh ${CA_CERTS_REFRESH}" && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN sed -i 's#^mozilla/emSign#!mozilla/emSign#' /etc/ca-certificates.conf \
    && update-ca-certificates --fresh >/dev/null \
    && ! ls /etc/ssl/certs | grep -qi emsign
COPY --from=bins /cctui-core-server /usr/local/bin/cctui-core-server
RUN useradd --system --uid 10001 --user-group --create-home --home-dir /home/cctui cctui
USER 10001
EXPOSE 8700
ENTRYPOINT ["cctui-core-server"]
