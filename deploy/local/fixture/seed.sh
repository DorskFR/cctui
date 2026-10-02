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
compose="$(cd "$here/.." && pwd)/docker-compose.yaml"

# Every checkout's stack defaults to the project name `local`, so without an
# explicit COMPOSE_PROJECT_NAME the target is the one running stack built from
# this checkout's compose file, and anything else is refused.
compose_project() {
	if [[ -n "${COMPOSE_PROJECT_NAME:-}" ]]; then
		echo "$COMPOSE_PROJECT_NAME"
		return
	fi
	local projects
	projects="$(docker compose ls --format json | COMPOSE_FILE_PATH="$compose" node -e '
		let raw = "";
		process.stdin.on("data", (c) => (raw += c)).on("end", () => {
			for (const p of JSON.parse(raw || "[]")) {
				if (p.ConfigFiles.split(",").includes(process.env.COMPOSE_FILE_PATH)) console.log(p.Name);
			}
		});
	')"
	local count
	count="$(grep -c . <<<"$projects" || true)"
	if [[ "$count" -ne 1 ]]; then
		echo "seed: $count running stacks use $compose${projects:+ ($(tr '\n' ' ' <<<"$projects"))}; set COMPOSE_PROJECT_NAME or DATABASE_URL" >&2
		exit 1
	fi
	echo "$projects"
}

seed_db() {
	if [[ -n "${DATABASE_URL:-}" ]]; then
		psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -f "$seed"
	else
		docker compose -p "$project" -f "$compose" exec -T postgres \
			psql -U postgres -d cctui -v ON_ERROR_STOP=1 -q < "$seed"
	fi
}

# Registering a session resets the row's status and metadata, so the SQL half
# runs again after the API half to restore what registration clobbered.
if [[ -z "${DATABASE_URL:-}" ]]; then project="$(compose_project)"; fi
seed_db
node "$here/seed-api.mjs" $theme
seed_db
echo "fixture: seeded"
