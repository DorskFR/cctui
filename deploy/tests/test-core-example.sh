#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fail=0

if ! cmp -s "$root/deploy/features/core.toml" "$root/deploy/core/core.toml"; then
  echo "FAIL deploy/core/core.toml differs from deploy/features/core.toml"
  fail=1
fi

if command -v kustomize >/dev/null 2>&1; then
  render=(kustomize build "$root/deploy/core")
elif command -v kubectl >/dev/null 2>&1; then
  render=(kubectl kustomize "$root/deploy/core")
else
  echo "SKIP render: neither kustomize nor kubectl is installed"
  exit "$fail"
fi
out="$("${render[@]}")"

kinds="$(printf '%s\n' "$out" | sed -n 's/^kind: //p' | sort | tr '\n' ' ')"
want="ConfigMap Deployment Deployment Service Service "
if [ "$kinds" != "$want" ]; then
  echo "FAIL kinds: got '$kinds', want '$want'"
  fail=1
fi
for forbidden in PersistentVolumeClaim volumeClaimTemplates 'kind: Secret'; do
  if printf '%s\n' "$out" | grep -q "$forbidden"; then
    echo "FAIL the rendered example contains $forbidden"
    fail=1
  fi
done
if ! printf '%s\n' "$out" | grep -q 'name: cctui-core-features-'; then
  echo "FAIL the feature ConfigMap is not mounted by its generated name"
  fail=1
fi

[ "$fail" = 0 ] && echo "ok core example"
exit "$fail"
