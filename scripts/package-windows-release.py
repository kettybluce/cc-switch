#!/usr/bin/env python3
"""Package Windows x64 MSI + Portable zip using the names release.yml publishes.

Used by:
  - .github/workflows/release.yml (real Tauri outputs on windows-2022)
  - scripts/ci/dry-run-windows-portable.sh (tiny dummy files; no version bump)

SCHEMA 18 / no installer semantics change: portable mode is still
``portable.ini`` next to ``cc-switch.exe`` (see is_portable_mode).
"""
from __future__ import annotations

import argparse
import shutil
import sys
import zipfile
from pathlib import Path

PORTABLE_INI = "# CC Switch portable build marker\nportable=true\n"


def find_msi(target_root: Path) -> Path:
    found: list[Path] = []
    msi_dir = target_root / "bundle" / "msi"
    if msi_dir.is_dir():
        found = sorted(p for p in msi_dir.rglob("*.msi") if p.is_file())
    if not found:
        bundle = target_root / "bundle"
        if bundle.is_dir():
            found = sorted(p for p in bundle.rglob("*.msi") if p.is_file())
    if not found:
        raise SystemExit("No Windows x64 MSI installer found")
    return found[0]


def find_exe(target_root: Path) -> Path:
    candidates = [
        target_root / "cc-switch.exe",
        target_root.parent / "x86_64-pc-windows-msvc" / "release" / "cc-switch.exe",
    ]
    for path in candidates:
        if path.is_file():
            return path
    raise SystemExit("Portable exe not found")


def zip_dir_contents(src: Path, dest: Path) -> None:
    """Zip files at archive root (exe + portable.ini), not a parent folder."""
    if dest.exists():
        dest.unlink()
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for child in sorted(src.iterdir()):
            if child.is_file():
                zf.write(child, arcname=child.name)


def verify_zip(zip_path: Path, min_bytes: int) -> None:
    size = zip_path.stat().st_size
    if size < min_bytes:
        raise SystemExit(f"Portable zip too small ({size} < {min_bytes}): {zip_path}")
    with zipfile.ZipFile(zip_path) as zf:
        names = zf.namelist()
        basenames = {name.rstrip("/").split("/")[-1] for name in names}
        if "cc-switch.exe" not in basenames:
            raise SystemExit(f"Portable zip missing cc-switch.exe: {names}")
        if "portable.ini" not in basenames:
            raise SystemExit(f"Portable zip missing portable.ini: {names}")
        if any(name.startswith("CC-Switch-Portable/") for name in names):
            raise SystemExit(f"Portable zip must not nest under CC-Switch-Portable/: {names}")
        ini_name = next(name for name in names if name.rstrip("/").endswith("portable.ini"))
        ini = zf.read(ini_name)
        if ini.startswith(b"\xef\xbb\xbf"):
            raise SystemExit("portable.ini has a UTF-8 BOM; write UTF-8 without BOM")
        if b"portable=true" not in ini:
            raise SystemExit("portable.ini missing portable=true")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--version",
        required=True,
        help="Release tag, e.g. v3.20.13 (asset names use the tag, not package.json)",
    )
    parser.add_argument("--target-root", default="src-tauri/target/release")
    parser.add_argument("--out-dir", default="release-assets")
    parser.add_argument(
        "--min-bytes",
        type=int,
        default=1024,
        help="Minimum size for MSI and zip (use 16 in CI dummy dry-run)",
    )
    args = parser.parse_args()

    target_root = Path(args.target_root)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    msi = find_msi(target_root)
    dest_msi = out_dir / f"CC-Switch-{args.version}-Windows.msi"
    shutil.copy2(msi, dest_msi)
    print(f"Installer copied: {dest_msi.name}")
    msi_size = dest_msi.stat().st_size
    if msi_size < args.min_bytes:
        raise SystemExit(f"MSI too small ({msi_size} < {args.min_bytes}): {dest_msi}")

    sig = Path(str(msi) + ".sig")
    if sig.is_file():
        shutil.copy2(sig, out_dir / f"{dest_msi.name}.sig")
        print(f"Signature copied: {dest_msi.name}.sig")
    else:
        print(f"WARNING: Signature not found for {msi.name}", file=sys.stderr)

    exe = find_exe(target_root)
    portable_dir = out_dir / "CC-Switch-Portable"
    if portable_dir.exists():
        shutil.rmtree(portable_dir)
    portable_dir.mkdir(parents=True)
    shutil.copy2(exe, portable_dir / exe.name)
    (portable_dir / "portable.ini").write_bytes(PORTABLE_INI.encode("utf-8"))
    portable_zip = out_dir / f"CC-Switch-{args.version}-Windows-Portable.zip"
    zip_dir_contents(portable_dir, portable_zip)
    shutil.rmtree(portable_dir)
    verify_zip(portable_zip, args.min_bytes)
    print(f"Windows portable zip created: {portable_zip.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
