#!/usr/bin/env bash
# Base fixture + marketing extras against the mkt-demo stack. Re-runnable.
#
#   source site/demo/env.sh && bash site/demo/seed.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
: "${CCTUI_TOKEN:?source site/demo/env.sh first}"
export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-mkt-demo}"
api="${CCTUI_API_URL:-http://localhost:8790}"
compose=(docker compose -p "$COMPOSE_PROJECT_NAME" -f "$root/deploy/local/docker-compose.yaml" --env-file "$root/tmp/mkt/.env")
psql() { "${compose[@]}" exec -T postgres psql -U postgres -d cctui -v ON_ERROR_STOP=1 -q "$@"; }

psql -c "DELETE FROM sessions WHERE machine_uuid::text LIKE 'c0000000-%' AND id NOT LIKE 'a0000000-%'"
bash "$root/deploy/local/fixture/seed.sh"

# seed-api.mjs registers sessions over HTTP, which parks handles in the server's
# in-memory registry that hide parent_id/adapter in the list. Drop them: the
# fake daemon registers the sessions properly over its WebSocket.
for n in 1 2 5; do
	curl -fsS -o /dev/null -X POST -H "Authorization: Bearer cctui_m_seed-fixture-workstation-01" \
		"$api/api/v1/sessions/a0000000-0000-4000-8000-00000000000$n/deregister"
done
psql < "$root/deploy/local/fixture/seed.sql"

note="$(mktemp --suffix=.ogg)"
ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=330:duration=2" -af "volume=0.3,afade=t=out:st=1.5:d=0.5" -c:a libopus "$note"
psql -v voice="$(base64 -w0 "$note")" < "$here/seed-extra.sql"
rm -f "$note"
node "$here/seed-extra.mjs"
echo "demo: seeded"
