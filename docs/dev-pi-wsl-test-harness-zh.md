# Linux 上测试「远控 WSL」路径

这份文档说明：**不必登录任何人的 Windows 电脑、不必碰真实 `machineId` / Ubuntu-22.04**，也能在 Cursor Cloud Agent Linux VM 和 GitHub CI Linux 上，把 CC Switch 的 WSL 远程控制路径跑通。

产品里 **接管 / models.json 写入** 仍走 `wsl.exe -d <distro> -- bash …`。**会话与用量发现**改为与开源版 Claude 相同的 WSL 家目录：Windows 上是 `\\wsl.localhost\<发行版>\home\<linux 用户>\.pi\…`，不得再默认镜像到 `%USERPROFILE%\.cc-switch\pi-wsl-sessions`。云端和 CI 没有 9P 共享，所以夹具用同一套 POSIX `~/.pi/agent`（文档中的等价路径），`LocalBashRunner` 只覆盖仍走 bash 的写入/探测。

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

| 路径                                                                | 用途                                                                          |
| ------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `cases/identical/{models.json,agent/models.json}`                   | 两份文件字节级相同（健康同步的稳态）；含 requested / served 两套单价          |
| `cases/diverge/`                                                    | agent 为规范上游；顶层残留 `/pi/<id>` 代理 URL（接管恢复后分叉）              |
| `cases/only-agent/`                                                 | 只有 `agent/models.json`（顶层镜像尚未写出）                                  |
| `sessions/--home-tfdx8045-code-agent--/*.jsonl`                     | cwd 组会话：`session`、`model_change`、带 `usage.cost.total = 0` 的 assistant |
| `sessions/…/<id>/tasks/*.jsonl`                                     | 任务子会话（相对 `sessions/` 深度 4）                                         |
| `sessions/…/<id>/tasks/group/*.jsonl`                               | 更深一层（深度 5）。旧 `find -maxdepth 4` 会漏掉；现共享 `SESSION_JSONL_MAXDEPTH = 8` |
| `sessions/…/2026-03-14T11-00-00_map.jsonl`                          | `model` ≠ `responseModel`：计价必须用 served 模型的 `models.json` 单价        |

cwd 编码与 Pi 一致：`/home/tfdx8045/code/agent` → `--home-tfdx8045-code-agent--`。会话 header 里的 `cwd` 才是权威路径；`decode_session_cwd` **仅测试使用**，目录名不能当 cwd 神谕（带连字符的路径无法往返）。

`models.json` 里 `gpt-4.1-mini` 的 `cost` 是 **美元 / 百万 token**（0.4）；`gpt-4.1-mini-served` 为 8。JSONL 里嵌入的 `cost.total` 故意为 0，导入时必须用单价 × 令牌，不能信 JSONL。requested ≠ served 时以 **served**（`responseModel`）为准。

## 覆盖范围（本 harness）

通过 `pi_runtime` 公共 API，**假装目标是 WSL**：

1. **嵌套会话发现**：会话列表与用量直接走 WSL 家目录（Linux 夹具即 POSIX `~/.pi/agent/sessions`）。probe 的 `sessionCount` 与发现深度 **共用** `SESSION_JSONL_MAXDEPTH`（当前 8）。cwd 组、`tasks/*.jsonl`、以及旧 maxdepth 4 会丢掉的 `tasks/group/*.jsonl` 都必须被看到，且 **不得** 写入 `pi-wsl-sessions`。
2. **逐行解析 JSONL**：抽出 `message.usage`；`cost.total == 0` 时按 `models.json` 单价计费；requested ≠ served 时按 served 模型计价。
3. **双写 / 同步**（默认 `cargo test` 路径，不藏在 feature flag 后）：`read` 时 heal（identical / diverge / only-agent）；`sync_live_providers` 投影与 restore：only-agent 会补写顶层镜像，diverge 会覆盖 stale top。
4. **cwd 编解码**：`--…--` 与 `/home/tfdx8045/code/agent` 往返；并断言带连字符路径不可往返。生产 `project_dir` 来自 JSONL `cwd`。
5. **代理主机候选（无真 WSL）**：用 canned `WslRunner` 回放 `HOST_SCRIPT` / 健康探测。镜像网络下 loopback 先应答；NAT 下 loopback `000` 时回落到网关 `172.30.208.1`。
6. **非 GNU find**：stub `find` 拒绝 `-printf` 时走 portable `find -print` + `stat`，不得静默 `fetched=0`；完全不可用的 `find` 必须硬失败。

Rust：`src-tauri/src/pi_runtime/wsl_linux_harness.rs`（`cargo test --lib wsl_linux_harness`）。  
前端契约：`tests/pi-wsl-harness.test.ts`（解析同一套 JSONL / cwd 目录名）。

## 明确不覆盖（真 Windows + WSL GUI）

| 能力                                                                 | 为何不在 Linux 云上做                                    |
| -------------------------------------------------------------------- | -------------------------------------------------------- |
| `wsl.exe` 启动/停止发行版、UTF-16 `wsl -l -q`                        | 没有 Windows 主机；UTF-16 列表另有单元测试               |
| 真网卡上的 NAT vs 镜像网络、改写发行版 `resolv.conf`                 | 需要真 WSL 网卡；候选顺序与探测结果由 canned runner 覆盖 |
| 登录壳 `~/.bashrc` / nvm 把 `pi` 放进 PATH                           | 替身去掉了 `PI_CODING_AGENT_DIR`，但不会跑用户的 profile |
| 设置页「Pi 接管」开关、托盘、会话列表 GUI                            | 无 Windows GUI；前端有 `PiRuntimeSettings` 等组件测试    |
| 真机 `\\wsl.localhost\…` 9P 读盘                                     | Linux 云没有 9P；夹具用 POSIX 等价路径，UNC 字符串有单元测试 |
| 用户本机 `machineId`、真实 Ubuntu-22.04 家目录                       | 约束：不得访问                                           |

真机验收（Windows MSI）仍按产品清单：打开 Pi 接管后两份 `models.json` 都变成 `/pi/<id>`；关掉后都恢复上游 URL。

## 依赖（Linux）

`bash`、`find`、`gzip`、`sha256sum`。发行版内脚本 **优先** GNU `find -printf`；若 `-printf` 不可用（BSD find），改为 `find -print` + `stat -c` / `stat -f`，并在 listing 工具完全不可用时失败，而不是把 stderr 吞掉装成 0 个会话。本 harness 以 Linux CI / 云代理为准。

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
