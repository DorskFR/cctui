#!/usr/bin/env bash
# Runs the Codex and Claude PreToolUse guard shims against a fake guard: only an
# explicit allow exits 0; every deny, ask, error and malformed exchange exits 2
# with a reason on stderr.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
shims="$here/../guard-shims"
. "$here/fake-guard.sh"

fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$3] lacks [$2]" ;; esac; }

tmp="$(mktemp -d)"
trap 'stop_fake_guard; rm -rf "$tmp"' EXIT
start_fake_guard "$tmp"

# run SHIM STDIN [URL]: sets rc, out, err.
run() {
    printf '%s' "$2" | CCTUI_GUARD_URL="${3-$GUARD_URL}" CCTUI_GUARD_TIMEOUT=1 \
        "$1" > "$tmp/out" 2> "$tmp/err"
    rc=$?
    out=$(cat "$tmp/out"); err=$(cat "$tmp/err")
}

reason="[Step 2] 'git push' is disallowed in this step — run the tests first
then: curl -s \$CCTUI_GUARD_URL/transition"
bash_call='{"session_id":"s1","tool_name":"Bash","tool_input":{"command":"git push"}}'

codex_allow='{"decision":"allow"}'
codex_deny=$(jq -nc --arg r "$reason" '{decision:"block",reason:$r}')
codex_ask=$(jq -nc --arg r "$reason" '{decision:"ask",reason:$r}')
claude_allow='{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}'
claude_deny=$(jq -nc --arg r "$reason" \
    '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}')
claude_ask=$(jq -nc --arg r "$reason" \
    '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"ask",permissionDecisionReason:$r}}')

for dialect in codex claude; do
    shim="$shims/$dialect/pretool.sh"
    allow_var="${dialect}_allow"; deny_var="${dialect}_deny"; ask_var="${dialect}_ask"

    echo "== $dialect: allow passes =="
    respond 200 "${!allow_var}"
    run "$shim" "$bash_call"
    check "exit 0" 0 "$rc"
    check "prints nothing" "" "$out"
    check "posts to the $dialect dialect" \
        "/check?dialect=$dialect {\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git push\"}}" \
        "$(last_request)"

    echo "== $dialect: deny blocks with the guard reason verbatim =="
    respond 200 "${!deny_var}"
    run "$shim" "$bash_call"
    check "exit 2" 2 "$rc"
    check "stderr is the reason" "$reason" "$err"

    echo "== $dialect: ask blocks (never fails open) =="
    respond 200 "${!ask_var}"
    run "$shim" "$bash_call"
    check "exit 2" 2 "$rc"
    check "stderr is the reason" "$reason" "$err"

    echo "== $dialect: guard unreachable blocks =="
    run "$shim" "$bash_call" "http://127.0.0.1:1"
    check "exit 2" 2 "$rc"
    has "says failing closed" "failing closed" "$err"

    echo "== $dialect: guard returns garbage blocks =="
    respond 200 'not json at all'
    run "$shim" "$bash_call"
    check "exit 2 on non-JSON" 2 "$rc"
    respond 200 '{"verdict":"allow"}'
    run "$shim" "$bash_call"
    check "exit 2 on an unknown shape" 2 "$rc"
    respond 200 '{"decision":"maybe","hookSpecificOutput":{"permissionDecision":"maybe"}}'
    run "$shim" "$bash_call"
    check "exit 2 on an unknown decision" 2 "$rc"
    respond 500 "${!allow_var}"
    run "$shim" "$bash_call"
    check "exit 2 on HTTP 500 even with an allow body" 2 "$rc"

    echo "== $dialect: hung guard blocks after the timeout =="
    respond hang ''
    run "$shim" "$bash_call"
    check "exit 2" 2 "$rc"

    echo "== $dialect: invalid stdin blocks without asking the guard =="
    respond 200 "${!allow_var}"
    : > "$tmp/requests.log"
    run "$shim" 'garbage {'
    check "exit 2 on non-JSON stdin" 2 "$rc"
    run "$shim" '{"tool_input":{"command":"ls"}}'
    check "exit 2 without tool_name" 2 "$rc"
    run "$shim" ''
    check "exit 2 on empty stdin" 2 "$rc"
    check "guard never asked" "" "$(cat "$tmp/requests.log")"

    echo "== $dialect: CCTUI_GUARD_URL unset blocks =="
    run "$shim" "$bash_call" ""
    check "exit 2" 2 "$rc"
    has "names the variable" "CCTUI_GUARD_URL" "$err"
done

echo "== codex: apply_patch and MCP tool names reach the guard verbatim =="
respond 200 "$codex_allow"
for tool in apply_patch mcp__github__create_pull_request; do
    run "$shims/codex/pretool.sh" "{\"tool_name\":\"$tool\",\"tool_input\":{\"command\":\"x\"}}"
    check "$tool allowed" 0 "$rc"
    check "$tool forwarded" "$tool" "$(last_request | sed 's/^[^ ]* //' | jq -r .tool_name)"
done
respond 200 "$codex_deny"
run "$shims/codex/pretool.sh" '{"tool_name":"mcp__github__merge","tool_input":{}}'
check "MCP deny exits 2" 2 "$rc"

[ "$fail" -eq 0 ] || exit 1
echo "all guard-shim hook cases passed"
