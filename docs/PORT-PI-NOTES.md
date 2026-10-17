# 定制功能项对照与维护指引

本文记录本仓库相对官方 `farion1231/cc-switch` 的**定制功能项**，用于后续追平官方版本时判断"哪些要保留、哪些已被官方覆盖、冲突时怎么取舍"。

- **上游基线**：`farion1231/cc-switch` @ `v4.0.14`（`5de6feb4`）
- **本仓库版本**：`4.15.0`
- **维护分支**：`port/pi-on-4.0.4`
- **上游远端名**：`upstream`（官方） / `origin`（本 fork）

> 新增、修复、变更等**面向使用者**的说明写入根目录 `CHANGELOG.md`；本文只讲**内部实现与维护决策**。

---

## 一、定制项总览

本仓库的定制全部围绕同一件事：**让 Pi 能经由 CC Switch 的本地代理接管**，并补齐 WSL 场景下的可用性。

| 编号 | 定制项 | 官方 v4.0.14 是否已有 | 状态 |
|---|---|---|---|
| C1 | Pi 代理接管（`models.json` 投影） | ❌ 无 | 生效 |
| C2 | Pi 选家（按 header 分流） | ❌ 无 | 生效 |
| C3 | Pi 用量归因 | ❌ 无 | 生效 |
| C4 | 投影守卫（防止投影回写覆盖真实凭据） | ❌ 无 | 生效 |
| C5 | Pi 模式行（前端直连 / 路由） | ❌ 无 | 生效 |
| C6 | WSL 残留代理清除 | ❌ 无 | 生效 |
| C7 | 用量同步间隔收紧至 20s | 官方为 10 分钟 | 生效 |
| C8 | 私有信息脱敏（密钥 / 服务器地址） | — | 生效 |

**官方已有的 Pi 基础能力**（本仓库直接沿用，不重复实现）：`pi_config/mod.rs`、`services/provider/pi.rs`、`services/session_usage_pi.rs`、`session_manager/providers/pi.rs`、`piProviderPresets.ts`、`piThinkingProfiles.ts`。

---

## 二、逐项说明

### C1 — Pi 代理接管（`models.json` 投影）

**问题**：Pi 的配置形态与 Claude / Codex 不同。它没有 env 变量、也没有 TOML，所有供应商（含各自 `baseUrl` 与 `apiKey`）都集中在一份 `models.json` 里。官方的接管机制（`mode/controller`）基于 env / 行式契约，无法直接套用。

**实现**：接管时把整份 `models.json` 的 `baseUrl` 改写为本地代理地址、`apiKey` 替换为占位符 `PROXY_MANAGED`，退出时按数据库卡片（SSOT）还原。

| 位置 | 作用 |
|---|---|
| `src-tauri/src/pi_config/proxy.rs` | 投影 / 还原 / 判定核心（约 800 行） |
| `src-tauri/src/pi_config/mod.rs` | `mod proxy;` + `pub(crate) use proxy::*;` |
| `src-tauri/src/mode/controller.rs` | `write_proxy` / `write_direct` 的 `AppType::Pi` 分支 |

**关键约束**：Pi 复用 Claude / Codex 的监听，**没有独立的 `proxy_config` 行**。因此 `AppType::supports_local_proxy()` 对 Pi 刻意返回 `false`，但 `has_route_mode()` 必须包含 Pi —— 两者口径不一致会导致 Pi 进不了模式系统。前端 `MODE_APP_IDS` 与后端 `PROXY_APPS` 必须同步。

**SSOT 原则**：数据库卡片是唯一真实来源。投影只是运行时视图，**任何时候都不得把投影写回卡片**（见 C4）。

---

### C2 — Pi 选家（按 header 分流）

**问题**：Pi 与 Codex 共用同一个本地监听端口，代理无法仅凭端口判断请求来自谁。

**实现**：

