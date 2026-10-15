#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/release-ci-bins.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin" "$tmp/stub"

# `runs` and `artifacts` hold one answer per poll (the last line repeats).
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
next() {
  local f="$GH_STUB/$1" n
  n="$(cat "$f.n")"; n=$((n + 1)); echo "$n" > "$f.n"
  local total; total="$(wc -l < "$f")"
  [ "$total" -eq 0 ] && return 0
  [ "$n" -gt "$total" ] && n="$total"
  sed -n "${n}p" "$f" | tr '|' '\n' | sed '/^$/d'
}
case "$1 $2" in
  "api "*"/runs?head_sha="*) next runs ;;
  "api "*"/artifacts?"*) next artifacts ;;
  "run download")
    shift 2; id="$1"; shift
    while [ $# -gt 0 ]; do case "$1" in --name) name="$2"; shift 2 ;; --dir) dir="$2"; shift 2 ;; *) shift ;; esac; done
    mkdir -p "$dir"; echo "$id" > "$dir/from-run"; echo "$name" >> "$GH_STUB/downloaded"
    ;;
  *) echo "unexpected gh call: $*" >&2; exit 1 ;;
esac
STUB
chmod +x "$tmp/bin/gh"

all="release-bins-linux-amd64|release-bins-linux-arm64|release-image-bins|release-image-bins-server"
reset() {
  rm -rf "$tmp/stub"/* "$tmp/out"
  for f in runs artifacts; do : > "$tmp/stub/$f"; echo 0 > "$tmp/stub/$f.n"; done
}
runs() { printf '%s\n' "$@" > "$tmp/stub/runs"; }
artifacts() { printf '%s\n' "$@" > "$tmp/stub/artifacts"; }

failures=0
expect() {
  local want="$1" name="$2" pattern="${3:-}" out status
  set +e
  out="$(PATH="$tmp/bin:$PATH" GH_STUB="$tmp/stub" CI_POLL_SECONDS=1 CI_WAIT_SECONDS=3 CI_APPEAR_SECONDS=1 \
    bash "$script" o/r abc "$tmp/out" 2>&1)"; status=$?
  set -e
  if [ "$status" -eq "$want" ] && { [ -z "$pattern" ] || grep -q -- "$pattern" <<<"$out"; }; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want${pattern:+, output matching '$pattern'})"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

reset; runs "7 completed"; artifacts "$all"
expect 0 "every artifact present: downloaded" "took"
if [ "$(sort "$tmp/stub/downloaded" | tr '\n' '|')" = "$(tr '|' '\n' <<<"$all" | sort | tr '\n' '|')" ] \
  && [ "$(cat "$tmp/out/release-image-bins/from-run")" = 7 ]; then
  echo "ok   — all four come from the push run"
else
  echo "FAIL — downloads: $(tr '\n' ' ' < "$tmp/stub/downloaded")"; failures=$((failures + 1))
fi

reset; runs "7 in_progress" "7 in_progress" "7 completed"; artifacts "release-bins-linux-amd64" "release-bins-linux-amd64|release-image-bins" "$all"
expect 0 "artifacts landing while the run is going are waited for" "waiting for"

reset; runs "7 completed"; artifacts "release-bins-linux-amd64|release-image-bins"
expect 2 "a finished run without every artifact means build it" "finished without release-bins-linux-arm64 release-image-bins-server"

reset; runs "7 completed"; artifacts ""
expect 2 "a push run that built nothing (no Rust change) means build it"

reset
expect 2 "no push run at all means build it" "no ci.yml push run"

reset; runs "" "7 completed"; artifacts "$all"
expect 0 "a push run that shows up a moment later is used"

reset; runs "7 in_progress"; artifacts "release-bins-linux-amd64"
expect 2 "a run that never delivers gives up after the wait" "gave up"

[ "$failures" -eq 0 ] && echo "all release-ci-bins cases passed"
exit "$failures"
