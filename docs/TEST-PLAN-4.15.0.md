# CC Switch 4.15.0 完整测试方案

> 本方案针对 **Windows + WSL 混合场景**：CC Switch 跑在 Windows，agent 在 WSL 里使用，
> CC Switch 通过 `\\wsl.localhost` UNC 路径接管 WSL 侧配置。
>
> 版本：4.15.0（基线官方 v4.0.14 + 自研 Pi 层 + DSH 阶段一）
> 安装位置：`C:\Users\<user>\AppData\Local\CC Switch\cc-switch.exe`

---

## 零、先决条件与已知环境事实

### 0.1 环境基线

| 项 | 实际值 | 备注 |
|---|---|---|
| WSL 发行版 | Ubuntu-22.04 | `networkingMode=mirrored`，`autoProxy=false` |
| CC Switch 配置指向 | 全部指向 WSL | `claudeConfigDir` / `codexConfigDir` / `piConfigDir` = `\\wsl.localhost\Ubuntu-22.04\home\<user>\...` |
| 本地代理端口 | `127.0.0.1:15721` | |
| 系统代理 | `127.0.0.1:7897`（10808 已无监听） | |
| 数据库 schema | v20 | 与应用 `SCHEMA_VERSION` 一致 |
| 卡片数 | codex 27 / claude 21 / claude-desktop 2 / grokbuild 2 / gemini 1 / hermes 1 / pi 1 | DSH 尚无卡片 |

### 0.2 ⚠️ 必须先知道的配置错位问题

**实测结论（2026-10-17 验证）：**

| Agent | WSL 内可执行 | 实际读取的配置 | 与 CC Switch 配置指向是否一致 |
|---|---|---|---|
| `claude` | ✅ 2.1.293 | **Windows 侧** `C:\Users\<user>\.claude` | ❌ **不一致** |
| `codex` | ✅ 0.161.0（仅 `/home/<user>/.npm-global/bin`，不在 PATH） | 待验证 | 待验证 |
| `pi` | ✅ 1.1.0 | **WSL 侧** `~/.pi` | ✅ 一致 |
| `dsh` | ✅ 0.2.0-rc.2 | 待验证 | 待验证 |

**claude 错位的原因与证据**：

WSL 里 `command -v claude` 解析到 `/mnt/c/Users/<user>/AppData/Roaming/npm/claude`，
该脚本执行的是 `claude.exe`（Windows 二进制）。Windows 进程按 `USERPROFILE` 找配置，
于是读 `C:\Users\<user>\.claude`，而**不是** WSL 的 `~/.claude`。

决定性实验：把 WSL 侧 `~/.claude/settings.json` 的 `ANTHROPIC_BASE_URL` 改成不可达的
`http://127.0.0.1:59999`，跑 `claude -p "say OK"`，报错里出现的仍是
`<VPS_HOST>:8317`（Windows 侧的真实地址）——证明它读的是 Windows 配置。

**这意味着**：在 CC Switch 里切换 claude 卡片，写入的是 WSL 配置，但 WSL 里跑 `claude`
读的是 Windows 配置，**看起来"切换没反应"**。

**pi 为何正常**：`pi` 虽然也经 `/mnt/c` 的包装器启动，但它是 Node 脚本，
`os.homedir()` 在 WSL 里返回 `/home/<user>`，所以读 WSL 的 `~/.pi`。

### 0.3 测试前必做

```powershell
# 1. 备份真实配置
$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$bak = "C:\code\cc-switch\backups\test-$stamp"
New-Item -ItemType Directory -Force -Path $bak | Out-Null
foreach ($p in @("$env:USERPROFILE\.cc-switch", "$env:USERPROFILE\.pi\agent")) {
  if (Test-Path $p) { Copy-Item $p "$bak\$(Split-Path $p -Leaf)" -Recurse -Force }
}

# 2. 记录 WSL 侧关键文件哈希（用于事后核对未被意外修改）
wsl -d Ubuntu-22.04 -- bash -c "sha256sum ~/.claude/settings.json ~/.pi/agent/models.json ~/.codex/config.toml 2>/dev/null"
```

