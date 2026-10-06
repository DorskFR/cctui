#!/usr/bin/env bash
# Runs the real seed.sh against a copied tree with a fake `docker` on PATH, so
# no stack is touched and the chosen compose project can be read back.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

root="$tmp/local"
mkdir -p "$root/fixture" "$tmp/bin"
cp "$here/seed.sh" "$root/fixture/seed.sh"
: > "$root/fixture/seed.sql"
: > "$root/fixture/seed-api.mjs"
: > "$root/docker-compose.yaml"
mine="$root/docker-compose.yaml"

cat > "$tmp/bin/docker" <<'STUB'
#!/usr/bin/env bash
if [[ "$1 $2" == "compose ls" ]]; then cat "$FAKE_LS"; exit 0; fi
echo "$*" >> "$FAKE_LOG"
STUB
chmod +x "$tmp/bin/docker"

run() {
  printf '%s' "$1" > "$tmp/ls.json"
  : > "$tmp/log"
  env -u COMPOSE_PROJECT_NAME -u DATABASE_URL ${2:+COMPOSE_PROJECT_NAME="$2"} \
    PATH="$tmp/bin:$PATH" FAKE_LS="$tmp/ls.json" FAKE_LOG="$tmp/log" \
    bash "$root/fixture/seed.sh" >"$tmp/out" 2>&1
}

execs_project() { [[ "$(grep -c "compose -p $1 -f $mine exec" "$tmp/log")" -eq 2 ]]; }
nothing_seeded() { [[ ! -s "$tmp/log" ]]; }
refused() { ! run "$@"; }

check "seeds the one stack started from this checkout" \
  run "[{\"Name\":\"wt-a\",\"ConfigFiles\":\"$mine\"},{\"Name\":\"local\",\"ConfigFiles\":\"/elsewhere/deploy/local/docker-compose.yaml\"}]"
check "  …through its own project" execs_project wt-a

check "refuses when only another checkout's stack runs" \
  refused "[{\"Name\":\"local\",\"ConfigFiles\":\"/elsewhere/deploy/local/docker-compose.yaml\"}]"
check "  …without seeding it" nothing_seeded

check "refuses when two stacks share this compose file" \
  refused "[{\"Name\":\"a\",\"ConfigFiles\":\"$mine\"},{\"Name\":\"b\",\"ConfigFiles\":\"$mine\"}]"
check "  …and names them" grep -q "(a b )" "$tmp/out"

check "an explicit COMPOSE_PROJECT_NAME wins" run "[]" picked
check "  …through that project" execs_project picked

cat > "$tmp/fake-api.mjs" <<'FAKE'
import { createServer } from 'node:http';
import { writeFileSync } from 'node:fs';
const settings = { version: 3, data: { plugins: { enabled: { other: true }, config: { other: { k: 'v' } } } } };
const server = createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c)).on('end', () => {
    const path = req.url.replace('/api/v1', '');
    if (req.method === 'PUT' && path === '/settings') writeFileSync(process.env.FAKE_PUT, body);
    const reply = req.method === 'GET' && path === '/settings' ? settings : req.method === 'GET' ? [] : {};
    res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(reply));
  });
});
server.listen(0, '127.0.0.1', () => console.log(server.address().port));
FAKE

FAKE_PUT="$tmp/put.json" node "$tmp/fake-api.mjs" > "$tmp/port" &
fake=$!
trap 'kill "$fake" 2>/dev/null; rm -rf "${tmp:?}"' EXIT
for _ in $(seq 50); do [[ -s "$tmp/port" ]] && break; sleep 0.1; done

seeds_plugin() {
  CCTUI_TOKEN=t CCTUI_API_URL="http://127.0.0.1:$(cat "$tmp/port")" node "$here/seed-api.mjs" >/dev/null
}
put_has() {
  node -e 'const d = JSON.parse(require("fs").readFileSync(process.argv[1])).data; process.exit(eval(process.argv[2]) ? 0 : 1)' "$tmp/put.json" "$1"
}

check "seed-api enables the pagedemo plugin for the seeded user" seeds_plugin
check "  …in the settings the plugin page reads" put_has 'd.plugins.enabled.pagedemo === true'
check "  …keeping the user's other plugin toggles" put_has 'd.plugins.enabled.other === true && d.plugins.config.other.k === "v"'

exit "$failures"
