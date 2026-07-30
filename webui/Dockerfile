# syntax=docker/dockerfile:1

# ---- build the static SPA ----
# Build context is the repo ROOT (see `make ui/image/build`): the webui imports
# the sibling gh-review UI (CCT-610) as workspace source and compiles it, so both
# `webui/` and `ghreview-ui/` must be in the context.
FROM node:22-alpine@sha256:76789712cd1ae89a1225eac9077010d68987a423588042dac30446f502f1858c AS build
WORKDIR /app

# gh-review is a path dependency + `$ghreview` vite-alias source. Install its
# deps so vite/esbuild can resolve its tsconfig + svelte; svelte is deduped to
# webui's single copy at bundle time (webui/vite.config.ts). ghreview-ui is a bun
# project (bun.lock, no package-lock.json), so it installs with bun.
COPY --from=oven/bun:1.3-alpine@sha256:efc5e42c7bedc1661ab0b7272c74c3ebf794f054297f530a62055f2d1a0eb662 /usr/local/bin/bun /usr/local/bin/bun
COPY ghreview-ui/ ./ghreview-ui/
RUN cd ghreview-ui && bun install --frozen-lockfile

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
