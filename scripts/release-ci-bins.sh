#!/usr/bin/env bash
# Usage: release-ci-bins.sh <owner/repo> <sha> <dir>
# Exit 2 means the ci.yml push run will not deliver them: the caller builds.
set -euo pipefail

repo="$1" sha="$2" dir="$3"
workflow="${CI_WORKFLOW:-ci.yml}"
timeout="${CI_WAIT_SECONDS:-900}"
appear="${CI_APPEAR_SECONDS:-60}"
interval="${CI_POLL_SECONDS:-15}"
read -ra names <<<"${CI_BIN_ARTIFACTS:-release-bins-linux-amd64 release-bins-linux-arm64 release-image-bins release-image-bins-server}"
waited=0

while :; do
  run="$(gh api "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&event=push&per_page=1" \
    --jq '.workflow_runs[0] | select(.) | "\(.id) \(.status)"')"
  if [ -z "$run" ]; then
    if [ "$waited" -ge "$appear" ]; then
      echo "no $workflow push run for $sha" >&2
      exit 2
    fi
    echo "no $workflow push run for $sha yet, waiting"
  else
    read -r id status <<<"$run"
    have="$(gh api "repos/$repo/actions/runs/$id/artifacts?per_page=100" \
      --jq '.artifacts[] | select(.expired | not) | .name')"
    missing=()
    for n in "${names[@]}"; do grep -qxF "$n" <<<"$have" || missing+=("$n"); done
    if [ "${#missing[@]}" -eq 0 ]; then
      for n in "${names[@]}"; do gh run download "$id" --repo "$repo" --name "$n" --dir "$dir/$n"; done
      echo "took ${names[*]} from $workflow run $id"
      exit 0
    fi
    if [ "$status" = completed ]; then
      echo "$workflow run $id finished without ${missing[*]}" >&2
      exit 2
    fi
    echo "$workflow run $id is '$status', waiting for ${missing[*]}"
  fi
  if [ "$waited" -ge "$timeout" ]; then
    echo "gave up after ${timeout}s waiting for $workflow binaries of $sha" >&2
    exit 2
  fi
  sleep "$interval"
  waited=$((waited + interval))
done
