# cctui gh-review backend (CCT-612): the versioned HTTP+SSE GitHub review API.
# Bun runtime, backend-only — the review UI ships inside cctui-ui (CCT-610), so
# nothing static is served here.
#
# ⚠️ This repo is PUBLIC. No private hosts/registries/secrets in this file.
FROM oven/bun:1.3-alpine@sha256:efc5e42c7bedc1661ab0b7272c74c3ebf794f054297f530a62055f2d1a0eb662 AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1.3-alpine@sha256:efc5e42c7bedc1661ab0b7272c74c3ebf794f054297f530a62055f2d1a0eb662
WORKDIR /app
ENV NODE_ENV=production
COPY --from=deps /app/node_modules ./node_modules
COPY package.json bun.lock ./
COPY src ./src
COPY migrations ./migrations
USER bun
EXPOSE 8790
ENTRYPOINT ["bun", "run", "src/index.ts"]
