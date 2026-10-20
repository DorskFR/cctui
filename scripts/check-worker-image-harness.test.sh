#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-worker-image-harness.sh"
dockerfile="$here/../deploy/worker.Dockerfile"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

arg() { sed -n "s/^ARG $1=\(.*\)$/\1/p" "$dockerfile" | head -n1; }

claude_floor="$(arg CLAUDE_CODE_VERSION)"
codex_floor="$(arg CODEX_VERSION)"
opencode_pin="$(arg OPENCODE_VERSION)"
gemini_pin="$(arg GEMINI_CLI_VERSION)"
qwen_pin="$(arg QWEN_CODE_VERSION)"
goose_pin="$(arg GOOSE_VERSION)"

# A docker stand-in answering `--version` per harness; an override argument
# `name=version` replaces that harness's answer.
fake_docker() {
  local claude="$claude_floor" codex="$codex_floor" opencode="$opencode_pin"
  local gemini="$gemini_pin" qwen="$qwen_pin" goose="$goose_pin"
  local kv
  for kv in "$@"; do
    printf -v "${kv%%=*}" '%s' "${kv#*=}"
  done
  cat > "$tmp/docker" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do
  case "\$a" in
    claude)   echo "$claude (Claude Code)"; exit 0 ;;
    codex)    echo "codex-cli $codex";     exit 0 ;;
    opencode) echo "$opencode";            exit 0 ;;
    gemini)   echo "$gemini";              exit 0 ;;
    qwen)     echo "$qwen";                exit 0 ;;
    goose)    echo "$goose";               exit 0 ;;
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

for pin in "$opencode_pin" "$gemini_pin" "$qwen_pin" "$goose_pin"; do
  [ -n "$pin" ] || { echo "FAIL — the Dockerfile declares every agent ARG"; exit 1; }
done

expect 0 "an image matching every floor and pin passes" \
  env DOCKER="$(fake_docker)" bash "$script" worker:test
expect 1 "an image whose claude is below the floor fails" \
  env DOCKER="$(fake_docker claude=0.0.1)" bash "$script" worker:test
expect 1 "an image whose codex is below the floor fails" \
  env DOCKER="$(fake_docker codex=0.0.1)" bash "$script" worker:test
for agent in opencode gemini qwen goose; do
  expect 1 "an image whose $agent is off its pin fails" \
    env DOCKER="$(fake_docker "$agent=0.0.1")" bash "$script" worker:test
done
expect 0 "a version printed with a prefix still matches" \
  env DOCKER="$(fake_docker "gemini=gemini-cli v$gemini_pin")" bash "$script" worker:test

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

expect 1 "a missing agent binary fails rather than skipping the check" \
  env DOCKER="$(fake_docker "goose=")" bash "$script" worker:test

expect 1 "a missing image argument fails" bash "$script"

[ "$failures" -eq 0 ] || exit 1
echo "all worker-image harness cases passed"
