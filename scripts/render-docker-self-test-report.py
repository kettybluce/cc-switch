#!/usr/bin/env python3
"""Render a Chinese markdown report from cargo test output.

Used by scripts/docker-self-test.sh after every Docker (or host-fallback)
backend self-test run. SCHEMA_VERSION is recorded as 18; this script does
not bump it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import re
import sys
from pathlib import Path
from typing import Iterable

RESULT_RE = re.compile(
    r"^test result: (ok|FAILED)\. "
    r"(\d+) passed; (\d+) failed; (\d+) ignored; "
    r"(\d+) measured; (\d+) filtered out"
    r"(?:; finished in ([0-9.]+)s)?"
)
FAILURE_STDOUT_RE = re.compile(r"^---- (.+) stdout ----$")
FAILURE_LIST_RE = re.compile(r"^    (\S.+)$")
RUNNING_RE = re.compile(r"^\s*Running (unittests |tests/|Doc-tests )(.+)$")
COMPILE_ERR_RE = re.compile(r"^error(\[E\d+\])?:")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


def clean_line(line: str) -> str:
    return ANSI_RE.sub("", line.replace("\r", "")).rstrip()


def parse_cargo_log(text: str) -> dict:
    lines = text.splitlines()
    targets: list[dict] = []
    current: dict | None = None
    failed_snippets: dict[str, list[str]] = {}
    failed_names: list[str] = []
    compile_errors: list[str] = []
    in_snippet: str | None = None
    snippet_buf: list[str] = []
    in_failure_list = False

    def flush_snippet() -> None:
        nonlocal in_snippet, snippet_buf
        if in_snippet:
            failed_snippets.setdefault(in_snippet, [])
            failed_snippets[in_snippet].extend(snippet_buf)
            if in_snippet not in failed_names:
                failed_names.append(in_snippet)
        in_snippet = None
        snippet_buf = []

    def ensure_target(name: str) -> dict:
        nonlocal current
        current = {
            "name": name,
            "status": "",
            "passed": 0,
            "failed": 0,
            "ignored": 0,
            "measured": 0,
            "filtered": 0,
            "secs": None,
        }
        targets.append(current)
        return current

    for raw in lines:
        line = clean_line(raw)
        running = RUNNING_RE.match(line)
        if running:
            flush_snippet()
            in_failure_list = False
            kind, rest = running.group(1), running.group(2).strip()
            if kind.startswith("unittests"):
                label = f"lib {rest.split()[0]}"
            elif kind.startswith("Doc-tests"):
                label = f"doc-tests {rest}"
            else:
                label = rest.split()[0]
            ensure_target(label)
            continue

        result = RESULT_RE.search(line)
        if result:
            flush_snippet()
            in_failure_list = False
            if current is None:
                ensure_target("(unnamed target)")
            current["status"] = result.group(1)
            current["passed"] = int(result.group(2))
            current["failed"] = int(result.group(3))
            current["ignored"] = int(result.group(4))
            current["measured"] = int(result.group(5))
            current["filtered"] = int(result.group(6))
            current["secs"] = result.group(7)
            continue

        stdout_h = FAILURE_STDOUT_RE.match(line)
        if stdout_h:
            flush_snippet()
            in_failure_list = False
            in_snippet = stdout_h.group(1)
            snippet_buf = []
            continue

        if line == "failures:":
            flush_snippet()
            in_failure_list = True
            continue

        if in_snippet is not None:
            if line.startswith("---- ") and line.endswith(" stdout ----"):
                flush_snippet()
                in_snippet = FAILURE_STDOUT_RE.match(line).group(1)  # type: ignore[union-attr]
                snippet_buf = []
            else:
                snippet_buf.append(line)
            continue

        if in_failure_list:
            listed = FAILURE_LIST_RE.match(line)
            if listed:
                name = listed.group(1).strip()
                if name not in failed_names:
                    failed_names.append(name)
                continue
            if line.strip() == "":
                continue
            in_failure_list = False

        if COMPILE_ERR_RE.match(line) or line.startswith("error: could not compile"):
            compile_errors.append(line)

    flush_snippet()

    totals = {
        "passed": sum(t["passed"] for t in targets),
        "failed": sum(t["failed"] for t in targets),
        "ignored": sum(t["ignored"] for t in targets),
        "measured": sum(t["measured"] for t in targets),
        "filtered": sum(t["filtered"] for t in targets),
        "status": "FAILED"
        if any(t["status"] == "FAILED" for t in targets) or compile_errors
        else ("ok" if targets else "unknown"),
    }
    return {
        "targets": targets,
        "totals": totals,
        "failed_names": failed_names,
        "failed_snippets": failed_snippets,
        "compile_errors": compile_errors,
    }


def format_duration(seconds: int | float | None) -> str:
    if seconds is None:
        return "未知"
    total = int(round(float(seconds)))
    hours, rem = divmod(total, 3600)
    minutes, secs = divmod(rem, 60)
    parts: list[str] = []
    if hours:
        parts.append(f"{hours} 小时")
    if minutes:
        parts.append(f"{minutes} 分")
    parts.append(f"{secs} 秒")
    return " ".join(parts)


def read_previous_report(reports_dir: Path, current: Path) -> Path | None:
    if not reports_dir.is_dir():
        return None
    candidates = sorted(
        p
        for p in reports_dir.glob("docker-self-test-*.md")
        if p.resolve() != current.resolve() and p.name != "README.md"
    )
    return candidates[-1] if candidates else None


def extract_field(text: str, label: str) -> str | None:
    m = re.search(rf"\- \*\*{re.escape(label)}\*\*: (.+)$", text, re.M)
    return m.group(1).strip() if m else None


def extract_counts(text: str) -> tuple[int, int, int] | None:
    m = re.search(
        r"\| \*\*合计\*\* \| (\d+) \| (\d+) \| (\d+) \|",
        text,
    )
    if not m:
        return None
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def diff_section(prev_path: Path | None, totals: dict, sha: str) -> str:
    if prev_path is None:
        return "首次报告，无上一份可对比。"
    prev = prev_path.read_text(encoding="utf-8", errors="replace")
    prev_sha = extract_field(prev, "git commit") or "未知"
    counts = extract_counts(prev)
    lines = [
        f"上一份报告：[`{prev_path.name}`]({prev_path.name})",
        f"- 上一份 git commit：{prev_sha}",
        f"- 本份 git commit：`{sha}`",
    ]
    if sha and prev_sha and sha not in prev_sha and prev_sha != "未知":
        lines.append("- 相对上一份：commit 已变化（摘要见 `git log`）。")
    else:
        lines.append("- 相对上一份：同一 commit 或无法解析上一份 SHA。")
    if counts:
        p, f, i = counts
        lines.append(
            f"- 通过 {p} → {totals['passed']}（{totals['passed'] - p:+d}）；"
            f"失败 {f} → {totals['failed']}（{totals['failed'] - f:+d}）；"
            f"忽略 {i} → {totals['ignored']}（{totals['ignored'] - i:+d}）"
        )
    else:
        lines.append("- 上一份合计行无法解析，跳过计数对比。")
    return "\n".join(lines)


def snippet_block(name: str, lines: Iterable[str], limit: int = 80) -> str:
    body = "\n".join(list(lines)[:limit]).rstrip()
    if not body:
        body = "（无 stdout 片段）"
    return f"### `{name}`\n\n```\n{body}\n```\n"


def render_markdown(meta: dict, parsed: dict) -> str:
    totals = parsed["totals"]
    stamp = meta["generated_at"]
    filter_note = {
        "all": "默认全量 `cargo test`（src-tauri 整 crate）",
        "*": "默认全量 `cargo test`（src-tauri 整 crate）",
        "proxy_projection_linux": "显式收窄：仅 `proxy_projection_linux`",
        "proxy": "显式收窄：仅 `proxy_projection_linux`",
        "pi-crud": "显式收窄：Pi CRUD Linux 夹具 + fetch_models_ / live-update lib",
        "pi_crud": "显式收窄：Pi CRUD Linux 夹具 + fetch_models_ / live-update lib",
        "lib-lite": "显式收窄：`--lib fetch_models_` + 关键 Pi CRUD lib",
        "lite": "显式收窄：`--lib fetch_models_` + 关键 Pi CRUD lib",
    }.get(meta["filter"], f"自定义 cargo 过滤：`{meta['filter']}`")

    verdict = "通过" if meta["exit_code"] == 0 and totals["status"] != "FAILED" else "失败"
    target_rows = []
    for t in parsed["targets"]:
        secs = f"{t['secs']}s" if t["secs"] else "—"
        target_rows.append(
            f"| `{t['name']}` | {t['status'] or '—'} | {t['passed']} | {t['failed']} | {t['ignored']} | {secs} |"
        )
    if not target_rows:
        target_rows.append("| （未解析到 cargo `test result` 行） | — | — | — | — | — |")

    failed_md = "无失败用例。\n"
    if parsed["failed_names"] or parsed["compile_errors"]:
        bits = []
        if parsed["compile_errors"]:
            bits.append("### 编译错误\n\n```\n" + "\n".join(parsed["compile_errors"][:40]) + "\n```\n")
        for name in parsed["failed_names"]:
            bits.append(snippet_block(name, parsed["failed_snippets"].get(name, [])))
        failed_md = "\n".join(bits)

    log_name = Path(meta["log_path"]).name if meta.get("log_path") else ""
    log_note = f"完整 cargo 日志：`{log_name}`（同目录，默认不入库）" if log_name else "无日志文件"

    return f"""# Docker 后端全量自测报告

