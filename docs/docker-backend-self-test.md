# Docker backend self-test / Docker 后端自测

**默认全量；每次给全量自测报告。**

Cloud / Linux / Docker Desktop can build an image and run the **full** `src-tauri` cargo test suite without touching a user Windows `C:` profile, without bumping **SCHEMA 18**, and without claiming the Tauri GUI works in Docker. Every run writes a Chinese markdown report under `docs/self-test-reports/`.

云端 / Linux / Docker Desktop 可构建镜像并跑 **src-tauri 全量** cargo 测试：不写用户 Windows `C:`、不升 SCHEMA（保持 **18**）、也不宣称 Docker 里能跑完整 Tauri GUI。**默认全量**；每次运行都会写出一份全量自测报告。

## What it covers / 覆盖范围

| Covered / 覆盖 | Not covered / 不覆盖 |
| --- | --- |
| **Default** `TEST_FILTER=all` — full `cargo test --manifest-path src-tauri/Cargo.toml` (includes `proxy_projection_linux` Pi / Claude / Codex takeover projection, Claude/Codex roundtrips: unknown fields, hot-switch backup, independent disable, fail-closed enable: missing/malformed Live, foreign local proxy including Pi, **and** Codex OAuth `CodexLiveAuthSwitchGuard` stale-binding cases (`MissingAccount` switch-away / stale target during takeover); plus `session_usage_scan` isolated-HOME JSONL usage and `provider_profile_race` concurrent CRUD/switch) | Live WSL UNC (`\\wsl.localhost\…`) on a real Windows host |
| Isolated `CC_SWITCH_TEST_HOME` under `/tmp` **inside** the container | Windows MSI / Portable installers |
| Markdown report: SHA / version / SCHEMA 18 / command / duration / passed-failed-ignored / failure snippets | Full Tauri GUI, tray, or WebView window |
| SCHEMA 18 assertions (no `proxy_config` row for `pi`) | Official 3.20.x GUI QA on the user's desktop |

Same Linux packages as `.github/workflows/ci.yml` (`pkg-config`, `libssl`, GTK 3, WebKit, Ayatana AppIndicator, soup). Base image: **Ubuntu 22.04** (CI `ubuntu-22.04`).

系统包与 CI Linux 任务一致。基础镜像 **Ubuntu 22.04**。

## Commands / 命令

From the repo root (Docker + Compose v2 required). **Default is the full crate** (long: first image build compiles all tests; subsequent runs reuse `cc-switch-backend-self-test:local`).

在仓库根目录（需要 Docker 与 Compose v2）。**默认全量**（首次镜像构建会编译全部测试，可能较久）。

```bash
# Default: FULL src-tauri cargo test + markdown report
# 默认：全量 cargo test，并写出中文报告
pnpm test:docker
# same:
make test-docker
bash scripts/docker-self-test.sh
docker compose -f docker-compose.test.yml run --build --rm backend-self-test
```

Expect compose / cargo **exit 0**, plus a report file:

期望 compose / cargo **退出码 0**，并生成报告：

```text
docs/self-test-reports/docker-self-test-YYYYMMDD-HHMM.md
```

The wrapper prints the report to stdout and the path as `REPORT_PATH=...`. Default cargo equivalent:

封装脚本会把报告打到 stdout，并打印 `REPORT_PATH=...`。默认等价于：

```bash
cargo test --offline --locked --no-fail-fast --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

### Explicit narrower filters / 显式收窄（可选）

Only use these when you do **not** want the full suite:

只有在你**明确不要全量**时才用：

```bash
# Linux fixture / proxy projection only (#43)
TEST_FILTER=proxy_projection_linux pnpm test:docker
# or
pnpm test:docker:proxy
make test-docker-proxy

# Pi CRUD linux stand-in + fetch_models_ / live-update lib tests (#46)
TEST_FILTER=pi-crud pnpm test:docker
pnpm test:docker:pi-crud
make test-docker-pi-crud

# Lightweight lib slice: fetch_models_ + key Pi CRUD lib tests
TEST_FILTER=lib-lite pnpm test:docker
pnpm test:docker:lite
make test-docker-lite
docker compose -f docker-compose.test.yml --profile lite run --build --rm backend-self-test-lite
```

`TEST_FILTER=all` is accepted but redundant (already the default). Any other value is passed to cargo as a name substring (same as `cargo test FILTER`):

`TEST_FILTER=all` 可写，但与默认相同。其它值当作 cargo 测试名子串：

```bash
docker compose -f docker-compose.test.yml run --build --rm -e TEST_FILTER=pi_takeover_projects backend-self-test
```

Override the cargo invocation entirely (no report unless you use the host wrapper):

完全覆盖 cargo 命令（直接 compose 不会写报告；要用报告请走 `pnpm test:docker`）：

```bash
docker compose -f docker-compose.test.yml run --build --rm backend-self-test \
  cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib linux_standin_agent_dir