| 位置 | 作用 |
|---|---|
| `src-tauri/src/proxy/handlers.rs` | `resolve_shared_listen_app()` 依据 `x-cc-switch-app` 头解析实际应用与 tag |
| `src-tauri/src/proxy/provider_router.rs` | `select_providers_for_request()` 分流 + `select_pi_providers()` Pi 专用选家 |
| `src-tauri/src/proxy/handler_context.rs` | Pi 分支：关故障转移、按模型名选家 |
| `src-tauri/src/proxy/providers/mod.rs` | `get_adapter()` 中 `AppType::Pi` → `CodexAdapter`（原为 `return None`）；注意 `from_app_type_and_config()` 里 Pi 仍返回 `None`，两者勿混 |

**关键约束（易踩）**：转发用 `AppType::Pi`，但传给 `RequestContext::new` 的**服务串必须保持监听应用**（`"codex"`）。`pi_api_matches_listen_app()`（`pi_config/proxy.rs`）的 `match` 只认 `"claude"` / `"codex"` / `"gemini"`，其余分支直接 `false` —— 传 `"pi"` 会让全部候选落选并返回 `503`。

**选家顺序**：Pi 目录过滤 → 与 live `models.json` 取交集 → 按模型名收窄 → `copy_pi_base_url_for_adapters`。

---

### C3 — Pi 用量归因

**实现**：`handler_context.rs` 中 `app_type_str` 改为 `usage_app_type_from_headers(headers, app_type_str)`，在 `RequestContext::new` 内完成归因，使请求虽经 Codex 适配器转发，用量仍计入 `pi`。

配套：`src-tauri/src/services/session_usage.rs`、`src-tauri/src/services/session_usage_pi.rs`、`src-tauri/src/session_manager/providers/pi.rs` 的会话扫描增量优化。

---

### C4 — 投影守卫（关键，勿删）

**问题**：接管期间 `models.json` 里的节点是投影值。启动或同步时若把这份投影同步回数据库，会把所有 Pi 卡片的 `baseUrl` 覆盖成 `127.0.0.1`、`apiKey` 覆盖成 `PROXY_MANAGED`，于是全部卡片判定为"不可转发" → 选家零候选 → `503`。

**实现**：`src-tauri/src/services/provider/pi.rs` 的 `sync_native_locked()` 内：

```rust
for (id, config) in native {
    if crate::pi_config::is_projected_provider_node(config) {
        continue; // 投影不是真实配置，绝不写回卡片
    }
    ...
}
```

**这是整个定制的地基**。追平官方时若该文件被上游覆盖，必须重新加回此守卫，否则 Pi 路由会静默失效（表现为 503，日志为 `[pi] [FO-005] 未配置可转发的 Pi 供应商`）。

---

### C5 — Pi 模式行（前端）

**实现**：Pi 支持「直连 / 路由」两种模式（不支持聚合 / Stack）。

| 位置 | 作用 |
|---|---|
| `src/config/appConfig.tsx` | `pi` 加入 `PROXY_APP_IDS` → 进入 `MODE_APP_IDS` |
| `src/components/settings/sections/RoutingSection.tsx` | 路由页展示 Pi 当前路由到谁 |
| `src-tauri/src/proxy/types.rs` | `ProxyTakeoverStatus` 的 `pi` 字段 |
| `src-tauri/src/services/proxy.rs` | `get_takeover_status()` 读取 Pi 模式标志 |

**易踩点**：Pi 进入 `MODE_APP_IDS` 后会改走 `SwitchModePanel` 分支，而非普通 `ProviderList` 分支。所有为 Pi 提供的列表回调（如 `onRemoveFromConfig`）必须放进 `listCallbacks`，否则该分支下功能缺失。

---

### C6 — WSL 残留代理清除

**问题**：WSL 的 `autoProxy` 会把 Windows 系统代理（如 v2rayN 的 `127.0.0.1:10808`）注入 WSL 环境变量。代理进程退出后变量仍残留，导致 WSL 内所有 HTTP 请求撞向无人监听的端口，CPA / 各 API 全部连不上。

**实现**：`src-tauri/src/commands/misc.rs` 的 `WSL_STALE_PROXY_GUARD`，在执行 WSL CLI 前探测代理端点。

**设计要点（宁可不清理，也不误清）**，以下情况一律保留代理：