---

## 一、测试分层与方法

本方案分四层，**每层解决不同问题**，缺一层就有盲区。

| 层 | 目的 | 能发现什么 | 不能发现什么 |
|---|---|---|---|
| **L1 静态检查** | 代码能否构建 | 编译错误、类型错误 | 运行时行为 |
| **L2 单元测试** | 逻辑是否正确 | 函数级缺陷、边界条件 | 真实文件/真实环境交互 |
| **L3 集成测试** | 组件间是否协同 | 命令层与配置层的对接 | 真实 agent 行为 |
| **L4 端到端** | 用户场景是否真的能用 | 环境错位、权限、路径问题 | — |

**关键教训**：前一轮只做了 L1/L2，就宣称"完成"，结果 L4 一个都没验证。
本方案要求 **L4 必须真实执行并留下证据**。

---

## 二、L1 静态检查

| # | 命令 | 通过标准 | 命令 |
|---|---|---|---|
| L1-1 | TypeScript 类型 | 退出码 0 | `pnpm typecheck` |
| L1-2 | Rust 编译 | 无 `error` | `cargo check --all-targets`（在 `src-tauri`） |
| L1-3 | 前端构建 | 产出 `dist/` | `pnpm build:renderer` |
| L1-4 | Release 构建 | 产出 exe，版本 4.15.0 | `cargo build --release` |

---

## 三、L2 单元测试

| # | 范围 | 命令 | 通过标准 |
|---|---|---|---|
| L2-1 | 前端全量 | `pnpm test:unit` | 228 文件 / 2982 用例全通过 |
| L2-2 | DSH 后端 | `cargo test --lib dsh` | 21 通过 |
| L2-3 | Rust 全量 | `cargo test --lib` | 除既有失败外全通过 |

**已知既有失败（与 DSH 无关，已用 `git stash` 在干净 HEAD 上复现）**：
- `commands::misc::tests::concurrent_lifecycle_runs_use_independent_scripts`
- `services::provider::tests::update_claude_desktop_route_provider_rewrites_profile_in_route_mode`

**注意**：`pi_runtime` / `wsl_linux_harness` 系列依赖真实 WSL，若失败需单独判断。

---

## 四、L3 集成测试（命令层 ↔ 配置层）

用 CC Switch 的命令层直接操作，验证写入结果落在正确的文件、格式正确。

### L3-A 目录解析

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L3-A1 | 读各应用配置目录 | `get_config_dir(app)` | 返回与设置一致的路径 |
| L3-A2 | DSH 目录解析优先级 | 设置 `dshConfigDir` → 清空 → 设 `DSH_HOME` | 依次生效，最后回落 `~/.dsh` |
| L3-A3 | WSL 路径可访问 | 对 `\\wsl.localhost\...` 做 `exists` 检查 | 不超时、不报错 |

### L3-B DSH 配置读写（阶段一核心）

**所有 DSH 测试必须在隔离的 `DSH_HOME` 下做，或先备份 `~/.dsh`。**

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L3-B1 | 空配置首次写入 | 删除 `~/.dsh/cordis.patch.yml` → 新增一张卡片 | 文件被创建，含 `llm-pi-ai` 与正确 `name` |
| L3-B2 | **原有 provider 不被覆盖** | 预置 2 个 provider → 新增第 3 个 | 3 个都在（这是「整体替换」事故的防线） |
| L3-B3 | 编辑卡片 | 改 baseURL → 保存 | 只有该 provider 的 baseURL 变，其它字段不动 |
| L3-B4 | 切换默认模型 | 选另一个 provider 的模型 | `agent-default-model` 的 `provider`/`model` 都更新 |
| L3-B5 | 删除卡片 | 删一张 | 从 patch 移除，其它 provider 保留 |
| L3-B6 | 未知字段保真 | 手工在 patch 里加 `customField: xxx` → 经 CC Switch 改别的 | `customField` 仍在 |
| L3-B7 | 未知条目保真 | patch 里有 `ui-chat` 等非 provider 条目 → 增删 provider | 这些条目原样保留 |
| L3-B8 | 并发写 | 同时发起 2 个写操作 | 不丢数据（文件锁生效） |

