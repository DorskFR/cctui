#!/usr/bin/env bash
# shellcheck disable=SC2016  # bash -c bodies expand their own positional args
# Runs the real check-forbidden-terms.sh against a scratch git repo and fake terms.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/check-forbidden-terms.sh"

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

terms="$tmp/forbidden.txt"
cat >"$terms" <<'EOF'
# fake terms
widgetcorp
lit:re:literal
re:(?-i)\bGadgetX\b
re:\bacct-\d{4}\b
EOF
export CCTUI_FORBIDDEN_TERMS_FILE="$terms"

out="$tmp/out"
run() { "$guard" "$@" >"$out" 2>&1; }
passes() { run "$@"; }
fails() { local rc=0; run "$@" || rc=$?; [ "$rc" -eq 1 ]; }
says() { grep -qF -- "$1" "$out"; }
no_leak() { ! grep -qiE 'widgetcorp|gadgetx|acct-[0-9]{4}|re:literal' "$out"; }

repo="$tmp/repo"
mkdir "$repo"
cd "$repo"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
printf 'fn main() {}\n' >a.rs
git add -A
git commit -qm base

stage() {
  git reset -q --hard
  git clean -qfd
  mkdir -p "$(dirname "$1")"
  printf '%s\n' "$2" >"$1"
  git add "$1"
}

stage a.rs 'fn main() { println!("hi"); }'
check "clean staged diff passes" passes --staged
stage b.rs $'one\ntwo WidgetCorp three'
check "literal term fails case-insensitively" fails --staged
check "hit is named by path:line" says "b.rs:2"
check "hit output does not leak the term" no_leak
stage b.rs 'x = GadgetX'
check "case-sensitive regex fails on its case" fails --staged
stage b.rs 'x = gadgetx'
check "case-sensitive regex passes another case" passes --staged
stage b.rs 'id ACCT-1234'
check "regex term fails" fails --staged
check "regex hit does not leak" no_leak
stage b.rs 'id acct-12345'
check "regex word boundary holds" passes --staged
stage b.rs 'x RE:Literal y'
check "lit: prefix is a literal" fails --staged
stage docs/widgetcorp-notes.md 'neutral text'
check "forbidden path fails" fails --staged
check "forbidden path is withheld" says "location withheld"
check "forbidden path does not leak" no_leak
git reset -q --hard
git clean -qfd
printf 'old widgetcorp\n' >legacy.txt
git add legacy.txt
git commit -qm legacy
git rm -q legacy.txt
check "deleting a line with a term passes" passes --staged
git commit -qm 'drop legacy'

check "absent pattern file passes" passes --staged --patterns "$tmp/missing.txt"
check "absent pattern file says so" says "no pattern file"
CCTUI_FORBIDDEN_TERMS_FILE="$tmp/missing.txt" run --staged
check "absent file via env says so" says "no pattern file"

printf 're:(unclosed\n' >"$tmp/bad.txt"
stage b.rs 'anything'
bad_rc=0
"$guard" --patterns "$tmp/bad.txt" --staged >"$out" 2>&1 || bad_rc=$?
check "invalid regex is an error" test "$bad_rc" -eq 2
check "invalid regex names its line only" says "line 1"
check "invalid regex is not echoed" bash -c '! grep -qF unclosed "$1"' _ "$out"
git reset -q --hard
git clean -qfd

msg="$tmp/msg"
printf 'feat: add thing\n\nfor widgetcorp\n' >"$msg"
check "commit message hit fails" fails --message "$msg"
check "message hit is named by line" says "commit-message:3"
check "message hit does not leak" no_leak
printf 'feat: add thing\n# widgetcorp in a git comment\n' >"$msg"
check "git comment lines are ignored" passes --message "$msg"
printf 'feat: thing\n# ------------------------ >8 ------------------------\n+widgetcorp\n' >"$msg"
check "verbose diff below the scissors is ignored" passes --message "$msg"
printf 'feat: thing\n' >"$msg"
check "clean message passes" passes --message "$msg"

git checkout -qb feature
printf 'acct-9876\n' >c.txt
git add c.txt
git commit -qm 'add c'
git rm -q c.txt
git commit -qm 'drop c'
check "range catches a term added then removed" fails --range main..feature
check "range hit names the commit" says "$(git rev-parse --short=12 HEAD~1):c.txt:1"
check "range hit does not leak" no_leak
check "three-dot range works too" fails --range main...feature
check "range defaults its tip to HEAD" fails --range main..
check "range ignores the base's own history" passes --range feature..main
git checkout -q main
git checkout -qb feature2
git commit -q --allow-empty -m 'chore: tidy for WidgetCorp'
check "range checks commit messages" fails --range main..feature2
check "range message hit names the commit" says "$(git rev-parse --short=12 HEAD):commit-message:1"
check "range message hit does not leak" no_leak
check "bad range is an error" bash -c '"$1" --range nonsense >/dev/null 2>&1; [ $? -eq 2 ]' _ "$guard"

git checkout -q main
git checkout -qb merged
printf 'one\n' >m.txt
git add m.txt
git commit -qm 'add m'
git checkout -q main
printf 'two\n' >m.txt
git add m.txt
git commit -qm 'other m'
git checkout -q merged
git merge -q main -m 'merge main' >/dev/null 2>&1 || true
printf 'resolved widgetcorp\n' >m.txt
git add m.txt
git commit -qm 'merge main'
check "a term in a merge resolution fails" fails --range main..merged
check "merge hit does not leak" no_leak

zero=0000000000000000000000000000000000000000
push_new() { printf 'refs/heads/%s %s refs/heads/%s %s\n' "$1" "$(git rev-parse "$1")" "$1" "$zero" | run --pre-push; }
push_update() { printf 'refs/heads/%s %s refs/heads/%s %s\n' "$1" "$(git rev-parse "$1")" "$1" "$(git rev-parse "$2")" | run --pre-push; }
check "pre-push of a new branch with a hit fails" bash -c "$(declare -f run push_new); guard='$guard' out='$out' zero=$zero; ! push_new feature2"
check "pre-push hit does not leak" no_leak
check "pre-push of a clean update passes" push_update main main
check "pre-push of a dirty update fails" bash -c "$(declare -f run push_update); guard='$guard' out='$out'; ! push_update feature main"
check "pre-push of a deletion passes" bash -c "printf 'refs/heads/x %s refs/heads/x %s\n' $zero $zero | '$guard' --pre-push >/dev/null"

printf 'widgetcorp\n\n' >"$tmp/terms.txt"
printf '(?-i)\\bGadgetX\\b\n' >"$tmp/regexes.txt"
built="$tmp/cfg/built.txt"
check "build writes a pattern file" passes --build "$built" "$tmp/terms.txt" "$tmp/regexes.txt"
check "build output does not leak" no_leak
check "built file is private" test "$(stat -c %a "$built")" = 600
check "built file has both kinds" test "$(grep -c -v '^#' "$built")" -eq 2
git checkout -q main
stage b.rs 'WIDGETCORP and GadgetX'
check "built file catches terms" fails --staged --patterns "$built"
printf '(broken\n' >"$tmp/regexes.txt"
check "build rejects an invalid regex" bash -c '! "$1" --build "$2" "$3" "$4" >/dev/null 2>&1 && [ ! -e "$2" ]' _ "$guard" "$tmp/cfg/bad.txt" "$tmp/terms.txt" "$tmp/regexes.txt"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
