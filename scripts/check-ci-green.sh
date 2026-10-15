#!/usr/bin/env bash
# Usage: check-ci-green.sh <owner/repo> <sha>
# A finished run on <sha> is authoritative; until it finishes, a green run on
# the same tree passes too.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$1" sha="$2"
workflow="${CI_WORKFLOW:-ci.yml}"
timeout="${CI_WAIT_SECONDS:-3600}"
interval="${CI_POLL_SECONDS:-30}"
waited=0

tree="$(gh api "repos/$repo/git/commits/$sha" --jq .tree.sha)" || tree=""

while :; do
  run="$(gh api "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&per_page=1" \
    --jq '.workflow_runs[0] | select(.) | "\(.status) \(.conclusion) \(.html_url)"')"
  status="" url=""
  if [ -n "$run" ]; then
    read -r status conclusion url <<<"$run"
    if [ "$status" = "completed" ]; then
      if [ "$conclusion" = "success" ]; then
        echo "$workflow passed on $sha: $url"
        exit 0
      fi
      echo "$workflow concluded '$conclusion' on $sha: $url; refusing to publish" >&2
      exit 1
    fi
  fi
  if [ -n "$tree" ] && tested="$(bash "$here/ci-tested-tree.sh" "$repo" "$tree")"; then
    echo "$workflow passed on tree $tree, the tree of $sha: $tested"
    exit 0
  fi
  if [ -z "$run" ]; then
    echo "no $workflow run found for $sha or its tree; refusing to publish" >&2
    exit 1
  fi
  if [ "$waited" -ge "$timeout" ]; then
    echo "$workflow still '$status' on $sha after ${timeout}s: $url" >&2
    exit 1
  fi
  echo "$workflow is '$status' on $sha, waiting: $url"
  sleep "$interval"
  waited=$((waited + interval))
done
