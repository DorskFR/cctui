DATABASE_URL ?= postgres://postgres:postgres@localhost:5480/cctui
TEST_DATABASE_URL ?= postgres://postgres:postgres@localhost:5481/cctui_test
CCTUI_AGENT_TOKENS ?= dev-agent
CCTUI_ADMIN_TOKENS ?= dev-admin
CCTUI_URL ?= http://localhost:8700
CCTUI_TOKEN ?= dev-admin

export DATABASE_URL
export TEST_DATABASE_URL
export CCTUI_AGENT_TOKENS
export CCTUI_ADMIN_TOKENS
export CCTUI_URL
export CCTUI_TOKEN

.PHONY: setup build check test test/unit fmt lint clean
.PHONY: db/up db/down db/reset db/migrate/up db/migrate/down db/migrate/add db/psql db/prepare
.PHONY: db/test/up db/test/down db/test/migrate/up
.PHONY: run/server run/tui run/admin
.PHONY: image/build image/push image/release
.PHONY: worker/image/build worker/image/push worker/image/release
.PHONY: dispatcher-kube/image/build dispatcher-kube/image/push dispatcher-kube/image/release
.PHONY: local/up local/down local/logs local/pull local/ps
.PHONY: bindings webui/install webui/dev webui/build

# CI publishes images on tag push (.github/workflows/release.yml → ghcr). These
# `make image/*` targets are a local fallback; they default to the same ghcr
# namespace so a manual push lands where the cluster pulls from. (CCT-199)
IMAGE_REGISTRY ?= ghcr.io/dorskfr
IMAGE_REPO     ?= cctui
IMAGE_VERSION  ?= $(shell awk -F'"' '/^\[workspace.package\]/{f=1} f && /^version/{print $$2; exit}' Cargo.toml)
IMAGE          ?= $(IMAGE_REGISTRY)/$(IMAGE_REPO)

# A local build off an unclean tree does not match the released $(IMAGE_VERSION),
# so it gets its own tag and never masquerades as the release. CI tags :latest;
# local builds deliberately do not.
IMAGE_DIRTY ?= $(shell test -z "$$(git status --porcelain 2>/dev/null)" || echo -dirty)
IMAGE_TAG   ?= $(IMAGE_VERSION)$(IMAGE_DIRTY)

# ── Setup ──────────────────────────────────────────────────

setup: db/up db/migrate/up build  ## Full setup: database + build
	@echo "Setup complete. Run 'make run/server' then 'make run/tui'."

# ── Build ──────────────────────────────────────────────────

build:  ## Build all crates in release mode
	cargo build --release --workspace

check:  ## Type check all crates
	cargo check --workspace

# ── Format & Lint ──────────────────────────────────────────

fmt:  ## Auto-format Rust + non-Rust files
	cargo +nightly fmt --all
	biome check --write .

lint:  ## Run clippy with deny warnings
	cargo clippy --workspace --all-targets -- -D warnings

# ── Test ───────────────────────────────────────────────────

test: db/test/up db/test/migrate/up  ## Run all tests
	DATABASE_URL=$(TEST_DATABASE_URL) cargo test --workspace
	@echo "Tests complete."

test/unit:  ## Run unit tests only (no DB required)
	env -u DATABASE_URL -u TEST_DATABASE_URL cargo test --workspace

# ── Run ────────────────────────────────────────────────────

run/server:  ## Run the server locally
	cargo run -p cctui-server

run/tui:  ## Run the TUI client
	cargo run -p cctui-tui

run/admin:  ## Run cctui-admin (e.g. `make run/admin ARGS="user list"`)
	cargo run -p cctui-admin -- $(ARGS)

# ── Database ───────────────────────────────────────────────

db/up:  ## Start development database
	docker compose up -d cctui-postgres
	@echo "Waiting for postgres..."
	@until docker exec cctui-postgres pg_isready -U postgres > /dev/null 2>&1; do sleep 1; done
	@echo "Postgres ready on port 5480"

db/down:  ## Stop development database
	docker compose down -v --remove-orphans

db/reset: db/down db/up db/migrate/up  ## Reset development database

db/migrate/up:  ## Apply migrations
	sqlx migrate run --source migrations

db/migrate/down:  ## Revert last migration
	sqlx migrate revert --source migrations

db/migrate/add:  ## Create new migration (NAME=xxx)
	sqlx migrate add -r $(NAME) --source migrations

db/psql:  ## Open psql shell to dev database
	docker exec -it cctui-postgres psql -U postgres -d cctui

db/prepare:  ## Prepare sqlx offline metadata
	cargo sqlx prepare --workspace

# ── Test Database ──────────────────────────────────────────

db/test/up:  ## Start test database
	docker compose up -d cctui-postgres-test
	@until docker exec cctui-postgres-test pg_isready -U postgres > /dev/null 2>&1; do sleep 1; done

db/test/down:  ## Stop test database
	docker compose down -v --remove-orphans

