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
.PHONY: run/server run/tui run/shim

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
	cargo test --workspace --lib

# ── Run ────────────────────────────────────────────────────

run/server:  ## Run the server locally
	cargo run -p cctui-server

run/tui:  ## Run the TUI client
	cargo run -p cctui-tui

run/shim:  ## Run the shim (pipe stdin to server WS)
	cargo run -p cctui-shim -- relay --session-id $(SESSION_ID) --ws-url $(WS_URL)

test/session:  ## Simulate a session (register, stream events, deregister)
	./scripts/test-session.sh $(CCTUI_URL) $(CCTUI_TOKEN)

setup/claude:  ## Configure local Claude Code to auto-register with the server
	./scripts/setup-claude.sh $(CCTUI_URL) dev-agent

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

# ── Help ───────────────────────────────────────────────────

help:  ## Show this help
	@grep -E '^[a-zA-Z_/]+:.*##' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*##"}; {printf "\033[36m%-25s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
