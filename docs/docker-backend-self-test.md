# Docker backend self-test / Docker 后端自测

Cloud / Linux / Docker Desktop can build an image and run **backend fixture + proxy** cargo tests without touching a user Windows `C:` profile, without bumping **SCHEMA 18**, and without claiming the Tauri GUI works in Docker.

云端 / Linux / Docker Desktop 可构建镜像并跑后端夹具与代理 cargo 测试：不写用户 Windows `C:`、不升 SCHEMA（保持 **18**）、也不宣称 Docker 里能跑完整 Tauri GUI。

## What it covers / 覆盖范围

| Covered / 覆盖 | Not covered / 不覆盖 |
| --- | --- |
| `proxy_projection_linux` — POSIX stand-in for Pi / Claude / Codex takeover projection | Live WSL UNC (`\\wsl.localhost\…`) on a real Windows host |
| Isolated `CC_SWITCH_TEST_HOME` under `/tmp` **inside** the container | Windows MSI / Portable installers |
| Optional `TEST_FILTER=all` → full `cargo test --manifest-path src-tauri/Cargo.toml` | Full Tauri GUI, tray, or WebView window |
| SCHEMA 18 assertions (no `proxy_config` row for `pi`) | Official 3.20.x GUI QA on the user's desktop |

Same Linux packages as `.github/workflows/ci.yml` (`pkg-config`, `libssl`, GTK 3, WebKit, Ayatana AppIndicator, soup). Base image: **Ubuntu 22.04** (CI `ubuntu-22.04`).

系统包与 CI Linux 任务一致。基础镜像 **Ubuntu 22.04**。

## Commands / 命令

From the repo root (Docker + Compose v2 required):

在仓库根目录（需要 Docker 与 Compose v2）：

```bash
# Default: Linux fixture / proxy projection only (minimum gate)
# 默认：只跑 proxy_projection_linux（最低门槛）
docker compose -f docker-compose.test.yml run --build --rm backend-self-test

# Same via wrappers
pnpm test:docker
make test-docker
```

Expect compose / cargo **exit 0**. The default command is:

期望 compose / cargo **退出码 0**。默认等价于：

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test proxy_projection_linux -- --test-threads=1
```

Broader crate (compiles extra tests on first run; slower). **Required before shipping a Portable/MSI after self-test work**, together with a written report (exit code + pass/fail counts):

自测工作之后出 Portable/MSI **必须**跑全量并附报告（退出码 + 通过/失败计数）：

```bash
docker compose -f docker-compose.test.yml run --build --rm -e TEST_FILTER=all backend-self-test
# or
pnpm test:docker:all
make test-docker-all
```

Any other `TEST_FILTER` is passed to cargo as a name substring (same as `cargo test FILTER`):

其它 `TEST_FILTER` 会当作 cargo 测试名子串（与 `cargo test FILTER` 相同）：

```bash
docker compose -f docker-compose.test.yml run --build --rm -e TEST_FILTER=pi_takeover_projects backend-self-test
```

Override the cargo invocation entirely:

完全覆盖 cargo 命令：

```bash
docker compose -f docker-compose.test.yml run --build --rm backend-self-test \
  cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib linux_standin_agent_dir
```

`CARGO_TEST_THREADS` defaults to `1` (the fixture tests hold a process-wide HOME mutex).

`CARGO_TEST_THREADS` 默认 `1`（夹具测试会持有进程级 HOME 互斥锁）。

## Isolation / 隔离

- `CC_SWITCH_TEST_HOME=/tmp/cc-switch-test-home` inside the container
- Named Docker volume `cc-switch-test-home` — **not** the host user profile, **not** `C:\Users\…`
- Tests still assert they do not write `pi-wsl-sessions` or Windows `C:` paths

容器内 `CC_SWITCH_TEST_HOME=/tmp/cc-switch-test-home`；命名卷 `cc-switch-test-home`，不是宿主机用户目录，也不是 `C:\Users\…`。测试仍会断言不写 `pi-wsl-sessions` / Windows `C:`。

Reset the volume if needed / 需要时重置卷：

```bash
docker volume rm cc-switch-test-home
```

## CI

Default GitHub Actions **does not** build this image (Rust + GTK compile is too heavy for every PR). CI still runs the existing frontend + backend jobs on the host runners.

默认 GitHub Actions **不会**在每个 PR 上构建此镜像（Rust + GTK 编译太重）。仓库 CI 仍在 runner 上跑原有前端 / 后端任务。

A cheap job only validates `docker compose -f docker-compose.test.yml config` when the Docker files change.

仅当 Docker 相关文件变更时，有一个轻量任务校验 compose 配置。

## Shipping / 发版

After self-test work, a Windows Portable (preferred) / MSI ship **requires** Docker **full-suite** self-test **and a report**:

自测工作之后发 Windows 绿色版（优先）/ MSI **必须**有 Docker **全量**自测 **和报告**：

- Command: `pnpm test:docker:all` / `make test-docker-all` / `TEST_FILTER=all`
- Report: exit code, pass/fail counts, SCHEMA still 18, no host `C:` writes
- Default `pnpm test:docker` (`proxy_projection_linux` only) is the **minimum gate**, not the shipping report

默认 `pnpm test:docker` 只是最低门槛，不是发版报告。

## Hard no / 硬约束

- No `SCHEMA_VERSION` 19 / `migrate_v18_to_v19`
- No Portable / MSI artifact from this workflow
- No claim that the desktop app runs under Docker
