#!/usr/bin/env bash
# Drives the real gen-bindings.sh against a throwaway repo tree with a fake
# `cargo` on PATH, so the destructive paths run without touching the checkout.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/gen-bindings.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

mk_repo() {
  local root="$tmp/$1"
  rm -rf "$root"
  mkdir -p "$root/webui/scripts" "$root/webui/src/lib/bindings"
  cp "$script" "$root/webui/scripts/gen-bindings.sh"
  touch "$root/Cargo.toml"
  for i in $(seq 1 20); do
    echo "export type Old$i = { n: bigint };" > "$root/webui/src/lib/bindings/Old$i.ts"
  done
  echo "$root"
}

mk_cargo() {
  local root="$1" body="$2"
  mkdir -p "$root/bin"
  { echo '#!/usr/bin/env bash'; echo "$body"; } > "$root/bin/cargo"
  chmod +x "$root/bin/cargo"
}

surface() { ls "$1/webui/src/lib/bindings" | sort; }

run() {
  local root="$1"
  PATH="$root/bin:$PATH" bash "$root/webui/scripts/gen-bindings.sh" >"$tmp/out" 2>&1 \
    && code=0 || code=$?
}

root="$(mk_repo fail)"
mk_cargo "$root" 'echo "error: could not acquire build lock" >&2; exit 101'
expected="$(surface "$root")"
run "$root"; out="$(cat "$tmp/out")"
check "failing cargo exits non-zero" test "$code" -ne 0
check "failing cargo leaves the binding surface untouched" \
  test "$(surface "$root")" = "$expected"
check "failure is announced loudly" \
  bash -c 'grep -q "gen-bindings.sh FAILED" <<< "$1"' _ "$out"
check "no staging dir is left behind" \
  bash -c '! ls "$1"/webui/src/lib/.bindings.staging.* >/dev/null 2>&1' _ "$root"

root="$(mk_repo killed)"
mk_cargo "$root" '
mkdir -p "$TS_RS_EXPORT_DIR"
echo "export type Half = {};" > "$TS_RS_EXPORT_DIR/Half.ts"
kill -TERM $$
sleep 5'
expected="$(surface "$root")"
run "$root"
check "killed cargo exits non-zero" test "$code" -ne 0
check "killed cargo leaves the binding surface untouched" \
  test "$(surface "$root")" = "$expected"

root="$(mk_repo piped)"
mk_cargo "$root" 'exit 101'
PATH="$root/bin:$PATH" bash -c \
  'bash "$0"/webui/scripts/gen-bindings.sh 2>&1 | tail -5; exit ${PIPESTATUS[0]}' "$root" \
  >"$tmp/piped.out" 2>&1 && code=0 || code=$?
check "piped run still reports non-zero via PIPESTATUS" test "$code" -ne 0
check "the banner is visible in the last 5 lines" grep -q "FAILED" "$tmp/piped.out"

root="$(mk_repo shrink)"
mk_cargo "$root" '
mkdir -p "$TS_RS_EXPORT_DIR"
echo "export type Only = {};" > "$TS_RS_EXPORT_DIR/Only.ts"'
expected="$(surface "$root")"
run "$root"
check "a collapsed surface is refused" test "$code" -ne 0
check "a collapsed surface leaves the old files in place" \
  test "$(surface "$root")" = "$expected"
CCTUI_BINDINGS_ALLOW_SHRINK=1 run "$root"
check "ALLOW_SHRINK=1 lets the swap through" test "$code" -eq 0
check "ALLOW_SHRINK=1 swapped in the new surface" \
  test "$(surface "$root")" = "$(printf 'Only.ts\nindex.ts\n' | sort)"

root="$(mk_repo sweep)"
mk_cargo "$root" '
mkdir -p "$TS_RS_EXPORT_DIR"
for i in $(seq 1 20); do echo "export type New$i = {};" > "$TS_RS_EXPORT_DIR/New$i.ts"; done'
mkdir -p "$root/webui/src/lib/.bindings.staging.99999"
touch "$root/webui/src/lib/.bindings.staging.99999/leftover.ts"
run "$root"
check "a run succeeds with a leaked staging dir present" test "$code" -eq 0
check "a leaked staging dir is swept" \
  bash -c '! ls -d "$1"/webui/src/lib/.bindings.staging.* >/dev/null 2>&1' _ "$root"

root="$(mk_repo ok)"
mk_cargo "$root" '
mkdir -p "$TS_RS_EXPORT_DIR"
for i in $(seq 1 20); do
  echo "export type New$i = { n: bigint };" > "$TS_RS_EXPORT_DIR/New$i.ts"
done'
run "$root"
check "a good run exits zero" test "$code" -eq 0
check "a good run replaces the surface" \
  test "$(ls "$root/webui/src/lib/bindings" | grep -c '^New')" -eq 20
check "a good run drops the stale files" \
  bash -c '! ls "$1"/webui/src/lib/bindings/Old1.ts >/dev/null 2>&1' _ "$root"
check "bigint is normalized to number" \
  bash -c '! grep -rq bigint "$1"/webui/src/lib/bindings' _ "$root"
check "the barrel re-exports every type" \
  bash -c 'test "$(grep -c "^export type \*" "$1"/webui/src/lib/bindings/index.ts)" -eq 20' _ "$root"
check "the barrel does not re-export itself" \
  bash -c '! grep -q "from ./index" "$1"/webui/src/lib/bindings/index.ts' _ "$root"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
