#!/usr/bin/env bash
# Usage: check-vad-assets.sh <origin>; extra curl flags in CHECK_VAD_CURL_ARGS.
set -euo pipefail

origin="${1:?usage: check-vad-assets.sh <origin>}"
origin="${origin%/}"
read -r -a extra <<<"${CHECK_VAD_CURL_ARGS:-}"
REQUIRED="ort-wasm-simd-threaded.mjs ort-wasm-simd-threaded.wasm silero_vad_v5.onnx vad.worklet.bundle.min.js"

fail=0
ok() { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fail=1; }

probe() {
  curl -sS --max-time 20 --head -o /dev/null -w '%{http_code} %{content_type}\n' ${extra[@]+"${extra[@]}"} "$1" || echo "000 -"
}

expected_type() {
  case "$1" in
    *.mjs | *.js) echo '^(text|application)/javascript(;.*)?$' ;;
    *.wasm) echo '^application/wasm$' ;;
    *.onnx) echo '^application/octet-stream$' ;;
    *.json) echo '^application/json(;.*)?$' ;;
    *) echo '.' ;;
  esac
}

manifest="$(curl -sS --max-time 20 -f ${extra[@]+"${extra[@]}"} "$origin/vad/manifest.json" || true)"
listed=""
[[ "$manifest" =~ ^[[:space:]]*\[ ]] && listed="$(printf '%s' "$manifest" | grep -oE '"[^"/]+"' | tr -d '"' || true)"
[ -n "$listed" ] && ok "manifest lists $(printf '%s\n' "$listed" | wc -l | tr -d ' ') file(s)" \
  || bad "$origin/vad/manifest.json is missing or empty"

for f in $(printf '%s\n' manifest.json $REQUIRED $listed | sort -u); do
  read -r code type < <(probe "$origin/vad/$f")
  want="$(expected_type "$f")"
  if [ "$code" = 200 ] && [[ "$type" =~ $want ]]; then ok "$f: $type"
  else bad "$f: got $code $type, want 200 matching $want"; fi
done

read -r code type < <(probe "$origin/vad/__missing__.mjs")
if [ "$code" = 404 ]; then ok "missing /vad/ file is 404"
else bad "missing /vad/ file answered $code $type, want 404"; fi

csp="$(curl -sS --max-time 20 -D - -o /dev/null ${extra[@]+"${extra[@]}"} "$origin/" | tr -d '\r' \
  | sed -n 's/^[Cc]ontent-[Ss]ecurity-[Pp]olicy: *//p')"
script_src="$(printf '%s' "$csp" | tr ';' '\n' | grep -E '^ *script-src ' || true)"
if [ -z "$script_src" ] || [[ "$script_src" == *"'wasm-unsafe-eval'"* || "$script_src" == *"'unsafe-eval'"* ]]; then
  ok "page CSP allows WebAssembly"
else bad "page CSP script-src lacks 'wasm-unsafe-eval': $script_src"; fi

media_src="$(printf '%s' "$csp" | tr ';' '\n' | grep -E '^ *media-src ' || true)"
if [ -z "$csp" ] || [[ "$media_src" == *"data:"* && "$media_src" == *"blob:"* ]]; then
  ok "page CSP lets voice audio play (media-src data: blob:)"
else bad "page CSP media-src must allow data: and blob: for spoken replies: ${media_src:-<none>}"; fi

exit "$fail"
