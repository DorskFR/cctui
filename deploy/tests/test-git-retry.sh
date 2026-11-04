#!/usr/bin/env bash
# Exercises git_retry() and phase_context_pack() from deploy/worker-entrypoint.sh
# against a `git` shim that fails transiently. Extracts the functions so the
# entrypoint's main() never runs.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { if grep -qF -- "$2" "$3"; then ok "$1"; else bad "$1: [$2] not in output:"; sed 's/^/       /' "$3"; fi; }

eval "$(awk '/^git_retry\(\)/,/^}/' "$ENTRY")"
eval "$(awk '/^phase_context_pack\(\)/,/^}/' "$ENTRY")"
log() { echo "cctui-worker: $*"; }
is_root() { return 1; }
pack_dirs() { :; }
pack_dir_target() { :; }
pack_wire_instructions() { :; }
adapter_kind() { echo claude; }
WORKER_UID=$(id -u)
WORKER_USER=nobody-test
export TMPDIR="$T"

# Fails the first SHIM_FAIL_<SUBCOMMAND> calls with exit 128, leaving junk behind.
mkdir -p "$T/bin"
cat > "$T/bin/git" <<'EOF'
#!/bin/sh
dir=.
[ "$1" = -C ] && { dir=$2; shift 2; }
sub=$1
n=$(cat "$STATE/$sub" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$STATE/$sub"
eval "fails=\${SHIM_FAIL_$(echo "$sub" | tr a-z A-Z):-0}"
case "$sub" in
  clone)
    for a; do dest=$a; done
    case " $* " in *" --branch "*) [ "${SHIM_NO_BRANCH:-0}" = 1 ] && { echo "fatal: Remote branch not found" >&2; exit 128; } ;; esac
    if [ "$n" -le "$fails" ]; then
      mkdir -p "$dest/.git"; : > "$dest/partial-junk"
      echo "fatal: unable to access: GnuTLS recv error (-110)" >&2; exit 128
    fi
    [ -z "$(ls -A "$dest" 2>/dev/null)" ] || { echo "fatal: destination path '$dest' already exists and is not an empty directory." >&2; exit 128; }
    mkdir -p "$dest/.git"; printf 'PACK\n' > "$dest/AGENTS.md" ;;
  init) [ -z "$(ls -A "$dir")" ] || { echo "dirty dir" >&2; exit 1; }; mkdir -p "$dir/.git" ;;
  fetch)
    if [ "$n" -le "$fails" ]; then : > "$dir/partial-junk"; echo "fatal: GnuTLS recv error (-110)" >&2; exit 128; fi ;;
  checkout) printf 'PACK\n' > "$dir/AGENTS.md" ;;
esac
exit 0
EOF
chmod +x "$T/bin/git"
PATH="$T/bin:$PATH"
export STATE="$T/state"

setup() {
  rm -rf "$STATE" "$T/ctx"; mkdir -p "$STATE"
  CONTEXT_DIR="$T/ctx"
  unset SHIM_FAIL_CLONE SHIM_FAIL_FETCH SHIM_NO_BRANCH CONTEXT_PACK_REF CONTEXT_PACK_SUBDIR TASK_PAYLOAD_JSON
  CONTEXT_PACK_URL=https://example.invalid/packs
  export CCTUI_GIT_RETRY_DELAYS="0 0 0"
}
run() { ( phase_context_pack && echo SURVIVED ) > "$T/out" 2>&1; echo $?; }
calls() { cat "$STATE/$1" 2>/dev/null || echo 0; }

echo "== 1. clone fails once (exit 128), then succeeds =="
setup; export SHIM_FAIL_CLONE=1
check "exit status"       0        "$(run)"
has   "entrypoint survived" SURVIVED "$T/out"
check "pack installed"    PACK     "$(cat "$CONTEXT_DIR/AGENTS.md" 2>/dev/null)"
check "partial clone wiped" absent "$([ -e "$CONTEXT_DIR/partial-junk" ] && echo present || echo absent)"
check "clone attempts"    2        "$(calls clone)"
has   "retry logged"      "context pack: fetch failed (attempt 1), retrying in 0s" "$T/out"

echo "== 2. clone always fails -> fatal after 4 attempts =="
setup; export SHIM_FAIL_CLONE=99
check "exit status"       1        "$(run)"
check "clone attempts"    4        "$(calls clone)"
has   "fatal message"     "FATAL context-pack clone failed (default branch)" "$T/out"
has   "git error surfaced" "GnuTLS recv error" "$T/out"
check "no survival" absent "$(grep -q SURVIVED "$T/out" && echo present || echo absent)"

echo "== 3. SHA ref: fetch fails once, then succeeds =="
setup; export SHIM_NO_BRANCH=1 SHIM_FAIL_FETCH=1; CONTEXT_PACK_REF=0123abcd
check "exit status"       0        "$(run)"
check "pack installed"    PACK     "$(cat "$CONTEXT_DIR/AGENTS.md" 2>/dev/null)"
check "fetch attempts"    2        "$(calls fetch)"

echo "== 4. SHA ref: fetch always fails -> fatal =="
setup; export SHIM_NO_BRANCH=1 SHIM_FAIL_FETCH=99; CONTEXT_PACK_REF=0123abcd
check "exit status"       1        "$(run)"
check "fetch attempts"    4        "$(calls fetch)"
has   "fatal message"     "FATAL context-pack fetch failed (CONTEXT_PACK_URL set, ref=0123abcd)" "$T/out"

echo "== 5. default backoff is 2s, 5s, 10s =="
sleep() { echo "$1" >> "$T/sleeps"; }
rm -f "$T/sleeps"; unset CCTUI_GIT_RETRY_DELAYS
git_retry "probe" false > /dev/null
check "delays" "2 5 10" "$(tr '\n' ' ' < "$T/sleeps" | sed 's/ $//')"

echo "== 6. empty delay list means a single attempt =="
rm -f "$T/sleeps"; n=0
count() { n=$((n + 1)); return 1; }
CCTUI_GIT_RETRY_DELAYS="" git_retry "probe" count > /dev/null
check "attempts" 1 "$n"
check "no sleep" absent "$([ -e "$T/sleeps" ] && echo present || echo absent)"
unset -f sleep

echo
[ "$fail" = 0 ] && echo "ALL PASS" || echo "FAILURES"
exit "$fail"