**默认全量；每次给全量自测报告。** SCHEMA 保持 **18**。不写用户 Windows `C:`，不宣称 Docker 内可跑完整 Tauri GUI，本流程不出 Portable / MSI。

- **结论**: {verdict}
- **生成时间（UTC）**: {stamp}
- **git commit**: `{meta["sha"]}`
- **版本**: {meta["version"]}
- **SCHEMA_VERSION**: {meta["schema"]}
- **镜像 / tag**: {meta["image"]}
- **镜像 digest / Id**: `{meta["image_id"]}`
- **命令**: `{meta["command"]}`
- **TEST_FILTER**: `{meta["filter"]}` — {filter_note}
- **运行环境**: {meta["runner"]}
- **墙钟耗时**: {format_duration(meta["duration_sec"])}（{meta["duration_sec"]} 秒）
- **退出码**: {meta["exit_code"]}
- **{log_note}**

## 合计

| | 通过 | 失败 | 忽略 |
| --- | ---: | ---: | ---: |
| **合计** | {totals["passed"]} | {totals["failed"]} | {totals["ignored"]} |

cargo 汇总状态：`{totals["status"]}`（分目标相加；含 lib / integration / doc-tests）。

## 分目标

| 目标 | 状态 | 通过 | 失败 | 忽略 | 用时 |
| --- | --- | ---: | ---: | ---: | --- |
{chr(10).join(target_rows)}

