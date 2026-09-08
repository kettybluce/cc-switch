# Linux 上测试「远控 WSL」路径

这份文档说明：**不必登录任何人的 Windows 电脑、不必碰真实 `machineId` / Ubuntu-22.04**，也能在 Cursor Cloud Agent Linux VM 和 GitHub CI Linux 上，把 CC Switch 的 WSL 远程控制路径跑通。

产品里的 WSL 路径是：Windows 上的 CC Switch 通过 `wsl.exe -d <distro> -- bash …` 读写发行版里的 `~/.pi/agent/`（**禁止**走 `\\wsl.localhost` UNC）。云端和 CI 没有 `wsl.exe`，所以用现成的 `WslRunner` 特质把「发行版里的 bash」换成 **本机 bash 测试替身**（`LocalBashRunner`），再配一套假的 `~/.pi/agent` 夹具。

```text
Windows 真机                         Linux 云 / CI 本 harness
─────────────────                    ─────────────────────────
CC Switch (Windows)                  cargo test wsl_linux_harness
    │                                     │
    │ wsl.exe -d Ubuntu-22.04 -- bash     │ LocalBashRunner
    ▼                                     │   HOME=<temp> bash -c <同一段脚本常量>
Ubuntu-22.04  ~/.pi/agent/                ▼
    models.json, sessions/           临时目录 ~/.pi/agent/  （fixtures 拷贝）
```

脚本仍是 `pi_runtime` 里的编译期常量，用户值仍走位置参数 `$1`、`$2`。替身会走与真 `wsl.exe` 相同的 `build_wsl_argv` 校验，只是把 `-d <distro> --` 丢掉后在本机执行 `bash`。

## 夹具包

位置：[`tests/fixtures/pi-wsl/`](../tests/fixtures/pi-wsl/)。

| 路径 | 用途 |
| --- | --- |
| `cases/identical/{models.json,agent/models.json}` | 两份文件字节级相同（健康同步的稳态） |
| `cases/diverge/` | agent 为规范上游；顶层残留 `/pi/<id>` 代理 URL（接管恢复后分叉） |
| `cases/only-agent/` | 只有 `agent/models.json`（顶层镜像尚未写出） |
| `sessions/--home-tfdx8045-code-agent--/*.jsonl` | cwd 组会话：`session`、`model_change`、带 `usage.cost.total = 0` 的 assistant |
| `sessions/--home-tfdx8045-code-agent--/<id>/tasks/*.jsonl` | 任务子会话（相对 `sessions/` 深度正好为 4） |

cwd 编码与 Pi 一致：`/home/tfdx8045/code/agent` → `--home-tfdx8045-code-agent--`。会话 header 里的 `cwd` 才是权威路径；目录名只用于分组。

`models.json` 里 `gpt-4.1-mini` 的 `cost` 是 **美元 / 百万 token**。JSONL 里嵌入的 `cost.total` 故意为 0，导入时必须用单价 × 令牌，不能信 JSONL。

## 覆盖范围（本 harness）

通过 `pi_runtime` 公共 API，**假装目标是 WSL**：

1. **嵌套会话发现**：`find -maxdepth 4`（manifest / probe），而不是 `sessions/*.jsonl` 扁平 glob。cwd 组文件和 `tasks/*.jsonl` 都会被镜像，再交给现有扫描器。
2. **逐行解析 JSONL**：抽出 `message.usage`；`cost.total == 0` 时按 `models.json` 单价计费。
3. **双写 / 同步**：`read` 时 heal（identical / diverge / only-agent）；`sync_live_providers` 投影与 restore 后 `~/.pi/agent/models.json` 与 `~/.pi/models.json` 必须一致。
4. **cwd 编解码**：`--…--` 与 `/home/tfdx8045/code/agent` 往返。

Rust：`src-tauri/src/pi_runtime/wsl_linux_harness.rs`（`cargo test --lib wsl_linux_harness`）。  
前端契约：`tests/pi-wsl-harness.test.ts`（解析同一套 JSONL / cwd 目录名）。

## 明确不覆盖（真 Windows + WSL GUI）

| 能力 | 为何不在 Linux 云上做 |
| --- | --- |
| `wsl.exe` 启动/停止发行版、UTF-16 `wsl -l -q` | 没有 Windows 主机；UTF-16 列表另有单元测试 |
| NAT vs 镜像网络、探测网关 / `resolv.conf`、把 `127.0.0.1` 写入发行版 | 需要真 WSL 网卡；代理探测是 `pi_runtime::proxy` |
| 登录壳 `~/.bashrc` / nvm 把 `pi` 放进 PATH | 替身去掉了 `PI_CODING_AGENT_DIR`，但不会跑用户的 profile |
| 设置页「Pi 接管」开关、托盘、会话列表 GUI | 无 Windows GUI；前端有 `PiRuntimeSettings` 等组件测试 |
| UNC `\\wsl.localhost\…` 写盘 | 产品禁止这条路径；Windows CI 另有原子写契约 |
| 用户本机 `machineId`、真实 Ubuntu-22.04 家目录 | 约束：不得访问 |

真机验收（Windows MSI）仍按产品清单：打开 Pi 接管后两份 `models.json` 都变成 `/pi/<id>`；关掉后都恢复上游 URL。

## 依赖（Linux）

`bash`、GNU `find`（`-printf`）、`gzip`、`sha256sum`。这是发行版内脚本已经在用的工具。macOS 的 BSD `find` 没有 `-printf`，本 harness 以 Linux CI / 云代理为准。

## 本地重跑（Linux）

仓库根目录：

```bash
# Rust：WSL runner 替身 + 夹具（约数秒，不拉起 GUI）
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness -- --nocapture

# 相关的既有 pi_runtime / 用量测试也可一并跑
cargo test --manifest-path src-tauri/Cargo.toml --lib pi_runtime::
cargo test --manifest-path src-tauri/Cargo.toml --lib session_usage_pi::

# 前端：夹具 JSONL / cwd 目录名契约
pnpm test:unit tests/pi-wsl-harness.test.ts
```

Windows 上若只想跑这套 **可选** 脚本（仍不连真实发行版，只是有 bash 时调 cargo）：[`scripts/optional-run-pi-wsl-harness.ps1`](../scripts/optional-run-pi-wsl-harness.ps1)。