**验证方法**：每次操作后直接读文件比对。

```bash
# 查看当前 providers
wsl -d Ubuntu-22.04 -- bash -c "cat ~/.dsh/cordis.patch.yml"

# 或用 DSH 自己确认解析结果（只读，不启动）
wsl -d Ubuntu-22.04 -- bash -c "dsh --profile headless --dump-config | grep -A20 'llm-pi-ai'"
```

### L3-C Pi 配置读写

| # | 用例 | 预期 |
|---|---|---|
| L3-C1 | 读 Pi 卡片 | 与 `~/.pi/agent/models.json` 的 providers 一致 |
| L3-C2 | 新增 Pi 卡片 | 写进 WSL 的 `models.json` |
| L3-C3 | 切 Pi 到路由模式 | `baseUrl` 变 `127.0.0.1:15721`，`apiKey` 变 `PROXY_MANAGED` |
| L3-C4 | **投影守卫** | 重启应用后，数据库卡片仍是真实值（未被占位符覆盖） |
| L3-C5 | 切回直连 | 按数据库卡片还原真实 baseUrl + key |

**L3-C4 是历史事故的防线**，必须验证。

---

## 五、L4 端到端测试（真实 agent）

### 5.1 claude（重点：验证配置错位影响）

| # | 用例 | 操作 | 预期 / 判定 |
|---|---|---|---|
| L4-CL1 | WSL 内 claude 可用 | `wsl -- claude -p "reply OK"` | 返回 `OK` |
| L4-CL2 | 确认读哪份配置 | 改 WSL 侧 baseUrl 为不可达 → 跑 claude | **若仍能通，证明读 Windows 配置**（预期就是这个结果） |
| L4-CL3 | CC Switch 切卡片后是否生效 | 切卡片 → WSL 跑 claude | **判定：是否读到新配置**。若未生效，记录为已知缺陷 |
| L4-CL4 | Windows 侧直接跑 claude | `claude -p "reply OK"` | 返回 `OK`（作为对照） |

**这一组的目的不是"证明能通"，而是"量化错位影响有多大"。**

### 5.2 codex

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L4-CO1 | WSL 内 codex 可用 | `/home/<user>/.npm-global/bin/codex --version` | 0.161.0 |
| L4-CO2 | 确认 PATH 缺失影响 | `wsl -- codex --version` | **可能 NOT_FOUND**（不在 PATH）——记录为待修 |
| L4-CO3 | 实际请求 | 加 PATH 后跑最小请求 | 返回内容 |
| L4-CO4 | CC Switch 切 codex 卡片 | 切卡片 → 跑 codex | 配置生效 |

### 5.3 pi

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L4-PI1 | WSL 内 pi 可用 | `wsl -- pi --version` | 1.1.0 |
| L4-PI2 | pi 读 WSL 配置 | `pi auth print-api-key --provider cpa` | 返回 WSL 侧的 key |
| L4-PI3 | 直连模式请求 | `pi -p --provider cpa --model <m> "reply OK"` | 返回 `OK` |
| L4-PI4 | 路由模式请求 | 切到路由 → 同样请求 | 200，日志显示经本地代理 |
| L4-PI5 | 切回直连后仍可用 | 切回 → 再请求 | 返回 `OK` |

