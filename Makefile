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

.PHONY: setup build check test fmt lint clean
.PHONY: db/up db/down db/reset db/migrate/up db/migrate/down db/migrate/add db/psql db/prepare
.PHONY: db/test/up db/test/down db/test/migrate/up
.PHONY: run/server run/tui run/channel run/admin
.PHONY: build/server build/channel deploy/server deploy/secrets

# ── Setup ──────────────────────────────────────────────────

setup: db/up db/migrate/up build  ## Full setup: database + build
	@echo "Setup complete. Run 'make run/server' then 'make run/tui'."

# ── Build ──────────────────────────────────────────────────

build:  ## Build all crates in release mode
	cargo build --release --workspace

build/channel:  ## Bundle channel TypeScript into channel/dist/channel.js
	cd channel && bun install && bun run build

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
	cargo test --workspace --lib

# ── Run ────────────────────────────────────────────────────

run/server:  ## Run the server locally
	cargo run -p cctui-server

run/tui:  ## Run the TUI client
	cargo run -p cctui-tui

run/admin:  ## Run cctui-admin (e.g. `make run/admin ARGS="user list"`)
	cargo run -p cctui-admin -- $(ARGS)

run/channel:  ## Install and run the channel server (for development)
	cd channel && bun install && bun run src/index.ts

test/session:  ## Simulate a session (register, stream events, deregister)
	./scripts/test-session.sh $(CCTUI_URL) $(CCTUI_TOKEN)

setup/claude:  ## Configure local Claude Code to use cctui-channel
	./scripts/setup-claude.sh $(CCTUI_URL) dev-agent

run/claude:  ## Start Claude Code with channel enabled (TUI messaging works)
	claude --dangerously-load-development-channels server:cctui

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

# ── Deploy ──────────────────────────────────────────────────

build/server:  ## Build server docker image
	docker build -f deploy/Dockerfile -t ghcr.io/dorskfr/cctui-server:latest .

deploy/server:  ## Deploy to K8s cluster
	kubectl apply -k deploy/k8s/

deploy/secrets:  ## Create K8s secrets (set env vars first)
	kubectl create secret generic cctui-secrets \
	  --namespace=cctui \
	  --from-literal=database-url="$(DATABASE_URL)" \
	  --from-literal=agent-tokens="$(CCTUI_AGENT_TOKENS)" \
	  --from-literal=admin-tokens="$(CCTUI_ADMIN_TOKENS)" \
	  --from-literal=vault-key="$(CCTUI_VAULT_KEY)" \
	  --dry-run=client -o yaml | kubectl apply -f -

# ── Help ───────────────────────────────────────────────────

help:  ## Show this help
	@grep -E '^[a-zA-Z_/]+:.*##' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*##"}; {printf "\033[36m%-25s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
