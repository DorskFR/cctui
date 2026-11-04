#!/bin/sh
# Claude Code PreToolUse hook: asks cctui-guard (POST /check?dialect=claude)
# and blocks with exit 2, the reason on stderr. Claude runs the tool when a
# hook crashes, exits non-2 or prints invalid JSON, so everything that is not
# an explicit guard allow exits 2. An allow prints nothing: it adds no grant.
block() {
    printf '%s\n' "$1" >&2
    exit 2
}
trap 'block "cctui-guard shim: interrupted — failing closed"' HUP INT TERM

url=${CCTUI_GUARD_URL:-}
[ -n "$url" ] || block "cctui-guard shim: CCTUI_GUARD_URL is unset — failing closed"

input=$(cat) || block "cctui-guard shim: cannot read the hook input — failing closed"
body=$(printf '%s' "$input" | jq -ce '
    {tool_name: (.tool_name | select(type == "string" and . != "")),
     tool_input: (.tool_input // {})}' 2>/dev/null) \
    || block "cctui-guard shim: hook input is not a tool call — failing closed"

resp=$(printf '%s' "$body" | curl -sS -f --max-time "${CCTUI_GUARD_TIMEOUT:-10}" \
    -H 'content-type: application/json' --data-binary @- \
    "${url%/}/check?dialect=claude" 2>/dev/null) \
    || block "cctui-guard shim: guard unreachable or failing at $url — failing closed"

decision=$(printf '%s' "$resp" \
    | jq -er '.hookSpecificOutput.permissionDecision | strings' 2>/dev/null) \
    || block "cctui-guard shim: invalid guard response — failing closed"

case $decision in
    allow) exit 0 ;;
    deny | ask)
        reason=$(printf '%s' "$resp" \
            | jq -r '.hookSpecificOutput.permissionDecisionReason // empty' 2>/dev/null)
        block "${reason:-cctui-guard: blocked}"
        ;;
    *) block "cctui-guard shim: unknown guard decision '$decision' — failing closed" ;;
esac
