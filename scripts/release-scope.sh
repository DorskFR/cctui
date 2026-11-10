#!/usr/bin/env bash
# Usage: release-scope.sh <tag> <sha>
# Decides what a release rebuilds by diffing <sha> against the previous
# published release, as GITHUB_OUTPUT lines. A stable only carries from a stable:
# a carried component reports the version it was built as.
#   prev_tag, prev_version   where unchanged components are carried from
#   rust=true|false          rebuild every Rust binary and release asset
#   modes={"<image>":"build"|"prevbins"|"retag",...}
#     prevbins  build the image around the prev_tag image's binaries
#     retag     publish the prev_tag image under the new tags
#   summary
#
# The Rust binaries all embed the workspace version and the server's daemon
# manifest serves the assets of its own version, so Rust is rebuilt as one unit
# or not at all: a carried component keeps reporting prev_version.
#
# CCTUI_RELEASED_TAGS: whitespace-separated tags of published releases.
# CCTUI_FULL_RELEASE=true rebuilds everything.
set -euo pipefail

tag="${1:?usage: release-scope.sh <tag> <sha>}"
sha="${2:?usage: release-scope.sh <tag> <sha>}"
version="${tag#v}"

images=(cctui cctui-worker cctui-dispatcher-kube cctui-orchestrator cctui-ui)
rust_images=(cctui cctui-worker cctui-dispatcher-kube cctui-orchestrator)

rust_paths=(
  crates/ Cargo.toml Cargo.lock rust-toolchain.toml .cargo/ keys/ migrations/
  packaging/ plugins/ fixtures/ deploy/examples/
  scripts/release-build-bins.sh scripts/release-build-server.sh
)
ui_paths=(webui/)
all_paths=(.dockerignore .github/workflows/release.yml scripts/release-scope.sh scripts/release-prev-bins.sh)
declare -A image_paths=(
  [cctui]="deploy/Dockerfile"
  [cctui-worker]="deploy/worker.Dockerfile deploy/worker-entrypoint.sh deploy/worker-net-init.sh deploy/guard-proxy-entrypoint.sh scripts/check-worker-image-harness.sh scripts/check-claude-version-drift.sh scripts/check-codex-version-drift.sh"
  [cctui-dispatcher-kube]="deploy/dispatcher.Dockerfile"
  [cctui-orchestrator]="deploy/orchestrator.Dockerfile"
  [cctui-ui]=""
)
version_files=(Cargo.toml Cargo.lock webui/package.json webui/package-lock.json)

is_beta() { [[ "$1" == *-beta.* ]]; }
# X.Y.Z sorts above every X.Y.Z-beta.N.
order_key() {
  local v="${1#v}"
  if is_beta "$v"; then echo "${v%-beta.*}.${v##*-beta.}"; else echo "$v.999999999"; fi
}
newer() { [ "$(printf '%s\n%s\n' "$(order_key "$1")" "$(order_key "$2")" | sort -V | tail -n1)" = "$(order_key "$1")" ]; }

prev_tag=""
for t in ${CCTUI_RELEASED_TAGS:-}; do
  [[ "$t" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]] || continue
  is_beta "$tag" || ! is_beta "$t" || continue
  [ "$t" != "$tag" ] && newer "$tag" "$t" || continue
  if [ -z "$prev_tag" ] || newer "$t" "$prev_tag"; then prev_tag="$t"; fi
done

emit() {
  local rust="$1" modes="" summary="$2" img
  for img in "${images[@]}"; do modes+="${modes:+,}\"$img\":\"${mode[$img]}\""; done
  echo "prev_tag=$prev_tag"
  echo "prev_version=${prev_tag#v}"
  echo "rust=$rust"
  echo "modes={$modes}"
  echo "summary=$summary"
}

declare -A mode
full_reason=""
if [ "${CCTUI_FULL_RELEASE:-}" = true ]; then
  full_reason="full release requested"
elif [ -z "$prev_tag" ]; then
  full_reason="no previous release to carry from"
fi
if [ -n "$full_reason" ]; then
  prev_tag=""
  for img in "${images[@]}"; do mode[$img]=build; done
  emit true "Full build: $full_reason."
  exit 0
fi

if ! git rev-parse -q --verify "refs/tags/$prev_tag^{commit}" >/dev/null; then
  git fetch -q --no-tags --depth=1 origin "refs/tags/$prev_tag:refs/tags/$prev_tag"
fi

prev_version="${prev_tag#v}"
changed=()
while IFS= read -r f; do
  for v in "${version_files[@]}"; do
    [ "$f" = "$v" ] || continue
    if git cat-file -e "$prev_tag:$f" 2>/dev/null &&
      [ "$(git show "$prev_tag:$f" | sed "s/\"${prev_version//./\\.}\"/\"$version\"/g")" = "$(git show "$sha:$f")" ]; then
      continue 2
    fi
  done
  changed+=("$f")
done < <(git diff --name-only "$prev_tag" "$sha")

touched() {
  local p f
  for p in "$@"; do
    for f in "${changed[@]}"; do
      case "$p" in
        */) [[ "$f" == "$p"* ]] && return 0 ;;
        *) [ "$f" = "$p" ] && return 0 ;;
      esac
    done
  done
  return 1
}

rust=false all=false
touched "${rust_paths[@]}" && rust=true
touched "${all_paths[@]}" && all=true

for img in "${rust_images[@]}"; do
  # shellcheck disable=SC2086
  if [ "$rust" = true ]; then
    mode[$img]=build
  elif [ "$all" = true ] || touched ${image_paths[$img]}; then
    mode[$img]=prevbins
  else
    mode[$img]=retag
  fi
done
if [ "$all" = true ] || touched "${ui_paths[@]}"; then mode[cctui-ui]=build; else mode[cctui-ui]=retag; fi

rebuilt="" carried=""
for img in "${images[@]}"; do
  case "${mode[$img]}" in
    retag) carried+="${carried:+, }$img" ;;
    *) rebuilt+="${rebuilt:+, }$img" ;;
  esac
done
summary="Rebuilt: ${rebuilt:-nothing}."
[ "$rust" = true ] || summary+=" Binaries and release assets carried from $prev_tag."
[ -z "$carried" ] || summary+=" Images carried from $prev_tag: $carried."
emit "$rust" "$summary"
