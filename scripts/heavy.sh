#!/usr/bin/env bash
# Usage: heavy.sh <cmd...>  — one heavy job per machine, memory-capped.
#   CCTUI_HEAVY_MEM             MemoryMax of the job's systemd scope (default 10G)
#   CCTUI_HEAVY_JOBS            CARGO_BUILD_JOBS (default: half the cores)
#   CCTUI_HEAVY_VITEST_WORKERS  vitest --maxWorkers (default: CCTUI_HEAVY_JOBS)
#   CCTUI_HEAVY_LOCK            lock file
# In CI only the lock applies. Inside another heavy job (CCTUI_HEAVY=1) the
# command runs directly.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: heavy.sh <cmd...>" >&2
  exit 2
fi

ci=""
case "${CI:-}" in "" | 0 | false) ;; *) ci=1 ;; esac

cmd=("$@")
if [ -z "$ci" ]; then
  cores="$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)"
  jobs="${CCTUI_HEAVY_JOBS:-$((cores > 1 ? cores / 2 : 1))}"
  workers="${CCTUI_HEAVY_VITEST_WORKERS:-$jobs}"
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$jobs}"

  vitest="" capped=""
  for a in "$@"; do
    [ "$a" = -- ] && break
    [ "${a##*/}" = vitest ] && vitest=1
    case "$a" in --maxWorkers | --maxWorkers=*) capped=1 ;; esac
  done
  if [ -n "$vitest" ] && [ -z "$capped" ]; then
    cmd=()
    inserted=""
    for a in "$@"; do
      if [ "$a" = -- ] && [ -z "$inserted" ]; then
        cmd+=("--maxWorkers=$workers")
        inserted=1
      fi
      cmd+=("$a")
    done
    [ -n "$inserted" ] || cmd+=("--maxWorkers=$workers")
  fi
fi

if [ -n "${CCTUI_HEAVY:-}" ]; then
  exec "${cmd[@]}"
fi
export CCTUI_HEAVY=1

if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  lock="${CCTUI_HEAVY_LOCK:-${XDG_RUNTIME_DIR%/}/cctui-heavy.lock}"
else
  lock="${CCTUI_HEAVY_LOCK:-${TMPDIR:-/tmp}}"
  [ -n "${CCTUI_HEAVY_LOCK:-}" ] || lock="${lock%/}/cctui-heavy-$(id -u).lock"
fi
holder="$lock.holder"

if command -v flock >/dev/null 2>&1; then
  exec 9>>"$lock"
  if ! flock -n 9; then
    who="$(cat "$holder" 2>/dev/null || true)"
    echo "heavy: queued behind ${who:-another heavy job}" >&2
    flock 9
  fi
  printf 'pid %s (%s)\n' "$$" "$*" >"$holder"
  trap 'rm -f "$holder"' EXIT
else
  echo "heavy: flock not found, running unserialized" >&2
fi

mem="${CCTUI_HEAVY_MEM:-10G}"
scope=()
if [ -z "$ci" ] && command -v systemd-run >/dev/null 2>&1 &&
  systemd-run --user --scope --quiet true >/dev/null 2>&1 9>&-; then
  scope=(systemd-run --user --scope --quiet -p "MemoryMax=$mem" --)
fi

status=0
${scope[@]+"${scope[@]}"} "${cmd[@]}" 9>&- || status=$?

if [ "$status" -eq 137 ] && [ "${#scope[@]}" -gt 0 ]; then
  echo "heavy: '$*' was killed, most likely by its MemoryMax=$mem ceiling; raise CCTUI_HEAVY_MEM to allow more" >&2
fi
exit "$status"
