#!/usr/bin/env bash
# Usage: pin-macho-uuid.sh <thin-64-bit-mach-o> <uuid>
# macOS keys Local Network grants on LC_UUID; re-sign the binary afterwards.
set -euo pipefail

bin="$1" uuid="$2"
python3 -I - "$bin" "$uuid" <<'PY'
import struct, sys, uuid as uuidlib

path, raw = sys.argv[1], sys.argv[2]
try:
    want = uuidlib.UUID(raw).bytes
except ValueError:
    sys.exit(f"not a uuid: {raw}")
data = bytearray(open(path, "rb").read())
if len(data) < 32 or struct.unpack_from("<I", data, 0)[0] != 0xFEEDFACF:
    sys.exit(f"{path}: not a thin 64-bit little-endian Mach-O")
ncmds, sizeofcmds = struct.unpack_from("<II", data, 16)
off, end, hits = 32, 32 + sizeofcmds, []
for _ in range(ncmds):
    cmd, size = struct.unpack_from("<II", data, off)
    if size < 8 or off + size > end:
        sys.exit(f"{path}: malformed load command at {off}")
    if cmd == 0x1B:
        hits.append(off + 8)
    off += size
if len(hits) != 1:
    sys.exit(f"{path}: expected one LC_UUID, found {len(hits)}")
data[hits[0]:hits[0] + 16] = want
open(path, "wb").write(data)
print(f"{path}: LC_UUID = {uuidlib.UUID(bytes=want)}")
PY