```

`CARGO_TEST_THREADS` defaults to `1` (fixture tests hold a process-wide HOME mutex).

`CARGO_TEST_THREADS` 默认 `1`（夹具测试会持有进程级 HOME 互斥锁）。

### Report contents / 报告内容

Each `pnpm test:docker` run writes `docs/self-test-reports/docker-self-test-YYYYMMDD-HHMM.md` (Chinese) including:

每次 `pnpm test:docker` 会写中文报告，至少包含：

- git commit SHA / `package.json` version
- `SCHEMA_VERSION`（必须为 **18**）
- image tag + digest / Id（若本机有 Docker）
- command used / 使用的命令
- wall-clock duration / 墙钟耗时
- total passed / failed / ignored
- full list of failed tests with stdout snippets
- diff vs the previous report in that directory（若有上一份）

Raw cargo logs: `docs/self-test-reports/*.log` (gitignored).

原始 cargo 日志：`docs/self-test-reports/*.log`（不入库）。

Without Docker, the wrapper refuses unless you opt into a host cargo equivalent (still emits the same report shape):

没有 Docker 时脚本会失败，除非显式打开宿主机回退（报告格式相同）：

```bash
CC_SWITCH_SELFTEST_HOST_FALLBACK=1 pnpm test:docker
```

## Isolation / 隔离

- `CC_SWITCH_TEST_HOME=/tmp/cc-switch-test-home` inside the container
- Named Docker volume `cc-switch-test-home` — **not** the host user profile, **not** `C:\Users\…`
- Container drops to uid `tester` (1000) after chowning that volume. Tests run as a regular user (same as CI), not root.
- Tests still assert they do not write `pi-wsl-sessions` or Windows `C:` paths

容器内 `CC_SWITCH_TEST_HOME=/tmp/cc-switch-test-home`；命名卷 `cc-switch-test-home`，不是宿主机用户目录，也不是 `C:\Users\…`。入口脚本会把卷交给非 root 用户 `tester` 再跑 cargo（与 CI 一致）。测试仍会断言不写 `pi-wsl-sessions` / Windows `C:`。

Reset the volume if needed / 需要时重置卷：

```bash
docker volume rm cc-switch-test-home
```

## CI

Default GitHub Actions **does not** build this image (Rust + GTK compile is too heavy for every PR). CI still runs the existing frontend + backend jobs on the host runners (backend `cargo test` is the merge gate).

默认 GitHub Actions **不会**在每个 PR 上构建此镜像（Rust + GTK 编译太重）。仓库 CI 仍在 runner 上跑原有前端 / 后端任务（后端 `cargo test` 仍是合入门槛）。

A cheap job validates `docker compose -f docker-compose.test.yml config` and the report renderer self-check when Docker files change.

仅当 Docker 相关文件变更时，有一个轻量任务校验 compose 配置和报告脚本自检。

## Shipping / 发版

Default `pnpm test:docker` **is** the full suite and writes the markdown report. After self-test work, a Windows Portable (preferred) / MSI ship **requires** that full-suite report:

默认 `pnpm test:docker` **就是全量**并写出 Markdown 报告。自测工作之后发 Windows 绿色版（优先）/ MSI **必须**有这份全量报告：

- Command: `pnpm test:docker` / `pnpm test:docker:all` / `make test-docker` (`TEST_FILTER=all`)
- Report: `docs/self-test-reports/docker-self-test-YYYYMMDD-HHMM.md` (exit code, pass/fail counts, SCHEMA 18, no host `C:` writes)
- Narrower filters (`proxy` / `session_usage_scan` / `provider_profile_race` / `pi-crud` / `lib-lite`) are **not** the shipping report

显式收窄不能代替发版报告。本 Docker 工作流本身不出 Portable / MSI。

v3.20.14 出包烧机清单（夹具 PR 门槛、Portable dry-run、禁止提前打 tag）：[release-v3.20.14-burn-in-zh.md](release-v3.20.14-burn-in-zh.md)。

## Hard no / 硬约束

- No `SCHEMA_VERSION` 19 / `migrate_v18_to_v19`
- No Portable / MSI artifact from this workflow
- No claim that the desktop app runs under Docker
