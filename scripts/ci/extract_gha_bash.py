#!/usr/bin/env python3
"""Extract bash ``run: |`` blocks from GitHub Actions YAML for ``bash -n``.

This is the cheap check that would have caught the missing ``fi`` in
``.github/workflows/release.yml`` Apple-signing detection (salvaged in v3.20.12).

Only extracts a block when:
  - the same step sets ``shell: bash`` / ``shell: sh``, or
  - there is no explicit shell and the body looks like bash (``set -euo``,
    ``if [``, ``if [[``).

PowerShell / cmd steps are skipped.
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

WORKFLOW_DIR = Path(".github/workflows")


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _looks_bash(body: str) -> bool:
    head = body.lstrip()
    return (
        head.startswith("set -")
        or "if [" in body
        or "if [[" in body
        or "#!/usr/bin/env bash" in body
        or "#!/bin/bash" in body
    )


def _shell_for_run(lines: list[str], run_index: int, run_indent: int) -> str | None:
    """Walk backwards within the same step for an explicit ``shell:``."""
    for j in range(run_index - 1, -1, -1):
        raw = lines[j]
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        ind = _indent(raw)
        stripped = raw.lstrip(" ")
        if ind <= run_indent and stripped.startswith("- "):
            break
        if ind < run_indent and not stripped.startswith("#"):
            break
        if stripped.startswith("shell:"):
            value = stripped.split(":", 1)[1].strip().strip("'\"")
            return value.split("#", 1)[0].strip().strip("'\"")
    return None


def extract_blocks(path: Path) -> list[tuple[int, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    blocks: list[tuple[int, str]] = []
    i = 0
    while i < len(lines):
        raw = lines[i]
        stripped = raw.lstrip(" ")
        if stripped.startswith("run:") and "|" in stripped.split(":", 1)[1]:
            rest = stripped.split(":", 1)[1].strip()
            if rest in ("|", "|-", "|+"):
                run_indent = _indent(raw)
                shell = _shell_for_run(lines, i, run_indent)
                i += 1
                body: list[str] = []
                start_line = i + 1
                body_indent: int | None = None
                while i < len(lines):
                    line = lines[i]
                    if line.strip() == "":
                        body.append("")
                        i += 1
                        continue
                    cur = _indent(line)
                    if body_indent is None:
                        if cur <= run_indent:
                            break
                        body_indent = cur
                    if cur < body_indent:
                        break
                    body.append(line[body_indent:])
                    i += 1
                text = "\n".join(body).rstrip() + "\n"
                skip_shells = {"pwsh", "powershell", "cmd", "python"}
                if shell in skip_shells:
                    continue
                if shell in {"bash", "sh"} or (shell is None and _looks_bash(text)):
                    blocks.append((start_line, text))
                continue
        i += 1
    return blocks


def check_workflows(root: Path) -> int:
    workflow_dir = root / WORKFLOW_DIR
    files = sorted(workflow_dir.glob("*.yml")) + sorted(workflow_dir.glob("*.yaml"))
    if not files:
        print(f"No workflow files under {workflow_dir}", file=sys.stderr)
        return 1
    fail = 0
    for path in files:
        rel = path.relative_to(root)
        blocks = extract_blocks(path)
        print(f"{rel}: {len(blocks)} bash run block(s)")
        for start_line, body in blocks:
            with tempfile.NamedTemporaryFile("w", suffix=".sh", delete=False, encoding="utf-8") as tmp:
                tmp.write(body)
                tmp_path = tmp.name
            try:
                proc = subprocess.run(
                    ["bash", "-n", tmp_path],
                    capture_output=True,
                    text=True,
                    check=False,
                )
            finally:
                os.unlink(tmp_path)
            if proc.returncode != 0:
                fail = 1
                print(f"  FAIL {rel}:{start_line}", file=sys.stderr)
                if proc.stderr:
                    print(proc.stderr, file=sys.stderr, end="")
                if proc.stdout:
                    print(proc.stdout, file=sys.stderr, end="")
            else:
                print(f"  ok   {rel}:{start_line}")
    return fail


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="bash -n extracted blocks")
    parser.add_argument("--root", default=".")
    args = parser.parse_args()
    root = Path(args.root).resolve()
    os.chdir(root)
    if args.check:
        return check_workflows(root)
    for path in sorted((root / WORKFLOW_DIR).glob("*.yml")):
        for start_line, body in extract_blocks(path):
            print(f"# {path.relative_to(root)}:{start_line}")
            print(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
