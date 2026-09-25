#!/usr/bin/env bash
# Runs the real check-comment-refs.sh against a scratch git repo.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/check-comment-refs.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

cd "$tmp"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
printf 'fn main() {}\n' > a.rs
printf '%s\n' '// legacy note CCT-1' > old.ts
git add -A
git commit -qm base

stage() {
  git reset -q --hard
  git clean -qfd
  printf '%s\n' "$2" > "$1"
  git add "$1"
}
passes() { stage "$1" "$2"; "$guard" >/dev/null 2>&1; }
fails() { stage "$1" "$2"; ! "$guard" >/dev/null 2>&1; }

T='CCT-''42'
check "clean staged diff passes" passes a.rs 'fn main() { println!("hi"); }'
check "ticket in a rust comment fails" fails a.rs "// see $T"
check "ticket in a ts block comment fails" fails b.ts "/* $T */"
check "ticket in a jsdoc line fails" fails b.ts "   * fixes $T"
check "ticket in an html comment fails" fails c.svelte "<!-- $T -->"
check "ticket in a shell comment fails" fails d.sh "# $T"
check "ticket in a yaml comment fails" fails e.yml "  # $T"
check "ticket in a Makefile comment fails" fails Makefile "# $T"
check "wave note fails" fails a.rs '// added in Wave 12'
check "lane note fails" fails a.rs '// lane W3 owns this'
check "ticket in a describe name fails" fails f.test.ts "describe('$T drawer', () => {});"
check "ticket in an it name fails" fails f.test.ts "  it(\"$T keeps order\", () => {});"
check "ticket in a test.each name fails" fails f.test.ts "test.each([1])('$T %s', () => {});"
check "ticket in a fixture string passes" passes f.test.ts "const id = '$T';"
check "ticket in a rust string passes" passes a.rs "let s = \"$T\";"
check "unlisted file types are ignored" passes notes.md "// $T"
check "a pre-existing comment is not re-flagged" passes a.rs 'fn other() {}'

stage g.rs "// $T"
check "offending line is named in the output" bash -c '"$1" 2>&1 | grep -q "g.rs:1:"' _ "$guard"

git reset -q --hard
git clean -qfd
git checkout -qb feature
printf '%s\n' "// $T" > h.ts
git add h.ts
git commit -qm feature
check "base-ref mode checks committed diff" bash -c '! "$1" main >/dev/null 2>&1' _ "$guard"
check "base-ref mode ignores the base's own history" bash -c '"$1" HEAD >/dev/null 2>&1' _ "$guard"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
