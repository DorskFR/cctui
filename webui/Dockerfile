# syntax=docker/dockerfile:1

# ---- build the static SPA ----
# Build context is the repo ROOT (see `make ui/image/build`).
FROM node:22-alpine@sha256:76789712cd1ae89a1225eac9077010d68987a423588042dac30446f502f1858c AS build
WORKDIR /app

WORKDIR /app/webui
COPY webui/package.json webui/package-lock.json ./
# --ignore-scripts: the `prepare` script compiles the paraglide i18n catalog,
# which needs project.inlang/ + messages/ that aren't copied yet.
RUN npm ci --ignore-scripts
COPY webui/ ./
# Stamp the client build version (shown in the header next to the server
# version). Passed by `make ui/image/build` as the workspace version.
ARG CLIENT_VERSION=dev
ENV CLIENT_VERSION=$CLIENT_VERSION
# Bindings are committed (generated from the Rust structs); no Rust toolchain
# needed here. Build the adapter-static SPA into /app/webui/build.
RUN npm run prepare && npm run build

# ---- serve ----
FROM nginx:1.27-alpine@sha256:62223d644fa234c3a1cc785ee14242ec47a77364226f1c811d2f669f96dc2ac8
COPY webui/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/webui/build /usr/share/nginx/html
# config.js is overridable at deploy time (mount over this path) to point the
# SPA at a different API origin without rebuilding the image.
EXPOSE 8080
