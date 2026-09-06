#!/usr/bin/env bash
# Load the invented demo fixture into a local cctui. Re-running it is a no-op.
#
#   make local/demo | make local/seed | deploy/local/fixture/seed.sh [theme]
#
# Seed timestamps are relative to now(), so this must run shortly before
# anything that reads sessions as live. The optional theme argument pins the
# stored UI theme, which is how the journey book captures one theme per pass.
set -euo pipefail

theme="${1:-}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
seed="$here/seed.sql"
compose="$here/../docker-compose.yaml"

seed_db() {
	if [[ -n "${DATABASE_URL:-}" ]]; then
		psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -f "$seed"
	else
		docker compose -f "$compose" exec -T postgres \
			psql -U postgres -d cctui -v ON_ERROR_STOP=1 -q < "$seed"
	fi
}

# Registering a session resets the row's status and metadata, so the SQL half
# runs again after the API half to restore what registration clobbered.
seed_db
node "$here/seed-api.mjs" $theme
seed_db
echo "fixture: seeded"
