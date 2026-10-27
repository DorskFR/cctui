#!/usr/bin/env bash
# Needs docker.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
image="$(sed -n 's/^FROM \(nginx:[^ ]*\).*/\1/p' "$root/webui/Dockerfile" | tail -n1)"
tmp="$(mktemp -d)"
name="cctui-ui-mime-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT

mkdir -p "$tmp/html/vad"
echo '<!doctype html><script>1</script>' >"$tmp/html/index.html"
files=(ort-wasm-simd-threaded.mjs ort-wasm-simd-threaded.wasm silero_vad_v5.onnx vad.worklet.bundle.min.js)
for f in "${files[@]}"; do echo x >"$tmp/html/vad/$f"; done
printf '["%s"]\n' "$(IFS=,; echo "${files[*]}" | sed 's/,/","/g')" >"$tmp/html/vad/manifest.json"
echo '{}' >"$tmp/html/app.webmanifest"
echo '{}' >"$tmp/html/app.js.map"
chmod -R a+rX "$tmp/html"

docker run -d --name "$name" -p 127.0.0.1::8080 \
  -e CCTUI_FRAME_ANCESTORS= -e 'NGINX_ENVSUBST_FILTER=^CCTUI_FRAME_ANCESTORS$' \
  -v "$root/webui/nginx.conf:/etc/nginx/templates/default.conf.template:ro" \
  -v "$tmp/html:/usr/share/nginx/html:ro" "$image" >/dev/null
origin="http://$(docker port "$name" 8080/tcp | head -n1)"
for _ in $(seq 60); do curl -sf -o /dev/null "$origin/" && break; sleep 0.5; done

fail=0
check() {
  local got
  got="$(curl -sS --head -o /dev/null -w '%{http_code} %{content_type}' "$origin$1")"
  if [ "$got" = "200 $2" ]; then printf '  ok   %s: %s\n' "$1" "$2"
  else printf '  FAIL %s: got [%s] want [200 %s]\n' "$1" "$got" "$2"; fail=1; fi
}
check /vad/ort-wasm-simd-threaded.mjs text/javascript
check /vad/ort-wasm-simd-threaded.wasm application/wasm
check /vad/silero_vad_v5.onnx application/octet-stream
check /app.webmanifest application/manifest+json
check /app.js.map application/json

"$root/scripts/check-vad-assets.sh" "$origin" || fail=1
if docker logs "$name" 2>&1 | grep -E '\[(emerg|error)\]'; then fail=1; fi
exit "$fail"