db/test/migrate/up:  ## Apply migrations to test database
	DATABASE_URL=$(TEST_DATABASE_URL) sqlx migrate run --source migrations

# ── Clean ──────────────────────────────────────────────────

clean:  ## Remove build artifacts
	cargo clean

# ── Local stack (published images, no build) ───────────────
# Self-contained: postgres + cctui-server + cctui-ui pulled from ghcr and wired
# together. UI on :8088, server API on :8700. See deploy/local/.
LOCAL_COMPOSE ?= deploy/local/docker-compose.yaml

.PHONY: local/up local/down local/logs local/pull local/ps

local/up:  ## Start the full local stack (postgres + server + UI) from published images
	docker compose -f $(LOCAL_COMPOSE) up -d
	@echo "cctui up — UI: http://localhost:8088  ·  API: http://localhost:8700  (admin token: dev-admin)"

local/down:  ## Stop the local stack (keeps the postgres volume)
	docker compose -f $(LOCAL_COMPOSE) down

local/pull:  ## Pull the latest published images for the local stack
	docker compose -f $(LOCAL_COMPOSE) pull

local/logs:  ## Tail logs from the local stack
	docker compose -f $(LOCAL_COMPOSE) logs -f

local/ps:  ## Show local stack status
	docker compose -f $(LOCAL_COMPOSE) ps

# ── Deploy ──────────────────────────────────────────────────

build/server:  ## Build server docker image
	docker build -f deploy/Dockerfile -t ghcr.io/dorskfr/cctui-server:$(IMAGE_TAG) .

IMAGE_GIT_HASH ?= $(shell git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)

image/build:  ## Build container image ($(IMAGE):$(IMAGE_TAG))
	docker build -f deploy/Dockerfile \
	  --build-arg CCTUI_GIT_HASH=$(IMAGE_GIT_HASH) \
	  -t $(IMAGE):$(IMAGE_TAG) .

image/push:  ## Push container image tag
	docker push $(IMAGE):$(IMAGE_TAG)

image/release: image/build image/push  ## Build + push container image

# ── Worker image (claude code + codex + cctui-daemon, non-enrolled) ────────
# The execution environment dispatchers spawn per session (CCT-245). CI builds
# + pushes it on tag (see .github/workflows/release.yml); these targets are the
# same local fallback as image/*.
WORKER_IMAGE ?= $(IMAGE_REGISTRY)/cctui-worker

worker/image/build:  ## Build the worker image ($(WORKER_IMAGE):$(IMAGE_TAG))
	docker build -f deploy/worker.Dockerfile \
	  -t $(WORKER_IMAGE):$(IMAGE_TAG) .

worker/image/push:  ## Push the worker image tag
	docker push $(WORKER_IMAGE):$(IMAGE_TAG)

worker/image/release: worker/image/build worker/image/push  ## Build + push the worker image

# ── Kubernetes dispatcher image (standalone, enrolled) ─────────────────────
# The dispatcher that spawns worker Jobs in-cluster (CCT-291). CI builds +
# pushes it on tag (see .github/workflows/release.yml); these targets are the
# same local fallback as image/*.
DISPATCHER_KUBE_IMAGE ?= $(IMAGE_REGISTRY)/cctui-dispatcher-kube

dispatcher-kube/image/build:  ## Build the kube dispatcher image ($(DISPATCHER_KUBE_IMAGE):$(IMAGE_TAG))
	docker build -f deploy/dispatcher.Dockerfile \
	  -t $(DISPATCHER_KUBE_IMAGE):$(IMAGE_TAG) .

dispatcher-kube/image/push:  ## Push the kube dispatcher image tag
	docker push $(DISPATCHER_KUBE_IMAGE):$(IMAGE_TAG)

dispatcher-kube/image/release: dispatcher-kube/image/build dispatcher-kube/image/push  ## Build + push the kube dispatcher image

# ── Web UI image (standalone SPA) ──────────────────────────
UI_IMAGE ?= $(IMAGE_REGISTRY)/cctui-ui

bindings:  ## Regenerate webui TypeScript bindings from Rust structs
	bash webui/scripts/gen-bindings.sh

webui/install:  ## Install web UI dependencies (npm)
	cd webui && npm ci

webui/dev:  ## Run the web UI dev server (Vite)
	cd webui && npm run dev

webui/build:  ## Production build of the web UI
	cd webui && npm run build

ui/image/build:  ## Build the web UI image ($(UI_IMAGE):$(IMAGE_TAG))
	docker build -f webui/Dockerfile \
	  --build-arg CLIENT_VERSION=$(IMAGE_TAG) \
	  -t $(UI_IMAGE):$(IMAGE_TAG) .

ui/image/push:  ## Push the web UI image tag
	docker push $(UI_IMAGE):$(IMAGE_TAG)

ui/image/release: ui/image/build ui/image/push  ## Build + push the web UI image

# ── Help ───────────────────────────────────────────────────

help:  ## Show this help
	@grep -E '^[a-zA-Z_/]+:.*##' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*##"}; {printf "\033[36m%-25s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