## 失败用例与片段

{failed_md}

## 相对上一份报告

{meta["diff"]}

## 硬约束

- `SCHEMA_VERSION` 仍为 **18**（无 `migrate_v18_to_v19`，无 `proxy_config` `app_type='pi'` 行）
- 隔离目录为容器内 `CC_SWITCH_TEST_HOME=/tmp/cc-switch-test-home`（命名卷），不是宿主机用户目录，也不是 `C:\\Users\\…`
- 未覆盖：真机 WSL UNC、Windows MSI / Portable、完整 Tauri GUI / 托盘 / WebView
"""


def self_check() -> None:
    sample = """
     Running unittests src/lib.rs (target/debug/deps/cc_switch_lib-abc)

running 3 tests
test services::model_fetch::tests::fetch_models_candidates_do_not_rewrite_to_claude_listen ... ok
test boom ... FAILED
test skippy ... ignored

failures:

---- boom stdout ----
thread 'boom' panicked at src/lib.rs:1:1:
assertion failed: SCHEMA_VERSION == 18

failures:
    boom

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.50s

\x1b[1m\x1b[92m     Running\x1b[0m tests/proxy_projection_linux.rs (target/debug/deps/proxy_projection_linux-xyz)

running 2 tests
test linux_standin_create_openai_completions_pins_developer_role_false ... ok
test linux_standin_update_writes_url_and_key_to_models_json ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s
"""
    parsed = parse_cargo_log(sample)
    assert parsed["totals"]["passed"] == 3, parsed
    assert parsed["totals"]["failed"] == 1, parsed
    assert parsed["totals"]["ignored"] == 1, parsed
    assert parsed["failed_names"] == ["boom"], parsed
    assert any("SCHEMA_VERSION == 18" in line for line in parsed["failed_snippets"]["boom"])
    assert any("proxy_projection_linux" in t["name"] for t in parsed["targets"]), parsed["targets"]
    md = render_markdown(
        {
            "generated_at": "2026-09-21 00:00",
            "sha": "deadbeef",
            "version": "3.20.12",
            "schema": "18",
            "image": "cc-switch-backend-self-test:local",
            "image_id": "sha256:demo",
            "command": "pnpm test:docker",
            "filter": "all",
            "runner": "self-check",
            "duration_sec": 12,
            "exit_code": 1,
            "log_path": "docs/self-test-reports/demo.log",
            "diff": "首次报告，无上一份可对比。",
        },
        parsed,
    )
    assert "SCHEMA_VERSION**: 18" in md
    assert "默认全量" in md
    assert "boom" in md
    print("render-docker-self-test-report.py self-check: ok")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Render Docker backend self-test markdown report")
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--log", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--sha", default="unknown")
    parser.add_argument("--version", default="unknown")
    parser.add_argument("--schema", default="18")
    parser.add_argument("--image", default="cc-switch-backend-self-test:local")
    parser.add_argument("--image-id", default="unavailable")
    parser.add_argument("--command", default="pnpm test:docker")
    parser.add_argument("--filter", default="all")
    parser.add_argument("--runner", default="docker")
    parser.add_argument("--duration-sec", type=float, default=0)
    parser.add_argument("--exit-code", type=int, default=1)
    parser.add_argument("--reports-dir", type=Path)
    parser.add_argument("--print", action="store_true", dest="do_print")
    args = parser.parse_args(argv)

    if args.self_check:
        self_check()
        return 0

    if not args.log or not args.out:
        parser.error("--log and --out are required unless --self-check")

    text = args.log.read_text(encoding="utf-8", errors="replace")
    parsed = parse_cargo_log(text)
    reports_dir = args.reports_dir or args.out.parent
    prev = read_previous_report(reports_dir, args.out)
    meta = {
        "generated_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d %H:%M"),
        "sha": args.sha,
        "version": args.version,
        "schema": args.schema,
        "image": args.image,
        "image_id": args.image_id,
        "command": args.command,
        "filter": args.filter,
        "runner": args.runner,
        "duration_sec": args.duration_sec,
        "exit_code": args.exit_code,
        "log_path": str(args.log),
        "diff": diff_section(prev, parsed["totals"], args.sha),
    }
    md = render_markdown(meta, parsed)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(md, encoding="utf-8")
    if args.do_print:
        sys.stdout.write(md)
        if not md.endswith("\n"):
            sys.stdout.write("\n")
    print(f"REPORT_PATH={args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
