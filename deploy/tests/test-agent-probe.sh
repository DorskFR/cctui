#!/usr/bin/env bash
# An agent-less worker refuses to run a session, a dispatch whose adapter
# binary is missing fails naming it, and the probe list matches the adapters.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
ENTRY="${1:-$here/../worker-entrypoint.sh}"
ADAPTERS="$here/../../crates/cctui-daemon/src/adapters"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else bad "$1: no [$2] in [$3]"; fi; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "== 1. the probed binaries are the adapters' defaults =="
rust_bins=$(
    {
        sed -n 's/.*claude_bin: "\([^"]*\)"\.to_string().*/\1/p' "$ADAPTERS/claude_code/control/mod.rs"
        sed -n 's/^ *bin: "\([^"]*\)"\.to_string(),$/\1/p' "$ADAPTERS/codex/app_server/config.rs"
        sed -n 's/^ *bin: "\([^"]*\)"\.to_owned(),$/\1/p' "$ADAPTERS/opencode/session.rs"
        sed -n 's/^ *bin: "\([^"]*\)",$/\1/p' "$ADAPTERS/acp/rows.rs"
    } | sort -u | tr '\n' ' '
)
eval "$(grep '^AGENT_BINS=' "$ENTRY")"
shell_bins=$(printf '%s\n' $AGENT_BINS | sort -u | tr '\n' ' ')
check "AGENT_BINS matches the adapter sources" "$rust_bins" "$shell_bins"
check "every adapter source yields a binary" 6 "$(wc -w <<<"$rust_bins" | tr -d ' ')"

script="$work/probe.sh"
{
    echo 'set -eu'
    echo 'log() { echo "cctui-worker: $*"; }'
    echo 'RESULT_FILE="$WORK/result.json"'
    echo 'TERMINATION_LOG="$WORK/termination-log"'
    echo 'CALLBACK_SENT=0'
    echo 'dispatch_done_marker() { return 1; }'
    grep '^AGENT_BINS=\|^NO_AGENT_MSG=\|^ACP_AGENTS=' "$ENTRY"
    for fn in agent_installed is_dispatch phase_agent_probe adapter_kind fail_dispatch \
        require_agent send_callback worker_on_exit; do
        awk "/^${fn}\\(\\)/,/^}/" "$ENTRY"
    done
    echo 'AGENT_DIRS="$WORK/fixed"'
    echo 'WORKER_HOME="$WORK/home"'
    echo 'phase_agent_probe'
    echo 'echo "probe returned"'
    echo '[ "$MODE" = dispatch ] || exit 0'
    echo 'REPLY_URL=http://reply.invalid/cb'
    echo 'trap worker_on_exit EXIT'
    echo 'require_agent'
    echo 'echo "require_agent passed"'
} > "$script"

tools="$work/tools"
mkdir -p "$tools"
for t in jq cat; do ln -s "$(command -v "$t")" "$tools/$t"; done
printf '#!/bin/sh\necho "interactive bash"\n' > "$tools/bash"
printf '#!/bin/sh\nfor a in "$@"; do printf "%%s\\n" "$a"; done > "$WORK/callback"\n' > "$tools/curl"
chmod 0755 "$tools/bash" "$tools/curl"

setup() {
    _c="$work/$1"; shift
    rm -rf "$_c"; mkdir -p "$_c/agents" "$_c/fixed" "$_c/home/.local/bin"
    for _a in "$@"; do printf '#!/bin/sh\n' > "$_c/agents/$_a"; chmod 0755 "$_c/agents/$_a"; done
    echo "$_c"
}
run() {
    _c="$1"; shift
    env -i PATH="$tools:$_c/agents" WORK="$_c" MODE="${MODE:-boot}" "$@" /bin/sh "$script" 2>&1
}

echo "== 2. no agent, not a dispatch, no TTY: exit non-zero with the warning =="
c=$(setup none)
out=$(run "$c"); code=$?
check "exit 1" 1 "$code"
has "warning printed" "no agent CLI installed in this image; build FROM cctui-worker with one (see deploy/examples/worker-agents)" "$out"
check "never reaches the daemon" no "$(grep -q 'probe returned' <<<"$out" && echo yes || echo no)"

