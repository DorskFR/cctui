#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/ci-changes.sh"

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$tmp"
git init -q -b main
git config user.email t@example.com
git config user.name t
git config commit.gpgsign false
git config core.hooksPath /dev/null

write() {
  mkdir -p "$(dirname "$1")"
  printf '%s\n' "$2" > "$1"
}

manifest() {
  write Cargo.toml "[workspace]
members = [\"crates/cctui-a\"]

[workspace.package]
version = \"$1\"

[workspace.dependencies]
serde = { version = \"$2\" }"
}

lock() {
  write Cargo.lock "[[package]]
name = \"cctui-a\"
version = \"$1\"
dependencies = [
 \"serde\",
]

[[package]]
name = \"serde\"
version = \"$2\""
}

npm_lock() {
  write webui/package-lock.json "{
  \"name\": \"cctui-webui\",
  \"version\": \"$1\",
  \"lockfileVersion\": 3,
  \"packages\": {
    \"\": { \"name\": \"cctui-webui\", \"version\": \"$1\" },
    \"node_modules/vite\": { \"version\": \"$2\" }
  }
}"
}

manifest 0.1.0-beta.1 1.0.0
lock 0.1.0-beta.1 1.0.0
write crates/cctui-a/src/lib.rs 'pub fn a() {}'
write webui/src/app.ts 'export const a = 1;'
npm_lock 0.1.0-beta.1 1.0.0
git add -A
git commit -qm base
base="$(git rev-parse HEAD)"

failures=0
change() {
  local name="$1"; shift
  local want=()
  while [ "$1" != "--" ]; do want+=("$1"); shift; done
  shift
  git checkout -q --detach "$base"
  "$@"
  git add -A
  git commit -qm change --allow-empty
  local out ok=1
  out="$(bash "$script" "$base" HEAD)"
  for kv in "${want[@]}"; do
    grep -qx -- "$kv" <<<"$out" || ok=0
  done
  if [ "$ok" -eq 1 ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (wanted ${want[*]})"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

bump() { manifest 0.1.0-beta.2 1.0.0; lock 0.1.0-beta.2 1.0.0; }
npm_bump() { npm_lock 0.1.0-beta.2 1.0.0; }
bump_and_ui() { bump; write webui/src/app.ts 'export const a = 2;'; }
dep_manifest() { manifest 0.1.0-beta.1 2.0.0; }
dep_lock() { lock 0.1.0-beta.1 1.0.1; }
bump_and_dep_lock() { manifest 0.1.0-beta.2 1.0.0; lock 0.1.0-beta.2 1.0.1; }
rust_src() { write crates/cctui-a/src/lib.rs 'pub fn a() -> u8 { 1 }'; }
ui_src() { write webui/src/app.ts 'export const a = 3;'; }
bindings() { write webui/src/lib/bindings/A.ts 'export type A = number;'; }
catalog() { write crates/cctui-server/src/settings_catalog/catalog.toml 'x = 1'; }
journeys() { write docs/journeys/a/spec.md '# a'; }
workflow() { write .github/workflows/ci.yml 'name: ci'; }
lockfile() { npm_lock 0.1.0-beta.1 1.0.1; }
broken_lockfile() { write webui/package-lock.json '{'; }
docs() { write docs/notes.md 'notes'; }
release_key() { write keys/release.minisign.pub 'RWQ'; }
unit_template() { write packaging/systemd/cctui-daemon.service '[Unit]'; }
plugin_catalog() { write plugins/catalog.json '{}'; }
fixture() { write fixtures/a.json '{}'; }
deploy_example() { write deploy/examples/profile.yaml 'a: 1'; }
deploy_file() { write deploy/Dockerfile 'FROM scratch'; }

change "a version-only bump with a webui change is webui-only" \
  rust=false webui=true deps=false -- bump_and_ui
change "a version-only bump alone touches nothing" \
  ci=false rust=false webui=false deps=false schema=false -- bump
change "a version-only webui lockfile bump is not a deps change" deps=false rust=false -- npm_bump
change "a dependency bump in Cargo.toml is a Rust change" rust=true -- dep_manifest
change "a dependency bump in Cargo.lock is a Rust and deps change" rust=true deps=true -- dep_lock
change "a version bump riding with a dependency bump is still a Rust change" \
  rust=true deps=true -- bump_and_dep_lock
change "a crate source change is Rust-only" rust=true webui=false -- rust_src
change "a webui source change is webui-only" rust=false webui=true deps=false -- ui_src
change "a committed bindings change is checked by both" rust=true webui=true -- bindings
change "a settings catalog change runs the schema check" rust=true schema=true -- catalog
change "a journey spec change is checked by both" rust=true webui=true -- journeys
change "a ci.yml change flags ci" ci=true -- workflow
change "a webui lockfile change is a deps change" webui=true deps=true rust=false -- lockfile
change "an unreadable webui lockfile is a deps change" deps=true -- broken_lockfile
change "a docs change touches nothing" rust=false webui=false deps=false -- docs
change "a release key change is a Rust change" rust=true webui=false -- release_key
change "a packaging template change is a Rust change" rust=true webui=false -- unit_template
change "a plugin catalog change is a Rust change" rust=true webui=false -- plugin_catalog
change "a fixture change is a Rust change" rust=true webui=false -- fixture
change "a deploy example change is a Rust change" rust=true webui=false -- deploy_example
change "a Dockerfile change is not a Rust change" rust=false webui=false -- deploy_file

out="$(bash "$script" --all)"
if [ "$(grep -c '=true$' <<<"$out")" -eq 5 ]; then
  echo "ok   — --all turns every area on"
else
  echo "FAIL — --all turns every area on"
  echo "$out" | sed 's/^/       /'
  failures=$((failures + 1))
fi

[ "$failures" -eq 0 ] || exit 1
echo "all ci-changes cases passed"
