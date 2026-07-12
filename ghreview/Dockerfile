# cctui gh-review backend (CCT-612): the versioned HTTP+SSE GitHub review API.
# Bun runtime, backend-only — the review UI ships inside cctui-ui (CCT-610), so
# nothing static is served here.
#
# ⚠️ This repo is PUBLIC. No private hosts/registries/secrets in this file.
FROM oven/bun:1.3-alpine AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1.3-alpine
WORKDIR /app
ENV NODE_ENV=production
COPY --from=deps /app/node_modules ./node_modules
COPY package.json bun.lock ./
COPY src ./src
COPY migrations ./migrations
USER bun
EXPOSE 8790
ENTRYPOINT ["bun", "run", "src/index.ts"]
