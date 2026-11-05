#!/usr/bin/env bash
# Usage: redteam-t2-gate.sh <version>
# Release gate: passes only on a fresh, complete, contained T2 run on <version>.
# The gate record is $REDTEAM_T2_RECORD (a t2-gate.json) when set, else the
# newest unexpired `redteam-t2-<version>` artifact of a successful
# redteam-t2.yml run in $GH_REPO. REDTEAM_T2_MAX_AGE_HOURS bounds its age (168).
set -euo pipefail

version="${1:-}"
max_age_hours="${REDTEAM_T2_MAX_AGE_HOURS:-168}"
workflow_path=".github/workflows/redteam-t2.yml"

fail() {
  echo "redteam T2 gate: $*; refusing to deploy ${version:-<none>}" >&2
  exit 1
}

case "$version" in
  "" | *[!0-9A-Za-z.+-]*) fail "invalid version '${version}'" ;;
esac

record="${REDTEAM_T2_RECORD:-}"
if [ -z "$record" ]; then
  repo="${GH_REPO:-}"
  [ -n "$repo" ] || fail "set GH_REPO=<owner/repo> or REDTEAM_T2_RECORD"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  name="redteam-t2-$version"
  artifacts="$(gh api "repos/$repo/actions/artifacts?name=$name&per_page=30" \
    --jq '[.artifacts[] | select(.expired | not)] | sort_by(.created_at) | reverse | .[] | "\(.id) \(.workflow_run.id)"')" \
    || fail "cannot list artifacts"
  run=""
  while read -r _artifact run_id; do
    [ -n "${run_id:-}" ] || continue
    meta="$(gh api "repos/$repo/actions/runs/$run_id" --jq '"\(.conclusion) \(.path)"')" || continue
    read -r conclusion path <<<"$meta"
    if [ "$conclusion" = success ] && [ "$path" = "$workflow_path" ]; then
      run="$run_id"
      break
    fi
  done <<<"$artifacts"
  [ -n "$run" ] || fail "no successful redteam-t2 run published $name"
  gh run download "$run" --repo "$repo" -n "$name" -D "$tmp" || fail "cannot download $name"
  record="$tmp/t2-gate.json"
fi

[ -f "$record" ] || fail "no gate record"
now="$(date +%s)"
if ! jq -e --arg v "$version" --argjson now "$now" --argjson max "$((max_age_hours * 3600))" '
  .schema == 1 and .version == $v and .verdict == "contained"
  and (.cells | type) == "number" and .cells > 0 and .contained_cells == .cells
  and (.generated_at | type) == "number"
  and ($now - .generated_at) <= $max and .generated_at <= ($now + 300)
' "$record" >/dev/null 2>&1; then
  summary="$(jq -c '{version, verdict, cells, contained_cells, generated_at}' "$record" 2>/dev/null || echo unreadable)"
  fail "the T2 record is not a fresh, complete, contained run on $version: $summary"
fi
echo "redteam T2 gate: $version contained ($(jq -r '"\(.contained_cells)/\(.cells) cells, run \(.run_id)"' "$record"))"