echo "== 3. no agent on a TTY: the warning, then bash =="
if command -v script >/dev/null 2>&1; then
    c=$(setup tty)
    out=$(SHELL=/bin/sh timeout 20 script -qec \
        "env -i PATH='$tools:$c/agents' WORK='$c' MODE=boot /bin/sh '$script'" /dev/null </dev/null 2>&1)
    code=$?
    check "exit 0 from bash" 0 "$code"
    has "warning printed" "no agent CLI installed in this image" "$out"
    has "bash exec'd" "interactive bash" "$out"
else
    bad "script(1) is needed to give the probe a TTY"
fi

echo "== 4. an installed agent is logged and boot continues =="
c=$(setup some claude goose)
out=$(run "$c"); code=$?
check "exit 0" 0 "$code"
has "agents logged" "agents: claude goose" "$out"
has "boot continues" "probe returned" "$out"

echo "== 5. agents are found where the daemon looks =="
c=$(setup elsewhere)
printf '#!/bin/sh\n' > "$c/home/.local/bin/codex"; chmod 0755 "$c/home/.local/bin/codex"
printf '#!/bin/sh\n' > "$c/fixed/opencode"; chmod 0755 "$c/fixed/opencode"
printf '#!/bin/sh\n' > "$c/agents/gemini"; chmod 0644 "$c/agents/gemini"
mkdir "$c/agents/qwen"
out=$(run "$c"); code=$?
has "~/.local/bin and the fixed dirs count" "agents: codex opencode" "$out"
check "a non-executable file or a directory does not" no \
    "$(grep -q 'gemini\|qwen' <<<"$out" && echo yes || echo no)"

echo "== 6. a dispatch on an agent-less image fails with a verdict and a callback =="
c=$(setup dispatch-none)
out=$(MODE=dispatch run "$c" TASK_PAYLOAD_JSON='{}' TASK_ID=T-1); code=$?
check "exit 1" 1 "$code"
has "probe defers to require_agent" "probe returned" "$out"
check "verdict status" failed "$(jq -r .status "$c/result.json" 2>/dev/null)"
check "verdict task_id" T-1 "$(jq -r .task_id "$c/result.json" 2>/dev/null)"
has "verdict names the cause" "no agent CLI installed in this image" "$(jq -r .error "$c/result.json" 2>/dev/null)"
has "termination message" "no agent CLI installed in this image" "$(cat "$c/termination-log" 2>/dev/null)"
cb=$(cat "$c/callback" 2>/dev/null)
has "callback posted to REPLY_URL" "http://reply.invalid/cb" "$cb"
has "callback carries the failed verdict" '"status":"failed"' "$cb"

echo "== 7. a kube dispatch (CCTUI_DISPATCH_ID only) counts as a dispatch =="
c=$(setup kube-none)
MODE=dispatch run "$c" CCTUI_DISPATCH_ID=d-1 >/dev/null; code=$?
check "exit 1" 1 "$code"
check "verdict written" failed "$(jq -r .status "$c/result.json" 2>/dev/null)"

echo "== 8. a dispatch whose adapter binary is missing fails naming the adapter =="
c=$(setup missing claude)
out=$(MODE=dispatch run "$c" TASK_PAYLOAD_JSON='{}' TASK_ADAPTER=codex); code=$?
check "exit 1" 1 "$code"
err=$(jq -r .error "$c/result.json" 2>/dev/null)
has "names the adapter" "adapter codex needs \`codex\`" "$err"
has "lists what is installed" "(installed: claude)" "$err"
c=$(setup missing-acp claude)
MODE=dispatch run "$c" TASK_PAYLOAD_JSON='{}' TASK_ADAPTER=qwen >/dev/null; code=$?
check "an ACP adapter is checked by its row's binary" "adapter qwen needs \`qwen\`" \
    "$(jq -r .error "$c/result.json" 2>/dev/null | cut -d, -f1)"

echo "== 9. a dispatch whose adapter is installed passes =="
for pair in ":claude" "claude-code:claude" "codex:codex" "opencode:opencode" "goose:goose"; do
    a=${pair%%:*}; b=${pair#*:}
    c=$(setup "ok-$b-${a:-unset}" "$b")
    out=$(MODE=dispatch run "$c" TASK_PAYLOAD_JSON='{}' ${a:+TASK_ADAPTER=$a}); code=$?
    check "adapter '${a:-unset}' with $b" 0 "$code"
    check "adapter '${a:-unset}' writes no verdict" no "$([ -e "$c/result.json" ] && echo yes || echo no)"
done

[ "$fail" -eq 0 ] || exit 1
echo "all agent-probe cases passed"
