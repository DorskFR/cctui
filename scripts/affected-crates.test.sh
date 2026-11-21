#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR CCTUI_CARGO_METADATA
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

repo="$tmp/repo"
mkdir -p "$repo/scripts"
cp "$here/affected-crates.sh" "$here/ci-changes.sh" "$repo/scripts/"
cd "$repo"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
git config core.hooksPath /dev/null

write() {
  mkdir -p "$(dirname "$1")"
  printf '%s\n' "$2" >"$1"
}

pkg() {
  local name="$1" deps="$2" out="[" d kind
  for d in $deps; do
    kind=null
    case "$d" in *:dev) kind='"dev"' d="${d%:dev}" ;; esac
    out+="{\"name\":\"$d\",\"kind\":$kind,\"path\":\"$repo/crates/$d\"},"
  done
  out="${out%,}]"
  printf '{"name":"%s","manifest_path":"%s/crates/%s/Cargo.toml","dependencies":%s}' "$name" "$repo" "$name" "$out"
}
printf '{"workspace_root":"%s","packages":[%s,%s,%s,%s,%s,%s,%s]}' "$repo" \
  "$(pkg cctui-proto "")" "$(pkg cctui-crypto "")" "$(pkg core "cctui-proto cctui-crypto")" \
  "$(pkg server "core")" "$(pkg tui "cctui-proto")" "$(pkg leaf "")" "$(pkg tools "core:dev")" \
  >"$tmp/metadata.json"
export CCTUI_CARGO_METADATA="$tmp/metadata.json"

write Cargo.toml '[workspace.package]
version = "0.1.0"'
for c in cctui-proto cctui-crypto core server tui leaf tools; do
  write "crates/$c/Cargo.toml" "[package]
name = \"$c\""
  write "crates/$c/src/lib.rs" '// a'
done
write webui/src/app.ts 'a'
git add -A
git commit -qm base
base="$(git rev-parse HEAD)"

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

affected() {
  git checkout -q --detach "$base"
  "$@"
  git add -A
  git commit -qm change --allow-empty
  bash scripts/affected-crates.sh "$base" HEAD | tr '\n' ' '
}

everything="cctui-crypto cctui-proto core leaf server tools tui "
check "a leaf change is only the leaf" "leaf " "$(affected write crates/leaf/src/lib.rs '// b')"
check "a shared crate pulls in its dependents, dev ones too" "core server tools " \
  "$(affected write crates/core/src/lib.rs '// b')"
check "dependents are followed transitively" "cctui-proto core server tools tui " \
  "$(affected write crates/cctui-proto/src/lib.rs '// b')"
check "a crate manifest belongs to its crate" "server " "$(affected write crates/server/Cargo.toml '[package]')"
check "a webui change affects no crate" "" "$(affected write webui/src/app.ts 'b')"
check "a version-only workspace bump affects no crate" "" \
  "$(affected write Cargo.toml '[workspace.package]
version = "0.2.0"')"
check "a workspace manifest change affects every crate" "$everything" \
  "$(affected write Cargo.toml '[workspace.dependencies]')"
check "a Rust input outside the crates affects every crate" "$everything" \
  "$(affected write migrations/0001.sql 'select 1;')"
check "a committed binding belongs to cctui-proto" "cctui-proto core server tools tui " \
  "$(affected write webui/src/lib/bindings/A.ts 'export type A = 1;')"
check "the scrub detector copy belongs to cctui-crypto" "cctui-crypto core server tools " \
  "$(affected write webui/src/lib/scrubDetectors.generated.ts 'x')"
check "--all lists every crate" "$everything" "$(bash scripts/affected-crates.sh --all | tr '\n' ' ')"

stub="$tmp/stub"
mkdir -p "$stub"
log="$tmp/cargo.log"
for c in core server tui; do
  printf '#!/usr/bin/env bash\necho "%s ran in $PWD manifest=$CARGO_MANIFEST_DIR args=$*" >>"%s"\n' "$c" "$log" >"$tmp/$c-test"
done
printf '#!/usr/bin/env bash\necho "server ran"  >>"%s"\nexit 1\n' "$log" >"$tmp/server-test"
chmod +x "$tmp"/*-test
cat >"$stub/cargo" <<EOF
#!/usr/bin/env bash
echo "cargo \$*" >>"$log"
case " \$* " in *" --no-run "*) ;; *) exit 0 ;; esac
for c in core server tui; do
  printf '{"reason":"compiler-artifact","manifest_path":"$repo/crates/%s/Cargo.toml","profile":{"test":true},"executable":"$tmp/%s-test"}\n' "\$c" "\$c"
  printf '{"reason":"compiler-artifact","manifest_path":"$repo/crates/%s/Cargo.toml","profile":{"test":false},"executable":null}\n' "\$c"
done
echo '{"reason":"build-finished","success":true}'
EOF
chmod +x "$stub/cargo"

run_tests() { : >"$log"; PATH="$stub:$PATH" bash scripts/affected-crates.sh test "$@" >/dev/null 2>&1 && echo 0 || echo $?; }

check "tests of the named packages pass" 0 "$(run_tests core tui -- --locked)"
check "the build covers the workspace with the caller's args" "cargo test --workspace --locked --no-run --message-format=json-render-diagnostics" \
  "$(head -n1 "$log")"
check "only the named packages' binaries run, from their crate dir" \
  "core ran in $repo/crates/core manifest=$repo/crates/core args=
tui ran in $repo/crates/tui manifest=$repo/crates/tui args=" "$(grep ' ran' "$log")"
check "a failing binary fails the run" 101 "$(run_tests server tui)"
check "the binaries after a failure still run" "server ran
tui ran in $repo/crates/tui manifest=$repo/crates/tui args=" "$(grep ' ran' "$log")"
check "every package is a plain workspace run" 0 "$(run_tests tools leaf tui server core cctui-proto cctui-crypto -- --locked)"
check "the plain run passes the caller's args" "cargo test --workspace --locked --" "$(cat "$log")"
check "binary args reach every binary" "core ran in $repo/crates/core manifest=$repo/crates/core args=--ignored a::b" \
  "$(run_tests core -- --locked --lib -- --ignored a::b >/dev/null; grep ' ran' "$log")"
check "cargo args shape the build" "cargo test --workspace --locked --lib --no-run --message-format=json-render-diagnostics" \
  "$(head -n1 "$log")"
check "binary args reach a plain workspace run" "cargo test --workspace --locked -- --ignored" \
  "$(run_tests tools leaf tui server core cctui-proto cctui-crypto -- --locked -- --ignored >/dev/null; cat "$log")"
check "no package runs nothing" 0 "$(run_tests)"
check "no package calls no cargo" "" "$(cat "$log")"

if [ "$failures" -gt 0 ]; then
  echo "$failures failure(s)"
  exit 1
fi
echo "all affected-crates cases passed"
