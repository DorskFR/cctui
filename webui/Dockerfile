# syntax=docker/dockerfile:1

# ---- build the static SPA ----
FROM node:22-alpine AS build
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm ci || npm install
COPY . .
# Stamp the client build version (shown in the header next to the server
# version). Passed by `make ui/image/build` as the workspace version.
ARG CLIENT_VERSION=dev
ENV CLIENT_VERSION=$CLIENT_VERSION
# Bindings are committed (generated from the Rust structs); no Rust toolchain
# needed here. Build the adapter-static SPA into /app/build.
RUN npm run build

# ---- serve ----
FROM nginx:1.27-alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/build /usr/share/nginx/html
# config.js is overridable at deploy time (mount over this path) to point the
# SPA at a different API origin without rebuilding the image.
EXPOSE 8080
