# syntax=docker/dockerfile:1

# ---- build the static SPA ----
# Build context is the repo ROOT (see `make ui/image/build`): the webui imports
# the sibling gh-review UI (CCT-610) as workspace source and compiles it, so both
# `webui/` and `ghreview-ui/` must be in the context.
FROM node:22-alpine AS build
WORKDIR /app

# gh-review is a path dependency + `$ghreview` vite-alias source. Install its
# deps so vite/esbuild can resolve its tsconfig + svelte; svelte is deduped to
# webui's single copy at bundle time (webui/vite.config.ts).
COPY ghreview-ui/ ./ghreview-ui/
RUN cd ghreview-ui && npm install --no-audit --no-fund

WORKDIR /app/webui
COPY webui/package.json webui/package-lock.json* ./
# --ignore-scripts: the `prepare` script compiles the paraglide i18n catalog,
# which needs project.inlang/ + messages/ that aren't copied yet.
RUN npm ci --ignore-scripts || npm install --ignore-scripts
COPY webui/ ./
# Stamp the client build version (shown in the header next to the server
# version). Passed by `make ui/image/build` as the workspace version.
ARG CLIENT_VERSION=dev
ENV CLIENT_VERSION=$CLIENT_VERSION
# Bindings are committed (generated from the Rust structs); no Rust toolchain
# needed here. Build the adapter-static SPA into /app/webui/build.
RUN npm run prepare && npm run build

# ---- serve ----
FROM nginx:1.27-alpine
COPY webui/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/webui/build /usr/share/nginx/html
# config.js is overridable at deploy time (mount over this path) to point the
# SPA at a different API origin without rebuilding the image.
EXPOSE 8080
