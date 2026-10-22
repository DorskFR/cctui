#!/usr/bin/env bash
# Exercises repo_registry_hostports() + phase_repo_registries() from deploy/worker-entrypoint.sh.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

log() { :; }
POLICY_FILE="$TMP/policy.json"
PRIVATE_ALLOW_FILE="$TMP/private-allow"
eval "$(grep '^GATEWAY_URL_KEYS=' "$ENTRY")"
for fn in url_hostport repo_hostport gateway_url gateway_hostports private_allow_lines seed_policy \
          repo_registry_hostports phase_repo_registries; do
  eval "$(awk "/^${fn}\\(\\)/,/^}/" "$ENTRY")"
  type "$fn" >/dev/null 2>&1 || { bad "$fn not found in $ENTRY"; exit 1; }
done
# The phase reads the repo under /workspace; point it at a scratch tree instead.
eval "$(type phase_repo_registries | sed '1d;s,/workspace/,'"$TMP"'/ws/,')"

repo() { rm -rf "$TMP/ws"; mkdir -p "$TMP/ws/widgets"; }
hosts() { repo_registry_hostports "$TMP/ws/widgets" | tr '\n' ' ' | sed 's/ $//'; }

NPM="registry.npmjs.org:443"
PYPI="files.pythonhosted.org:443 pypi.org:443"
CRATES="crates.io:443 index.crates.io:443 static.crates.io:443"
GO="proxy.golang.org:443 sum.golang.org:443"

echo "== 1. each manifest maps to its registry =="
for m in package.json package-lock.json npm-shrinkwrap.json; do
  repo; : > "$TMP/ws/widgets/$m"; check "$m" "$NPM" "$(hosts)"
done
for m in pyproject.toml uv.lock requirements.txt requirements-dev.txt; do
  repo; : > "$TMP/ws/widgets/$m"; check "$m" "$PYPI" "$(hosts)"
done
repo; : > "$TMP/ws/widgets/Cargo.toml"; check "Cargo.toml" "$CRATES" "$(hosts)"
repo; : > "$TMP/ws/widgets/go.mod"; check "go.mod" "$GO" "$(hosts)"

echo "== 2. nested manifests count, vendored/build dirs and deep ones do not =="
repo; mkdir -p "$TMP/ws/widgets/webui" "$TMP/ws/widgets/crates/core"
: > "$TMP/ws/widgets/webui/package.json"; : > "$TMP/ws/widgets/crates/core/Cargo.toml"
check "webui + crate" "crates.io:443 index.crates.io:443 $NPM static.crates.io:443" "$(hosts)"
repo; mkdir -p "$TMP/ws/widgets/node_modules/left-pad" "$TMP/ws/widgets/a/b/c" "$TMP/ws/widgets/.venv"
: > "$TMP/ws/widgets/node_modules/left-pad/package.json"; : > "$TMP/ws/widgets/a/b/c/go.mod"
: > "$TMP/ws/widgets/.venv/pyproject.toml"
check "skipped" "" "$(hosts)"
repo; : > "$TMP/ws/widgets/README.md"; check "no manifest" "" "$(hosts)"
check "missing dir" "" "$(repo_registry_hostports "$TMP/nope")"

reset() {
  unset REPLY_URL CONTEXT_PACK_URL TASK_REPO_URL WORKER_NET_ALLOW TASK_PAYLOAD_JSON CCTUI_URL \
        CCTUI_SERVER_URL ANTHROPIC_BASE_URL OPENAI_BASE_URL FIREWORKS_BASE_URL GOOGLE_GEMINI_BASE_URL \
        WORKER_REPO_REGISTRIES
  CCTUI_BASE_URL="http://cctui.cctui.svc:8700"
  TASK_REPO=widgets
  repo
  : > "$POLICY_FILE"
}
allowed() { jq -r '.allowed_hosts | sort | join(" ")' "$POLICY_FILE"; }

echo "== 3. the phase folds them into the seeded policy =="
reset
TASK_REPO_URL="https://github.com/acme/widgets"
: > "$TMP/ws/widgets/package.json"
seed_policy
phase_repo_registries
check "allowed_hosts" "cctui.cctui.svc:8700 github.com:443 $NPM" "$(allowed)"
check "default deny" "deny" "$(jq -r .default "$POLICY_FILE")"

echo "== 4. no manifest adds nothing =="
reset
seed_policy
before=$(cat "$POLICY_FILE")
phase_repo_registries
check "policy unchanged" "$before" "$(cat "$POLICY_FILE")"

echo "== 5. WORKER_REPO_REGISTRIES off adds nothing =="
for v in 0 false off no; do
  reset
  : > "$TMP/ws/widgets/go.mod"
  WORKER_REPO_REGISTRIES=$v
  seed_policy
  phase_repo_registries
  check "WORKER_REPO_REGISTRIES=$v" "cctui.cctui.svc:8700" "$(allowed)"
done
reset
: > "$TMP/ws/widgets/go.mod"
WORKER_REPO_REGISTRIES=1
seed_policy
phase_repo_registries
check "WORKER_REPO_REGISTRIES=1" "cctui.cctui.svc:8700 $GO" "$(allowed)"

echo "== 6. deduped against WORKER_NET_ALLOW =="
reset
: > "$TMP/ws/widgets/package.json"
WORKER_NET_ALLOW="registry.npmjs.org:443, yt.example:443"
seed_policy
phase_repo_registries
check "allowed_hosts" "cctui.cctui.svc:8700 $NPM yt.example:443" "$(allowed)"
check "no duplicate" "1" "$(jq '[.allowed_hosts[] | select(. == "registry.npmjs.org:443")] | length' "$POLICY_FILE")"

echo "== 7. no seeded policy, no write =="
reset
rm -f "$POLICY_FILE"
: > "$TMP/ws/widgets/package.json"
phase_repo_registries
check "policy absent" "absent" "$([ -e "$POLICY_FILE" ] && echo present || echo absent)"

exit $fail
