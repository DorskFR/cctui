#!/usr/bin/env bash
# Exercises the REAL phase_codex_config from deploy/worker-entrypoint.sh against
# a block rendered by cctui_proto::codex_config, so the shell writer and the
# daemon's `-c` emitter cannot drift apart. The function is extracted rather
# than sourced: the entrypoint execs a supervisor at the bottom.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
entrypoint="$here/../deploy/worker-entrypoint.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

# The marker constants and the function body, lifted verbatim from the real file.
{
  grep -E '^CODEX_MARKER_(BEGIN|END)=' "$entrypoint"
  awk '/^phase_codex_config\(\) \{/{f=1} f{print} f&&/^\}/{exit}' "$entrypoint"
} > "$tmp/fn.sh"
grep -q 'CCTUI_CODEX_CONFIG_TOML' "$tmp/fn.sh" || {
  echo "FAIL — extraction did not capture the config-block splice"; exit 1; }

run_phase() {
  ( set -eu
    # shellcheck disable=SC1091
    log() { :; }
    adapter_is_codex() { return 1; }
    WORKER_USER=worker WORKER_UID=1000
    CODEX_HOME="$tmp/codex"
    OPENAI_API_KEY=tok OPENAI_BASE_URL=https://gw/openai
    export CODEX_HOME
    . "$tmp/fn.sh"
    phase_codex_config )
}

# The block a real render produces (see the round-trip test in codex_config.rs).
export CCTUI_CODEX_CONFIG_TOML='history.persistence = "none"
model_context_window = 272000
web_search = true'

cfg="$tmp/codex/config.toml"
run_phase
# Written twice: the marker merge must be idempotent, not accumulate.
run_phase

toml_get() { python3 -c '
import sys,tomllib
d=tomllib.load(open(sys.argv[1],"rb"))
for p in sys.argv[2].split("."):
    d=d[p]
print(repr(d))' "$cfg" "$1"; }

check "config.toml parses as TOML" python3 -c 'import sys,tomllib;tomllib.load(open(sys.argv[1],"rb"))' "$cfg"
check "account string reaches the file" test "$(toml_get history.persistence)" = "'none'"
check "account integer stays an integer" test "$(toml_get model_context_window)" = "272000"
check "account boolean stays a boolean" test "$(toml_get web_search)" = "True"
check "managed model_provider still wins" test "$(toml_get model_provider)" = "'cctui'"
check "managed gateway block survives" test "$(toml_get model_providers.cctui.env_key)" = "'OPENAI_API_KEY'"
check "account keys sit above the gateway table" \
  test "$(grep -n 'web_search' "$cfg" | cut -d: -f1)" -lt "$(grep -n '\[model_providers.cctui\]' "$cfg" | cut -d: -f1)"
check "re-running does not duplicate an account key" \
  test "$(grep -c '^web_search = ' "$cfg")" -eq 1

# With no block set, the file is exactly what it was before this change.
unset CCTUI_CODEX_CONFIG_TOML
rm -f "$cfg"
run_phase
check "no block set leaves no account keys" bash -c '! grep -q "^web_search" "$1"' _ "$cfg"
check "no block set still writes the gateway wiring" test "$(toml_get model_provider)" = "'cctui'"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
