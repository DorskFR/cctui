#!/usr/bin/env bash
# Exercises the adapter switch in deploy/worker-entrypoint.sh: adapter_kind()
# maps TASK_ADAPTER to a harness kind, require_known_adapter() fails an unknown
# one with a verdict, and phase_acp_config() lays out the agent's state with
# its sandbox and telemetry off.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
log()  { :; }

eval "$(grep '^ACP_AGENTS=' "$ENTRY")"
eval "$(awk '/^adapter_kind\(\)/,/^}/' "$ENTRY")"
eval "$(awk '/^adapter_is_codex\(\)/,/^}/' "$ENTRY")"
eval "$(awk '/^require_known_adapter\(\)/,/^}/' "$ENTRY")"
eval "$(awk '/^phase_acp_config\(\)/,/^}/' "$ENTRY")"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
WORKER_UID="$(id -u)"
WORKER_USER=worker

echo "== 1. every known adapter maps to its kind =="
unset TASK_ADAPTER
check "unset is claude" claude "$(adapter_kind)"
for a in claude claude-code claude_code; do TASK_ADAPTER=$a; check "$a" claude "$(adapter_kind)"; done
for a in codex codex-cli;               do TASK_ADAPTER=$a; check "$a" codex "$(adapter_kind)"; done
TASK_ADAPTER=opencode; check "opencode" opencode "$(adapter_kind)"
for a in $ACP_AGENTS;                   do TASK_ADAPTER=$a; check "$a" acp "$(adapter_kind)"; done
TASK_ADAPTER=codex;    check "adapter_is_codex" yes "$(adapter_is_codex && echo yes)"
TASK_ADAPTER=gemini;   check "adapter_is_codex for gemini" no "$(adapter_is_codex && echo yes || echo no)"

echo "== 2. an unknown adapter is unknown, never claude =="
for a in gpt claude-cli acp "codex exec"; do TASK_ADAPTER=$a; check "'$a'" unknown "$(adapter_kind)"; done

echo "== 3. require_known_adapter fails an unknown one with a verdict =="
RESULT_FILE="$tmp/result.json"
TASK_ID=T-9
TASK_ADAPTER=nope
rc=0; (require_known_adapter) || rc=$?
check "exit 1" 1 "$rc"
check "verdict status" failed "$(jq -r .status "$RESULT_FILE")"
check "verdict task_id" T-9 "$(jq -r .task_id "$RESULT_FILE")"
check "verdict names the adapter" yes "$(jq -r .error "$RESULT_FILE" | grep -q 'nope' && echo yes)"
rm -f "$RESULT_FILE"
TASK_ADAPTER=qwen
rc=0; (require_known_adapter) || rc=$?
check "known adapter passes" 0 "$rc"
check "no verdict written" no "$([ -e "$RESULT_FILE" ] && echo yes || echo no)"

echo "== 4. phase_acp_config lays out gemini and qwen state =="
for a in gemini qwen; do
  WORKER_HOME="$tmp/home-$a"; mkdir -p "$WORKER_HOME"
  TASK_ADAPTER=$a phase_acp_config
  f="$WORKER_HOME/.$a/settings.json"
  check "$a settings.json exists" yes "$([ -s "$f" ] && echo yes)"
  check "$a sandbox off" false "$(jq -r .tools.sandbox "$f")"
  check "$a telemetry off" false "$(jq -r .telemetry.enabled "$f")"
  check "$a usage stats off" false "$(jq -r .general.usageStatisticsEnabled "$f")"
done
check "gemini auth type" gemini-api-key "$(jq -r .security.auth.selectedType "$tmp/home-gemini/.gemini/settings.json")"
check "qwen auth type" openai "$(jq -r .security.auth.selectedType "$tmp/home-qwen/.qwen/settings.json")"

echo "== 5. an existing settings.json is merged, not replaced =="
WORKER_HOME="$tmp/home-merge"; mkdir -p "$WORKER_HOME/.gemini"
printf '{"security":{"auth":{"selectedType":"vertex-ai"}},"ui":{"theme":"dark"}}\n' > "$WORKER_HOME/.gemini/settings.json"
TASK_ADAPTER=gemini phase_acp_config
check "auth kept" vertex-ai "$(jq -r .security.auth.selectedType "$WORKER_HOME/.gemini/settings.json")"
check "other keys kept" dark "$(jq -r .ui.theme "$WORKER_HOME/.gemini/settings.json")"
check "sandbox still off" false "$(jq -r .tools.sandbox "$WORKER_HOME/.gemini/settings.json")"

echo "== 6. goose gets its config and data dirs =="
WORKER_HOME="$tmp/home-goose"; mkdir -p "$WORKER_HOME"
TASK_ADAPTER=goose phase_acp_config
check "config.yaml" yes "$([ -e "$WORKER_HOME/.config/goose/config.yaml" ] && echo yes)"
check "data dir" yes "$([ -d "$WORKER_HOME/.local/share/goose" ] && echo yes)"

echo "== 7. a non-ACP adapter is a no-op =="
WORKER_HOME="$tmp/home-claude"; mkdir -p "$WORKER_HOME"
TASK_ADAPTER=claude-code phase_acp_config
check "nothing created" "" "$(ls -A "$WORKER_HOME")"

[ "$fail" -eq 0 ] || exit 1
echo "all adapter-kind cases passed"
