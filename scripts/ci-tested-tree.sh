#!/usr/bin/env bash
# Usage: ci-tested-tree.sh <owner/repo> <tree> [run id to ignore]
# Finds a green run by the `ci-tree-<tree>` artifact ci.yml uploads: a pull
# request run tests the merge ref, whose tree a squash merge reproduces.
set -euo pipefail

repo="$1" tree="$2" ignore="${3:-}"
workflow="${CI_WORKFLOW:-ci.yml}"

ids="$(gh api "repos/$repo/actions/artifacts?name=ci-tree-$tree&per_page=100" \
  --jq '.artifacts[].workflow_run.id')" || exit 1

for id in $ids; do
  [ "$id" = "$ignore" ] && continue
  run="$(gh api "repos/$repo/actions/runs/$id" \
    --jq '"\(.status) \(.conclusion) \(.path) \(.html_url)"')" || continue
  read -r status conclusion path url <<<"$run"
  case "$path" in
    ".github/workflows/$workflow"*) ;;
    *) continue ;;
  esac
  if [ "$status" = "completed" ] && [ "$conclusion" = "success" ]; then
    echo "$url"
    exit 0
  fi
done
exit 1
