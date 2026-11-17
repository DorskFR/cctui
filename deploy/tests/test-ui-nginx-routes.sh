#!/usr/bin/env bash
# Needs docker.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
image="$(sed -n 's/^FROM \(nginx:[^ ]*\).*/\1/p' "$root/webui/Dockerfile" | tail -n1)"
tmp="$(mktemp -d)"
name="cctui-ui-routes-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT

# adapter-static layout: <route>.html beside a <route>/ dir with no index.html.
html="$tmp/html"
mkdir -p "$html/settings" "$html/cctuiverse"
page() { printf '<!doctype html><title>%s</title><script>1</script>\n' "$2" >"$html/$1"; }
page index.html shell
page settings.html settings
page settings/appearance.html settings-appearance
page cctuiverse/join.html cctuiverse-join
chmod -R a+rX "$html"

docker run -d --name "$name" -p 127.0.0.1::8080 \
  -e CCTUI_FRAME_ANCESTORS= -e 'NGINX_ENVSUBST_FILTER=^CCTUI_FRAME_ANCESTORS$' \
  -v "$root/webui/nginx.conf:/etc/nginx/templates/default.conf.template:ro" \
  -v "$html:/usr/share/nginx/html:ro" "$image" >/dev/null
origin="http://$(docker port "$name" 8080/tcp | head -n1)"
for _ in $(seq 60); do curl -sf -o /dev/null "$origin/" && break; sleep 0.5; done

fail=0
check() {
  local path="$1" want="$2" code location body
  code="$(curl -sS -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' "$origin$path")"
  location="$(grep -i '^location:' "$tmp/headers" | tr -d '\r' || true)"
  body="$(sed -n 's/.*<title>\(.*\)<\/title>.*/\1/p' "$tmp/body")"
  if [ "$code" = 200 ] && [ -z "$location" ] && [ "$body" = "$want" ]; then
    printf '  ok   %s -> %s\n' "$path" "$want"
  else
    printf '  FAIL %s: got [%s %s %s] want [200 %s]\n' "$path" "$code" "$body" "$location" "$want"
    fail=1
  fi
}
check / shell
check /settings settings
check /settings/ shell
check /settings/appearance settings-appearance
check /settings/appearance/ shell
check /settings/not-a-page shell
check /cctuiverse shell
check /cctuiverse/ shell
check /cctuiverse/join cctuiverse-join
check /sessions/abc shell

if docker logs "$name" 2>&1 | grep -E '\[(emerg|error)\]'; then fail=1; fi
exit "$fail"
