#!/usr/bin/env bash
# A shell guard test that forgets to unset GIT_DIR rewrites the config of
# whatever repository GIT_DIR points at. Run after the *.test.sh steps.
set -euo pipefail

repo="${1:-${GITHUB_WORKSPACE:-$PWD}}"
status=0

bare="$(git -C "$repo" config --local --get core.bare || echo false)"
if [ "$bare" != "false" ]; then
  echo "core.bare is '$bare' in $repo: a test leaked git state into the checkout" >&2
  status=1
fi

hooks="$(git -C "$repo" config --local --get core.hooksPath || true)"
if [ -n "$hooks" ]; then
  echo "core.hooksPath is '$hooks' in $repo: a test leaked git state into the checkout" >&2
  status=1
fi

dirty="$(git -C "$repo" status --porcelain)"
if [ -n "$dirty" ]; then
  echo "the checkout is dirty after the tests:" >&2
  echo "$dirty" >&2
  status=1
fi

[ "$status" -eq 0 ] && echo "git state in $repo is untouched"
exit "$status"