### 5.4 DSH

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L4-DS1 | WSL 内 dsh 可用 | `wsl -- dsh --version` | 0.2.0-rc.2 |
| L4-DS2 | 初始化完成 | `ls ~/.dsh/profiles` | 有 `headless` |
| L4-DS3 | 配置能被 DSH 读到 | 在 CC Switch 加卡片 → `dsh --dump-config` | 出现该 provider |
| L4-DS4 | **原有 provider 未被覆盖** | 加卡片前后对比 | 原有 provider 仍在 |
| L4-DS5 | 实际请求 | `dsh --profile headless "reply OK"` | 返回内容 |
| L4-DS6 | 切换默认模型后生效 | 切默认模型 → 请求 | 用的是新模型 |

**DSH 的密钥需在 WSL 环境变量里设置**（方案 A）：`apiKeyEnv` 指向的变量名必须存在。

### 5.5 CC Switch UI

| # | 用例 | 操作 | 预期 |
|---|---|---|---|
| L4-UI1 | 主窗口能打开 | 托盘唤出 | 渲染完整，非空白 |
| L4-UI2 | 各应用卡片列表 | 逐个切换应用 | 数量与数据库一致 |
| L4-UI3 | DSH 入口 | 设置里打开 DSH 可见性 | 侧栏出现 DSH |
| L4-UI4 | DSH 新增卡片 | 走表单 | 能保存，写进 patch |
| L4-UI5 | 设置页 | 进出 | 不崩溃 |
| L4-UI6 | 用量页 | 打开 | 有数据，无异常 |

---

## 六、回归项（v4.0.14 合并带来的）

| # | 用例 | 预期 |
|---|---|---|
| R1 | Claude Desktop 模式迁移 | 启动日志出现迁移提示（或已迁移完成） |
| R2 | Pi 移除入口 | Pi 卡片「从配置移除」可点，弹窗正常 |
| R3 | Opus 5.5 思维档位 | Pi 预设里 `claude-opus-5-5` 有 `forceAdaptiveThinking` |
| R4 | 用量面板币种切换 | 可切换并持久化 |

---

## 七、执行顺序与判定

```
L1 → L2 → L3 → L4 → R
```

**每一层必须全通过才进入下一层**（L2 的既有失败除外）。

### 通过标准

| 层级 | 标准 |
|---|---|
| L1 | 4 项全过 |
| L2 | 前端 2982 全过；DSH 21 全过；Rust 除 2 个既有失败外全过 |
| L3 | 全部用例通过，且**每次写入后文件内容与预期逐字段比对一致** |
| L4 | **每个 agent 至少有一条真实请求成功**；配置错位项需明确记录结论 |
| R | 4 项全过 |

### 不通过的处置

| 现象 | 判定 | 动作 |
|---|---|---|
| L1/L2 失败 | 代码缺陷 | 修代码，重跑 |
| L3 写入位置错 | 路径/权限问题 | 修代码或调整设置 |
| L3 数据丢失 | **严重缺陷**（违反整体替换防线） | 立即停止，修 `dsh_config` |
| L4 agent 不可用 | 环境问题 | 记录，给出修复步骤 |
| L4 配置错位 | 环境问题（非代码） | 记录，建议改 WSL 的安装方式 |

---

## 八、测试记录模板

每条用例记录：

```
用例编号：
执行时间：
操作步骤：
实际结果：
与预期是否一致：是/否
若否，证据（日志/文件内容/报错）：
结论：
```

---

## 九、已知风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| claude 读 Windows 配置 | 切卡片不生效 | 记录为已知问题；建议在 WSL 装 Linux 版 claude |
| codex 不在 WSL 的 PATH | WSL 内 `codex` 命令不可用 | 加 PATH 或用绝对路径 |
| DSH 密钥需环境变量 | 未设则请求失败 | 在 `~/.bashrc` 里 export |
| 测试会改真实配置 | 可能破坏日常使用 | 测试前备份，测试后核对哈希 |
| WSL 发行版停止时 UNC 不可用 | 相关用例报错 | 先 `wsl -d Ubuntu-22.04 -- true` 唤醒 |