1. 端口无法解析为纯数字（如 `http_proxy=http://proxy.internal` 未带端口）——`${hp##*:}` 会回退成整个主机名，探测必然失败，属假阴性；
2. 目标发行版没有 `bash`（`/dev/tcp` 是 bash 特有特性）——探测根本跑不起来。

对应测试：`wsl_proxy_guard_keeps_proxy_when_port_is_unparseable`、`wsl_proxy_guard_requires_bash_before_probing`。

---

### C7 — 用量同步间隔

`src-tauri/src/lib.rs` 中会话用量同步间隔**本仓库为 20 秒**，官方为 10 分钟兜底。

**理由**：会话文件位于 WSL 的 UNC 路径上，单次 `stat` 约 1ms。实测一轮（遍历 57 个会话 + 57 次 `stat` + 未改动文件整轮跳过）约 70–100ms，20s 间隔约 0.5% CPU，换来新用量最多晚 20 秒入库。未改动文件由增量游标（mtime + 尾部指纹）整轮跳过，不会重复解析。

**追平官方时**：若上游改动此处，保留 20s 或重新评估，不要直接接受 10 分钟。

---

### C8 — 私有信息脱敏

| 位置 | 处理 |
|---|---|
| `.gitignore` | 忽略 `docs/CPA-*.md`（私有部署笔记，含密钥与服务器 IP）。用 ASCII 通配因中文路径直接写入会因 GBK 编码匹配失败 |
| `scripts/fix-cli-to-cpa.ps1` | CPA 地址与密钥从 `CCS_CPA_BASE_URL` / `CCS_CPA_API_KEY` 读取，未配置时优雅跳过 |
| `scripts/restore-pi-cards.mjs` | 7 个上游密钥改为 `CCS_PI_*_KEY` / `CCS_PI_*_BASE_URL` 环境变量，缺失时跳过对应卡片 |

**原则**：任何进 git 的文件不得出现真实密钥或服务器地址。

---

## 三、追平官方时的检查清单

按此顺序做，可避免本文档记录的坑。

1. **先确认版本**。核对 `git ls-remote --tags upstream`，以官方远端实际存在的 tag 为准；不要凭本地 tag 排序判断最新版。本仓库历史上曾错把 fork 自建的 tag 当作官方版本。
2. **确认 schema 兼容**。检查 `SCHEMA_VERSION`（`src-tauri/src/database/mod.rs`）是否 ≥ 现有数据库的 `user_version`。若小于，应用会进入"数据库版本过新"阻断页。
3. **试合并并统计冲突**：`git merge upstream/main --no-commit --no-ff`，重点看本文列出的定制文件。
4. **冲突解决原则**：
   - 双方各加枚举 / 数组项 → **都保留**（如 `ProxyTakeoverStatus` 字段、`PROXY_APPS`）；
   - 不同 `AppType` 的 match 分支 → **必须并存**，不能二选一；
   - 涉及 C7 间隔、C6 代理策略等**行为差异** → 保留本仓库取值并核对理由；
   - 数据类文件（如 `piThinkingProfiles.ts`）→ 先确认本仓库是否有独有内容，官方若为超集则整体采用官方版。
5. **合并后必查**：
   - `src-tauri/src/services/provider/pi.rs` 的投影守卫（C4）是否还在；
   - `AppType::has_route_mode()` 是否仍包含 `Pi`；
   - 前端 `PROXY_APP_IDS` 与后端 `PROXY_APPS` 是否一致。
6. **验证**：
   ```
   pnpm typecheck
   pnpm test:unit
   cargo check --all-targets
   ```
   基线：`pnpm test:unit` 应为 227 文件 / 2968 用例全通过；`cargo check` 应无 error。

---

## 四、定制文件清单

相对官方 `v4.0.14` 有差异的文件（共 41 个，以下为与定制相关的部分）：

**后端**

