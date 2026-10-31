#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/heavy.sh"
bash_bin="$(command -v bash)"

unset CI CCTUI_HEAVY CCTUI_HEAVY_MEM CCTUI_HEAVY_JOBS CCTUI_HEAVY_VITEST_WORKERS CARGO_BUILD_JOBS
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export CCTUI_HEAVY_LOCK="$tmp/heavy.lock"

bin="$tmp/bin"
mkdir -p "$bin"
for tool in bash sh env flock cat sleep touch rm id nproc getconf true false; do
  if p="$(type -P "$tool")"; then ln -s "$p" "$bin/$tool"; fi
done

stub="$tmp/stub"
mkdir -p "$stub"
cat >"$stub/systemd-run" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$SYSTEMD_RUN_LOG"
last=""
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do last="$1"; shift; done
if [ "$#" -gt 0 ]; then shift; exec "$@"; fi
exec "$last"
EOF
cat >"$stub/vitest" <<'EOF'
#!/usr/bin/env bash
echo "$*"
EOF
chmod +x "$stub/systemd-run" "$stub/vitest"
export SYSTEMD_RUN_LOG="$tmp/systemd-run.log"

heavy() { PATH="$bin" "$bash_bin" "$script" "$@"; }
heavy_stubbed() { PATH="$stub:$bin" "$bash_bin" "$script" "$@"; }

failures=0
check() {
  local name="$1" want="$2" got="$3"
  if [ "$got" = "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name"
    diff <(echo "$want") <(echo "$got") | sed 's/^/       /' || true
    failures=$((failures + 1))
  fi
}
status() { "$@" >/dev/null 2>&1 && echo 0 || echo $?; }

check "systemd-run is absent from the test PATH" 1 "$(PATH="$bin" "$bash_bin" -c 'command -v systemd-run >/dev/null && echo 0 || echo 1')"
check "heavy.sh true exits 0" 0 "$(status heavy true)"
check "heavy.sh false exits 1" 1 "$(status heavy false)"
check "the wrapped exit code is preserved" 7 "$(status heavy sh -c 'exit 7')"
check "no command is a usage error" 2 "$(status heavy)"
check "the child sees CCTUI_HEAVY=1" 1 "$(heavy sh -c 'echo "$CCTUI_HEAVY"')"
check "the holder file is gone after the job" absent "$([ -e "$CCTUI_HEAVY_LOCK.holder" ] && echo present || echo absent)"

check "CARGO_BUILD_JOBS follows CCTUI_HEAVY_JOBS" 3 "$(CCTUI_HEAVY_JOBS=3 heavy sh -c 'echo "$CARGO_BUILD_JOBS"')"
check "an explicit CARGO_BUILD_JOBS wins" 5 "$(CARGO_BUILD_JOBS=5 CCTUI_HEAVY_JOBS=3 heavy sh -c 'echo "$CARGO_BUILD_JOBS"')"
check "CI leaves CARGO_BUILD_JOBS alone" unset "$(CI=true CCTUI_HEAVY_JOBS=3 heavy sh -c 'echo "${CARGO_BUILD_JOBS:-unset}"')"

check "vitest gets --maxWorkers" "run --maxWorkers=2" "$(CCTUI_HEAVY_VITEST_WORKERS=2 PATH="$stub:$bin" "$bash_bin" "$script" vitest run)"
check "--maxWorkers goes before a --" "run --maxWorkers=2 -- x" "$(CCTUI_HEAVY_VITEST_WORKERS=2 PATH="$stub:$bin" "$bash_bin" "$script" vitest run -- x)"
check "an explicit --maxWorkers wins" "run --maxWorkers=5" "$(CCTUI_HEAVY_VITEST_WORKERS=2 PATH="$stub:$bin" "$bash_bin" "$script" vitest run --maxWorkers=5)"
check "nested calls still cap vitest" "run --maxWorkers=2" "$(CCTUI_HEAVY=1 CCTUI_HEAVY_VITEST_WORKERS=2 PATH="$stub:$bin" "$bash_bin" "$script" vitest run)"
check "CI leaves vitest alone" "run" "$(CI=true CCTUI_HEAVY_VITEST_WORKERS=2 PATH="$stub:$bin" "$bash_bin" "$script" vitest run)"

rm -f "$SYSTEMD_RUN_LOG"
check "the scope keeps the exit code" 7 "$(CCTUI_HEAVY_MEM=2G status heavy_stubbed sh -c 'exit 7')"
check "the scope carries the memory ceiling" "--user --scope --quiet -p MemoryMax=2G -- sh -c exit 7" "$(tail -n1 "$SYSTEMD_RUN_LOG")"
rm -f "$SYSTEMD_RUN_LOG"
check "CI runs without a scope" 0 "$(CI=true status heavy_stubbed true)"
check "CI never calls systemd-run" absent "$([ -e "$SYSTEMD_RUN_LOG" ] && echo present || echo absent)"

order="$tmp/order"
: >"$order"
PATH="$bin" "$bash_bin" "$script" sh -c "touch '$tmp/started'; sleep 1; echo first >>'$order'" &
first=$!
for _ in $(seq 50); do [ -e "$tmp/started" ] && break; sleep 0.1; done
heavy sh -c "echo second >>'$order'" 2>"$tmp/queued.err"
wait "$first"
check "a second job waits for the first" "first
second" "$(cat "$order")"
check "the waiter says once who it is queued behind" 1 "$(grep -c "^heavy: queued behind pid $first (sh -c touch " "$tmp/queued.err" || true)"

rm -f "$tmp/started"
: >"$order"
PATH="$bin" "$bash_bin" "$script" sh -c "touch '$tmp/started'; sleep 1; echo outer >>'$order'" &
first=$!
for _ in $(seq 50); do [ -e "$tmp/started" ] && break; sleep 0.1; done
CCTUI_HEAVY=1 heavy sh -c "echo nested >>'$order'" 2>"$tmp/nested.err"
wait "$first"
check "a nested call does not wait on the lock" "nested
outer" "$(cat "$order")"
check "a nested call prints nothing" "" "$(cat "$tmp/nested.err")"

check "the default lock lives in XDG_RUNTIME_DIR" present "$(mkdir -p "$tmp/xdg" && env -u CCTUI_HEAVY_LOCK XDG_RUNTIME_DIR="$tmp/xdg" PATH="$bin" "$bash_bin" "$script" true && [ -e "$tmp/xdg/cctui-heavy.lock" ] && echo present || echo absent)"
check "without XDG_RUNTIME_DIR the lock falls back to TMPDIR" present "$(mkdir -p "$tmp/t" && env -u CCTUI_HEAVY_LOCK -u XDG_RUNTIME_DIR TMPDIR="$tmp/t" PATH="$bin" "$bash_bin" "$script" true && [ -e "$tmp/t/cctui-heavy-$(id -u).lock" ] && echo present || echo absent)"

if [ "$failures" -gt 0 ]; then
  echo "$failures failure(s)"
  exit 1
fi
