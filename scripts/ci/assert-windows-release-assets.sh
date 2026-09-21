#!/usr/bin/env bash
# Gate GitHub Release publish on Windows x64 Portable + MSI names.
# macOS zip/dmg is best-effort (unsigned macOS must not block Portable).
set -euo pipefail

TAG="${1:?usage: assert-windows-release-assets.sh <tag> [dir] [min-bytes]}"
DIR="${2:-release-assets}"
MIN_BYTES="${3:-${MIN_BYTES:-1024}}"

missing=0
msi="${DIR}/CC-Switch-${TAG}-Windows.msi"
portable="${DIR}/CC-Switch-${TAG}-Windows-Portable.zip"

if [ ! -f "$msi" ]; then
  echo "Windows x64 MSI is required; refusing to publish." >&2
  missing=1
elif [ "$(wc -c < "$msi")" -lt "$MIN_BYTES" ]; then
  echo "Windows x64 MSI is too small; refusing to publish: $msi" >&2
  missing=1
fi

if [ ! -f "$portable" ]; then
  echo "Windows x64 Portable zip is required; refusing to publish." >&2
  missing=1
else
  python3 - "$portable" "$MIN_BYTES" <<'PY'
import sys
import zipfile
from pathlib import Path

zip_path = Path(sys.argv[1])
min_bytes = int(sys.argv[2])
size = zip_path.stat().st_size
if size < min_bytes:
    print(f"Windows x64 Portable zip is too small ({size} < {min_bytes}): {zip_path}", file=sys.stderr)
    sys.exit(1)
with zipfile.ZipFile(zip_path) as zf:
    names = zf.namelist()
    basenames = {name.rstrip("/").split("/")[-1] for name in names}
    if "cc-switch.exe" not in basenames or "portable.ini" not in basenames:
        print(f"Portable zip must contain cc-switch.exe and portable.ini: {names}", file=sys.stderr)
        sys.exit(1)
    if any(name.startswith("CC-Switch-Portable/") for name in names):
        print(f"Portable zip must not nest under CC-Switch-Portable/: {names}", file=sys.stderr)
        sys.exit(1)
    ini_name = next(name for name in names if name.rstrip("/").endswith("portable.ini"))
    ini = zf.read(ini_name)
    if ini.startswith(b"\xef\xbb\xbf"):
        print("portable.ini has a UTF-8 BOM; write UTF-8 without BOM", file=sys.stderr)
        sys.exit(1)
    if b"portable=true" not in ini:
        print("portable.ini missing portable=true", file=sys.stderr)
        sys.exit(1)
print(f"Portable zip members ok: {names}")
PY
fi

if [ ! -f "${DIR}/CC-Switch-${TAG}-macOS.zip" ] && [ ! -f "${DIR}/CC-Switch-${TAG}-macOS.dmg" ]; then
  echo "⚠️ macOS zip/dmg missing (unsigned build may have failed); publishing Windows anyway." >&2
fi

ls -la "$DIR" || true
exit "$missing"