| 文件 | 定制内容 |
|---|---|
| `src-tauri/src/pi_config/proxy.rs` | **新增**。投影 / 还原核心 |
| `src-tauri/src/pi_config/mod.rs` | `mod proxy`、WSL 路径解析、文档修订锁、native defaults |
| `src-tauri/src/pi_config/fixtures/models_with_unknown_fields.json` | **新增**。测试夹具 |
| `src-tauri/src/mode/controller.rs` | `PROXY_APPS` 含 Pi、`write_proxy` / `write_direct` 的 Pi 分支 |
| `src-tauri/src/services/provider/pi.rs` | **投影守卫**（C4） |
| `src-tauri/src/proxy/provider_router.rs` | `select_providers_for_request`、`select_pi_providers` |
| `src-tauri/src/proxy/handler_context.rs` | Pi 分支 + 用量归因 |
| `src-tauri/src/proxy/handlers.rs` | `resolve_shared_listen_app` |
| `src-tauri/src/proxy/providers/mod.rs` | `get_adapter()` 中 `AppType::Pi` → `CodexAdapter`（原为 `return None`）；注意 `from_app_type_and_config()` 里 Pi 仍返回 `None`，两者勿混 |
| `src-tauri/src/proxy/providers/streaming_codex_anthropic.rs` | Pi 流式协议适配 |
| `src-tauri/src/proxy/types.rs` | `ProxyTakeoverStatus.pi` |
| `src-tauri/src/services/proxy.rs` | `get_takeover_status()` 含 Pi |
| `src-tauri/src/services/session_usage.rs`、`session_usage_pi.rs` | Pi 用量与会话扫描 |
| `src-tauri/src/session_manager/providers/pi.rs` | Pi 会话 |
| `src-tauri/src/lib.rs` | 会话同步间隔 20s（C7） |
| `src-tauri/src/commands/misc.rs` | WSL 残留代理清除（C6） |
| `src-tauri/src/app_config.rs` | `has_route_mode()` 含 Pi |

**前端**

| 文件 | 定制内容 |
|---|---|
| `src/config/appConfig.tsx` | `pi` ∈ `PROXY_APP_IDS` |
| `src/components/settings/sections/RoutingSection.tsx` | Pi 路由展示 |
| `src/lib/api/config.ts`、`src/lib/api/connectivity-check.ts` | Pi 相关 API |

**脚本**（本地维护工具，不参与构建）

| 文件 | 用途 |
|---|---|
| `scripts/restore-pi-cards.mjs` | 卡片被投影污染时写回真实配置 |
| `scripts/build-pi-models.mjs` | 按数据库生成 `models.json`（`real` / `proxy` 两种模式） |
| `scripts/check-pi-headers.py` | 检查 `models.json` 各 provider 的 header 与 api |
| `scripts/pi-routing-patch.diff` | 路由补丁存档，便于回溯 |
| `scripts/fix-cli-to-cpa.ps1` | Windows + WSL(mirrored) 下把 claude / pi 修成直连 CPA |

---

## 五、已知环境问题

| 现象 | 原因 | 处理 |
|---|---|---|
| MSI 打包失败，`light.exe` 报 `ICE38` | 上游 wix 模板 + 无代码签名 | 改用 `pnpm tauri build --bundles nsis` |
| NSIS 打包失败，下载工具链超时 | GitHub 不可达或代理 TLS 握手失败 | 挂可用代理后重试 |
| 启动日志 `Skipping Codex state DB on a WSL path` | Windows 无法对 `\\wsl.localhost` 上的 SQLite 加锁 | 官方设计行为，非缺陷 |
| 应用启动后主窗口不显示 | 静默启动模式（`静默启动模式：主窗口已隐藏`），常驻托盘 | 从托盘图标唤出，非缺陷 |

---

## 六、开发注意事项

1. **Windows 下 `Out-File` 会写 UTF-8 BOM**，Rust 会因此编译失败。从 git 取文件后若出现异常编译错误，先检查 BOM。
2. **覆盖正在运行的 exe 会静默失败**。先 `Stop-Process`，覆盖后用 `Get-FileHash` 确认一致再启动；否则容易拿旧二进制测出错误结论。
3. **PowerShell 5.1 不支持 `param()` 默认值里的 `if` 表达式**，需在 `param` 块之后计算。
4. **带中文的 `.ps1` 必须存为 UTF-8 with BOM**，否则 PowerShell 5.1 按 GBK 解析会报语法错误。
5. **同名模型会让 Pi CLI 拒绝选家**（`More than one matching provider is authenticated`）。增删卡片时注意模型名不要重复。
