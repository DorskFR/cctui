#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/pin-macho-uuid.sh"
dir="$(mktemp -d)"
trap 'rm -rf -- "${dir:?}"' EXIT

failures=0
check() {
  if [ "$2" = "$3" ]; then echo "ok   — $1"; else
    echo "FAIL — $1: want '$2' got '$3'"; failures=$((failures + 1))
  fi
}

fake() {
  python3 -I - "$1" "$2" <<'PY'
import struct, sys
uuids = int(sys.argv[2])
cmds = struct.pack("<II", 0x19, 16) + b"\0" * 8
cmds += (struct.pack("<II", 0x1B, 24) + bytes(range(16))) * uuids
hdr = struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1 + uuids, len(cmds), 0, 0)
open(sys.argv[1], "wb").write(hdr + cmds + b"payload")
PY
}
uuid_hex() { python3 -I -c 'import sys;print(open(sys.argv[1],"rb").read()[56:72].hex())' "$1"; }

want=7d3f0c52-1a4e-4b8e-9b61-2f0a6c5e9d13
fake "$dir/a" 1
before="$(md5sum < "$dir/a")"
bash "$script" "$dir/a" "$want" > /dev/null
check "LC_UUID is overwritten" "${want//-/}" "$(uuid_hex "$dir/a")"
check "size is unchanged" "79" "$(wc -c < "$dir/a" | tr -d ' ')"
check "payload is untouched" "payload" "$(tail -c 7 "$dir/a")"
bash "$script" "$dir/a" "$want" > /dev/null
check "re-pinning is idempotent" "${want//-/}" "$(uuid_hex "$dir/a")"
[ "$before" != "$(md5sum < "$dir/a")" ] || { echo "FAIL — file did not change"; failures=$((failures + 1)); }

refused() { if bash "$script" "$@" > /dev/null 2>&1; then echo refused=no; else echo refused=yes; fi; }
printf 'not a binary' > "$dir/txt"
check "non-Mach-O is refused" refused=yes "$(refused "$dir/txt" "$want")"
fake "$dir/b" 1
check "bad uuid is refused" refused=yes "$(refused "$dir/b" nope)"
fake "$dir/c" 2
check "two LC_UUIDs are refused" refused=yes "$(refused "$dir/c" "$want")"
fake "$dir/d" 0
check "missing LC_UUID is refused" refused=yes "$(refused "$dir/d" "$want")"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)"; exit 1; }
