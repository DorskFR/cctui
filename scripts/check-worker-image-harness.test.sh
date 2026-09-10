#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-worker-image-harness.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fake_docker() {
  cat > "$tmp/docker" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do
  case "\$a" in
    claude) echo "$1 (Claude Code)"; exit 0 ;;
    codex)  echo "codex-cli $2";     exit 0 ;;
  esac
done
echo "unexpected: \$*" >&2; exit 1
EOF
  chmod +x "$tmp/docker"
  echo "$tmp/docker"
}

failures=0
expect() {
  local want="$1" name="$2"; shift 2
  local out status
  set +e
  out="$("$@" 2>&1)"; status=$?
  set -e
  if [ "$status" -eq "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want)"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

claude_floor="$(sed -n 's/^ARG CLAUDE_CODE_VERSION=\(.*\)$/\1/p' "$here/../deploy/worker.Dockerfile" | head -n1)"
codex_floor="$(sed -n 's/^ARG CODEX_VERSION=\(.*\)$/\1/p' "$here/../deploy/worker.Dockerfile" | head -n1)"

expect 0 "an image at both floors passes" \
  env DOCKER="$(fake_docker "$claude_floor" "$codex_floor")" bash "$script" worker:test
expect 1 "an image whose claude is below the floor fails" \
  env DOCKER="$(fake_docker 0.0.1 "$codex_floor")" bash "$script" worker:test
expect 1 "an image whose codex is below the floor fails" \
  env DOCKER="$(fake_docker "$claude_floor" 0.0.1)" bash "$script" worker:test

cat > "$tmp/broken" <<'EOF'
#!/usr/bin/env bash
echo "Error response from daemon: no such image" >&2; exit 125
EOF
chmod +x "$tmp/broken"
expect 1 "an unrunnable image fails rather than skipping the check" \
  env DOCKER="$tmp/broken" bash "$script" worker:test

cat > "$tmp/silent" <<'EOF'
#!/usr/bin/env bash
echo "unknown"; exit 0
EOF
chmod +x "$tmp/silent"
expect 1 "a harness printing no version fails rather than skipping the check" \
  env DOCKER="$tmp/silent" bash "$script" worker:test

expect 1 "a missing image argument fails" bash "$script"

[ "$failures" -eq 0 ] || exit 1
echo "all worker-image harness cases passed"
