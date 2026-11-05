#!/usr/bin/env bash
# Loads the OpenCode guard plugin in node and drives its tool.execute.before
# against a fake guard: only an explicit allow lets the call through; every
# deny, ask, error and malformed exchange throws, the guard reason verbatim.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
plugin="$here/../guard-shims/opencode/plugins/cctui-guard.js"
. "$here/fake-guard.sh"

node_bin="${NODE:-$(command -v node || command -v nodejs || true)}"
[ -n "$node_bin" ] || { echo "FAIL node is required for this test"; exit 1; }

fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$3] lacks [$2]" ;; esac; }

tmp="$(mktemp -d)"
trap 'stop_fake_guard; rm -rf "$tmp"' EXIT
start_fake_guard "$tmp"

cp "$plugin" "$tmp/plugin.mjs"
cat > "$tmp/drive.mjs" <<'JS'
const mod = await import(process.argv[2]);
const names = Object.keys(mod);
if (names.length !== 1) {
  console.log(`EXPORTS:${names.join(",")}`);
  process.exit(0);
}
const hooks = await mod[names[0]]({});
const tool = process.argv[3] === "-" ? undefined : process.argv[3];
const args = JSON.parse(process.argv[4]);
try {
  await hooks["tool.execute.before"]({ tool, sessionID: "s1", callID: "c1" }, { args });
  process.stdout.write("ALLOW");
} catch (e) {
  process.stdout.write(`DENY:${e.message}`);
}
JS

# run TOOL ARGS_JSON [URL]: sets res.
run() {
    res=$(CCTUI_GUARD_URL="${3-$GUARD_URL}" CCTUI_GUARD_TIMEOUT=1 \
        "$node_bin" "$tmp/drive.mjs" "$tmp/plugin.mjs" "$1" "$2" 2>&1)
}

reason="[Step 2] 'git push' is disallowed in this step — run the tests first
then: curl -s \$CCTUI_GUARD_URL/transition"
deny=$(jq -nc --arg r "$reason" '{allow:false,reason:$r}')
ask=$(jq -nc --arg r "$reason" '{allow:false,ask:true,reason:$r}')
push='{"command":"git push","description":"push"}'

echo "== plugin exports exactly one plugin =="
run bash "$push"
case "$res" in EXPORTS:*) bad "extra exports: $res" ;; *) ok "single export" ;; esac

echo "== allow passes =="
respond 200 '{"allow":true}'
run bash "$push"
check "allowed" "ALLOW" "$res"
check "posts the opencode dialect with Claude tool names" \
    "/check?dialect=opencode {\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git push\",\"description\":\"push\"}}" \
    "$(last_request)"
run edit '{"filePath":"/w/a.rs","oldString":"a","newString":"b"}'
check "file tools carry file_path" "Edit /w/a.rs" \
    "$(last_request | sed 's/^[^ ]* //' | jq -r '"\(.tool_name) \(.tool_input.file_path)"')"
run github_create_issue '{"title":"t"}'
check "MCP tools pass through" "github_create_issue" \
    "$(last_request | sed 's/^[^ ]* //' | jq -r .tool_name)"

echo "== deny blocks with the guard reason verbatim =="
respond 200 "$deny"
run bash "$push"
check "denied with the reason" "DENY:$reason" "$res"

echo "== ask blocks =="
respond 200 "$ask"
run bash "$push"
check "denied with the reason" "DENY:$reason" "$res"

echo "== guard unreachable blocks =="
run bash "$push" "http://127.0.0.1:1"
has "denied, failing closed" "failing closed" "$res"
check "is a deny" "DENY" "${res%%:*}"

echo "== guard returns garbage blocks =="
for body in 'not json' '{"decision":"allow"}' '{"allow":"true"}' 'null'; do
    respond 200 "$body"
    run bash "$push"
    check "deny on [$body]" "DENY" "${res%%:*}"
done
respond 500 '{"allow":true}'
run bash "$push"
check "deny on HTTP 500 even with an allow body" "DENY" "${res%%:*}"

echo "== hung guard blocks after the timeout =="
respond hang ''
run bash "$push"
check "deny" "DENY" "${res%%:*}"

echo "== invalid tool call blocks =="
respond 200 '{"allow":true}'
run - "$push"
check "deny without a tool name" "DENY" "${res%%:*}"
run bash 'null'
check "null args still asks the guard and passes" "ALLOW" "$res"
check "null args become {}" "{}" "$(last_request | sed 's/^[^ ]* //' | jq -c .tool_input)"

echo "== CCTUI_GUARD_URL unset blocks =="
run bash "$push" ""
has "names the variable" "CCTUI_GUARD_URL" "$res"
check "is a deny" "DENY" "${res%%:*}"

[ "$fail" -eq 0 ] || exit 1
echo "all opencode guard-plugin cases passed"
