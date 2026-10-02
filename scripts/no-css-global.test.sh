#!/usr/bin/env bash
# Runs the real no-css-global.sh against a scratch git repo.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/no-css-global.sh"

# Inherited git env would point `git init`/`git config` below at the caller's
# real repository instead of the scratch one.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

G=':global('

cd "$tmp"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
# An inherited hooksPath (lefthook) would print on every scratch commit.
git config core.hooksPath /dev/null
mkdir -p webui/src
write() {
  mkdir -p "$(dirname "$1")"
  printf '%s\n' "$2" > "$1"
}

write webui/src/Legacy.svelte "<style>${G}.card) { color: red; }</style>"
write webui/src/Clean.svelte '<style>.card { color: red; }</style>'
git add -A
git commit -qm base

git checkout -qb feature
write webui/src/Clean.svelte '<style>.card { color: blue; }</style>'
git add -A
git commit -qm 'clean change'

check "range mode ignores pre-existing occurrences" \
  bash -c '"$1" main...feature >/dev/null 2>&1' _ "$guard"
check "absolute mode still sees pre-existing occurrences" \
  bash -c '! "$1" --all >/dev/null 2>&1' _ "$guard"

write webui/src/New.svelte "<style>${G}.a) { color: red; }</style>"
git add -A
git commit -qm 'adds one global'
check "range mode fails on an added occurrence" \
  bash -c '! "$1" main...feature >/dev/null 2>&1' _ "$guard"
check "range mode reports the added count only" \
  bash -c '"$1" main...feature 2>&1 | grep -q "added count is 1"' _ "$guard"

drop() {
  git rm -q "$1"
  git commit -qm "drops $1"
}

drop webui/src/New.svelte
write webui/src/Two.svelte "<style>${G}.a) span ${G}.b) { color: red; }</style>"
git add -A
git commit -qm 'adds two globals on one line'
check "multiple occurrences on one added line all count" \
  bash -c '"$1" main...feature 2>&1 | grep -q "added count is 2"' _ "$guard"

drop webui/src/Two.svelte
check "removing an occurrence does not fail the range" \
  bash -c '"$1" main...feature >/dev/null 2>&1' _ "$guard"

write webui/src/Legacy.svelte '<style>.card { color: red; }</style>'
git add -A
git commit -qm 'clears the legacy offender'
check "a deleted occurrence is not counted as added" \
  bash -c '"$1" main...feature >/dev/null 2>&1' _ "$guard"
check "absolute mode reports being under budget once cleared" \
  bash -c '"$1" --all 2>&1 | grep -q "at budget"' _ "$guard"

write webui/src/Staged.svelte "<style>${G}.a) { color: red; }</style>"
git add -A
check "staged mode fails on a staged added occurrence" \
  bash -c '! "$1" --staged >/dev/null 2>&1' _ "$guard"
git reset -q --hard
git clean -qfd
check "staged mode passes with nothing staged" \
  bash -c '"$1" --staged >/dev/null 2>&1' _ "$guard"

write server/src/lib.rs "${G}"
git add -A
check "non-webui files are ignored" bash -c '"$1" --staged >/dev/null 2>&1' _ "$guard"
git reset -q --hard
git clean -qfd

check "no argument prints the usage and exits 2" bash -c '
  out=$("$1" 2>&1); rc=$?
  [ "$rc" = 2 ] || exit 1
  case "$out" in *"<git-diff-range>"*"--staged"*"--all"*) ;; *) exit 1 ;; esac
' _ "$guard"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
