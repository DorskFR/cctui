#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/release-scope.sh"
repo="$(mktemp -d)"
trap 'rm -rf "$repo"' EXIT

g() { git -C "$repo" -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@"; }

write() { mkdir -p "$(dirname "$repo/$1")"; printf '%s\n' "$2" > "$repo/$1"; }

set_version() {
  write Cargo.toml "[workspace.package]
version = \"$1\""
  write Cargo.lock "[[package]]
name = \"cctui-server\"
version = \"$1\"

[[package]]
name = \"serde\"
version = \"$serde\""
  write webui/package.json "{ \"name\": \"webui\", \"version\": \"$1\" }"
}

release() {
  set_version "${1#v}"
  g add -A
  g commit -q --allow-empty -m "$1"
  g tag "$1"
}

serde=1.0.0
g init -q
write crates/cctui-server/src/main.rs "fn main() {}"
write webui/src/app.ts "export {}"
write deploy/Dockerfile "FROM scratch"
write deploy/worker.Dockerfile "FROM scratch"
write .dockerignore "target/"
write README.md "readme"
release v0.1.0-beta.1

failures=0
check() {
  local name="$1" tag="$2" released="$3" want="$4" out got line
  out="$(cd "$repo" && CCTUI_RELEASED_TAGS="$released" bash "$script" "$tag" "$(g rev-parse "$tag")" 2>&1)" || out="refused: $out"
  got=""
  while IFS= read -r line; do
    got+="$(printf '%s\n' "$out" | grep -E "^${line%%=*}=" || echo "${line%%=*}=<missing>")"$'\n'
  done <<< "$want"
  if [ "$got" = "$want"$'\n' ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name"
    diff <(echo "$want") <(printf '%s' "$got") | sed 's/^/       /' || true
    failures=$((failures + 1))
  fi
}

carried='modes={"cctui":"retag","cctui-worker":"retag","cctui-dispatcher-kube":"retag","cctui-orchestrator":"retag","cctui-ui":"retag"}'
all_built='modes={"cctui":"build","cctui-worker":"build","cctui-dispatcher-kube":"build","cctui-orchestrator":"build","cctui-ui":"build"}'

write webui/src/app.ts "export const x = 1"
release v0.1.0-beta.2
check "webui change + version bump rebuilds only the ui" v0.1.0-beta.2 "v0.1.0-beta.1" "prev_tag=v0.1.0-beta.1
prev_version=0.1.0-beta.1
rust=false
modes={\"cctui\":\"retag\",\"cctui-worker\":\"retag\",\"cctui-dispatcher-kube\":\"retag\",\"cctui-orchestrator\":\"retag\",\"cctui-ui\":\"build\"}
summary=Rebuilt: cctui-ui. Binaries and release assets carried from v0.1.0-beta.1. Images carried from v0.1.0-beta.1: cctui, cctui-worker, cctui-dispatcher-kube, cctui-orchestrator."

release v0.1.0-beta.3
check "a bare version bump carries everything" v0.1.0-beta.3 "v0.1.0-beta.1 v0.1.0-beta.2" "rust=false
$carried"

write README.md "readme 2"
release v0.1.0-beta.4
check "paths outside every component carry everything" v0.1.0-beta.4 "v0.1.0-beta.3" "rust=false
$carried"

write crates/cctui-server/src/main.rs "fn main() { println!(); }"
release v0.1.0-beta.5
check "a crate change rebuilds every rust image, not the ui" v0.1.0-beta.5 "v0.1.0-beta.4" "rust=true
modes={\"cctui\":\"build\",\"cctui-worker\":\"build\",\"cctui-dispatcher-kube\":\"build\",\"cctui-orchestrator\":\"build\",\"cctui-ui\":\"retag\"}"

serde=1.0.1
release v0.1.0-beta.6
check "a dependency bump in Cargo.lock is a rust change" v0.1.0-beta.6 "v0.1.0-beta.5" "rust=true"

write deploy/worker.Dockerfile "FROM debian"
release v0.1.0-beta.7
check "a worker Dockerfile change rebuilds the worker around the previous binaries" v0.1.0-beta.7 "v0.1.0-beta.6" "rust=false
modes={\"cctui\":\"retag\",\"cctui-worker\":\"prevbins\",\"cctui-dispatcher-kube\":\"retag\",\"cctui-orchestrator\":\"retag\",\"cctui-ui\":\"retag\"}"

write .dockerignore "target/
dist/"
release v0.1.0-beta.8
check ".dockerignore rebuilds every image" v0.1.0-beta.8 "v0.1.0-beta.7" "rust=false
modes={\"cctui\":\"prevbins\",\"cctui-worker\":\"prevbins\",\"cctui-dispatcher-kube\":\"prevbins\",\"cctui-orchestrator\":\"prevbins\",\"cctui-ui\":\"build\"}"

release v0.1.0-beta.9
release v0.1.0-beta.10
check "the previous release is the highest lower one on the channel" v0.1.0-beta.10 \
  "v0.1.0-beta.11 v0.1.0-beta.10 v0.1.0-beta.9 v0.1.0-beta.2 v0.0.9 garbage" "prev_tag=v0.1.0-beta.9"
check "an unpublished tag is never the previous release" v0.1.0-beta.10 "v0.1.0-beta.8" "prev_tag=v0.1.0-beta.8"

release v0.1.0
check "a stable never carries from a beta" v0.1.0 "v0.1.0-beta.10" "prev_tag=
rust=true
$all_built
summary=Full build: no previous release to carry from."

release v0.1.1
check "a stable carries from the previous stable" v0.1.1 "v0.1.0 v0.1.0-beta.10" "prev_tag=v0.1.0
rust=false"

release v0.2.0-beta.1
check "a beta carries from a newer stable over an older beta" v0.2.0-beta.1 "v0.1.0-beta.10 v0.1.1 v0.1.0" "prev_tag=v0.1.1
rust=false"
check "a beta never carries from its own stable" v0.1.0-beta.10 "v0.1.0 v0.1.0-beta.9" "prev_tag=v0.1.0-beta.9"

out="$(cd "$repo" && CCTUI_FULL_RELEASE=true CCTUI_RELEASED_TAGS=v0.1.0 bash "$script" v0.1.1 "$(g rev-parse v0.1.1)")"
if printf '%s\n' "$out" | grep -qx "$all_built" && printf '%s\n' "$out" | grep -qx "summary=Full build: full release requested."; then
  echo "ok   — CCTUI_FULL_RELEASE=true forces a full build"
else
  echo "FAIL — CCTUI_FULL_RELEASE=true forces a full build"; printf '%s\n' "$out" | sed 's/^/       /'
  failures=$((failures + 1))
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures failure(s)" >&2
  exit 1
fi
