#!/usr/bin/env bash
# Runs the real no-native-confirm.sh against a scratch git repo.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/no-native-confirm.sh"

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}
fails() { ! "$@"; }

cd "$tmp"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
git config core.hooksPath /dev/null
mkdir -p webui/src
printf '<script>\n\tlet x = 1;\n</script>\n' > webui/src/A.svelte
git add -A && git commit -qm base

check "clean tree passes --all" bash "$guard" --all

printf '<script>\n\tif (!confirm("sure?")) return;\n</script>\n' > webui/src/B.svelte
git add -A
check "staged bare confirm( fails" fails bash "$guard" --staged
git commit -qm bad
check "committed confirm( fails the range" fails bash "$guard" HEAD~1...HEAD
check "committed confirm( fails --all" fails bash "$guard" --all
git rm -q webui/src/B.svelte && git commit -qm fix

printf '<script>\n\tif (window.confirm("sure?")) go();\n</script>\n' > webui/src/C.svelte
git add -A
check "window.confirm( fails" fails bash "$guard" --staged
git reset -q

printf '<script>\n\tasync function confirm() {}\n\tfunction confirm() {}\n\tthis.#d.confirm(m);\n\tdeps.confirm(m);\n\tconst ok = onconfirm();\n</script>\n<ConfirmModal onconfirm={run} />\n' > webui/src/C.svelte
git add -A
check "methods, definitions and ConfirmModal pass" bash "$guard" --staged

git reset -q
rm -f webui/src/C.svelte

printf 'export const ok = () => window.confirm("sure?");\n' > webui/src/a.ts
git add -A
check "window.confirm( in .ts fails" fails bash "$guard" --staged
git reset -q
printf 'export const ok = () => globalThis.confirm("sure?");\n' > webui/src/a.ts
git add -A
check "globalThis.confirm( in .ts fails" fails bash "$guard" --staged
git reset -q

printf 'class D {\n\tconfirm(m: string) {\n\t\treturn this.#d.confirm(m);\n\t}\n}\nexport const run = (d: D) => d.confirm("x");\n' > webui/src/a.ts
git add -A
check ".ts methods and member calls on other objects pass" bash "$guard" --staged
git commit -qm ts
check "clean .ts passes --all" bash "$guard" --all

mkdir -p webui/src/lib/paraglide
printf 'export const x = () => window.confirm("x");\n' > webui/src/lib/paraglide/gen.ts
git add -A && git commit -qm gen
check "generated paraglide output is ignored" bash "$guard" --all

check "no argument is a usage error" fails bash "$guard"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
