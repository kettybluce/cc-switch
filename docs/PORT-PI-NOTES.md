# Pi 移植到官方 4.0.4 —— 进度与剩余工作

分支：`port/pi-on-4.0.4`（基线 = 官方 `a7f66764` / upstream/main / v4.0.4）
对照源：`main`（3.20.15，你的定制线）
目标：除 Pi 外全部跟官方走，保留 Pi 的代理接管与 WSL 能力

---

### ✅ 已确认可用：Pi 路由本身在 4.0.4 上是通的

加临时日志（`resolve_shared_listen_app`）实测证明：

```
[PI-DIAG] x-cc-switch-app=Some("pi") is_pi=true default_app=codex
[Codex] >>> 请求目标: https://api.saviour.cc.cd/v1/responses (model=gpt-6-astra)
[pi] [FWD-003] Provider ZeroAPI 请求失败
```

关键点：
- 头**到达了** handler（`Some("pi")`）
- `is_pi=true`，我的补丁**生效**
- 转发日志同时带 `[Codex] >>>`（适配器名）和 `[pi]`（tag）—— 符合预期
  （Pi 复用 Codex 适配器，tag 归因到 pi）
- 选中的是 **Pi 卡片** `saviour`（名字叫 "ZeroAPI"，baseUrl
  `https://api.saviour.cc.cd/v1`），不是 Codex 路由

### ⛔ 真正的阻塞：启动时把投影后的 models.json 同步回 DB，覆盖真实凭据

现象：应用每次启动后，**所有 7 张 Pi 卡片**都被写成

```
baseUrl = http://127.0.0.1:15721/v1
apiKey  = PROXY_MANAGED
```

于是 `pi_config::pi_provider_is_forwardable` 判定**全部不可转发** →
`select_pi_providers` 零候选 → 日志 `[pi] [FO-005] 未配置可转发的 Pi 供应商 (listen=codex)` → 503。

`sync_native_locked`（`services/provider/pi.rs:282`）本来有
`if is_projected_provider_node(config) { continue; }` 守卫，而且
`is_projected_provider_node` 对 `apiKey == PROXY_MANAGED` 会返回 true ——
所以是**别的路径**在写。下一步应定位：谁在启动时把投影写进了 Pi 卡片，
（候选：`mode/controller.rs` 的 attach 流程、`services/provider/pi.rs:245`
的 `save_provider(PI_APP, ...)`、或 `pi_state.rs`）。

**修复方向（二选一）**：
1. 在写回 DB 前加同样的 `is_projected_provider_node` 守卫，宁可跳过也不要
   用占位符覆盖真实凭据；
2. 让 `select_pi_providers` 不依赖 DB 卡片里的 baseUrl/apiKey，而是从
   **未投影的来源**取（但这需要真实值有个存放处，否则同样被覆盖）。

**临时规避**（已验证可行）：停掉应用 →
`node scripts/restore-pi-cards.mjs <db>` 写回真实配置 →
`node scripts/build-pi-models.mjs <db> <models.json> real` →
**不要启动应用**，此时 Pi 卡片与 models.json 都是真实配置。

---

## 交付物

| 包 | 说明 |
|---|---|
| `CC Switch_4.0.4_x64-setup.exe` (11.9 MB) | 官方 4.0.4 + Pi 后端移植。**编译/打包/测试都过**，但 Pi 代理路由被上面那个同步 bug 挡住 |
| `CC Switch_3.20.15_x64-setup.exe` (11.1 MB) | 原 fork + Pi 路由修复，**Pi 实测 200 OK** |

⚠️ 4.0.4 二进制已在 `C:\Users\<user>\AppData\Local\CC Switch\cc-switch.exe` 上跑过（触发 DB 迁移）。
迁移前的数据库备份：`~/.cc-switch/cc-switch.db.bak-before-404-20261007_224210`。

---

## 已完成的移植（编译 + 测试通过）

### 新增文件
`pi_config/proxy.rs`、`pi_config/fixtures/*`、`lib/api/config.ts`、
`lib/api/connectivity-check.ts`

### `pi_config/mod.rs`（576 → 986 行）
### `container_router` → `provider_router.rs`
新增 `select_providers_for_request` + `select_pi_providers`（+ `HashSet` import）
### `handler_context.rs`
Pi 分支（关故障转移、从 Pi 目录选家）+ 用量归因
### `handlers.rs`
`resolve_shared_listen_app()` + 两个入口改用 `AppType::Pi`（适配 7 参 `stack`）
### `providers/mod.rs`
`AppType::Pi` → `CodexAdapter`（`Mcode` 保持 `return None`）
### `mode/controller.rs`
`write_proxy` / `write_direct` 加 `AppType::Pi` 分支 + `restore_pi_models_from_ssot()`

---

## 踩过的坑
1. **`Out-File` 加 UTF-8 BOM** → Rust 编译失败，必须清
2. **`git switch` 被未提交改动静默中止** → 先存 `scripts/pi-routing-patch.diff`
3. `pi_blocks.rs` / `useVisibleAppIds.ts` / `pi.messages.json` 是**官方文件**，不需移植
4. 官方 `AppType` 多一个 `Mcode`，改 `get_adapter` 时别顺手改它
5. 官方 4.0.4 缺 `tailwindcss-animate`（fork 的 node_modules 里没有）→ 先 `pnpm install`
6. **`cp` 覆盖正在运行的 exe 会静默失败** → 必须先 `Stop-Process`，
   然后用 `Get-FileHash` 确认一致；我曾因此拿错误二进制测了一轮



提交：
- `f5153c33` wip(pi): port Pi config module + proxy core
- `7681a316` feat(pi): wire Pi provider routing onto 4.0.4

### 关键发现：官方 4.0.4 的架构和 fork 完全不同
| | fork (3.20.15) | 官方 4.0.4 |
|---|---|---|
| 接管实现 | `services/proxy.rs` + `proxy_live_backup` 备份表 | `mode/controller.rs`（6121 行）"per-app 直连/代理模式" |
| 选家 API | `select_providers_for_request` | `select_providers` / `select_providers_with_current` |
| `RequestContext::new` | 6 参 | **7 参（多 `stack`）** |
| `ProxyTakeoverStatus` | 含 `pi` 字段 | 不含 |
| `AppType` | 无 `Mcode` | **有 `Mcode`** |

所以这不是"复制 fork 代码"，而是**按官方新架构重新实现 Pi 接管**。

---

## 已完成

### 1. 新增文件（从 main 检出）
`pi_config/proxy.rs`（767 行，Pi 接管核心）、`pi_config/fixtures/*`、
`lib/api/config.ts`、`lib/api/connectivity-check.ts`

### 2. `pi_config/mod.rs` 整file替换（576 → 986 行）
新增 `mod proxy;`、`resolve_pi_agent_dir`、`inferred_wsl_pi_agent_dir`、
`wsl_unc_home_from_app_config_dir`、`first_model_id`、`write_pi_native_defaults`、
`set_pi_default_provider`、`reassign_pi_default_if_current`、`lock_models_file`、
`read_models_document_with_revision`、`write_models_document`、`test_support`。
依赖的 `settings.rs` 四个函数官方已有，无需移植。

### 3. `provider_router.rs` —— Pi 选家（已完成）
新增 `select_providers_for_request`（按 header 分流）+
`select_pi_providers`（Pi 目录过滤 → live models.json 交集 →
模型名收窄 → `copy_pi_base_url_for_adapters`）。
`HashSet` 已加入 import。

### 4. `handler_context.rs` —— Pi 分支（已完成）
- 新增 `None if request_is_pi_client(headers)` 分支：关故障转移、
  从 Pi 目录按模型选家、`current_provider_id` 取自选中家
- `app_type_str` 改为 `usage_app_type_from_headers(headers, app_type_str)`，
  用量归因到 `pi`

### 5. `handlers.rs` —— 路由补丁（已完成，已适配 7 参）
- 新增 `resolve_shared_listen_app(headers, default_app, default_tag) -> (AppType, tag)`
- `handle_chat_completions`：`AppType::Codex` → 解析后的 `app_type`；
  `RequestContext::new` 传 `app_type.clone()` / `tag` / `"codex"` / `stack`
- `handle_responses_for_app`：新增 `effective_app` / `effective_tag`，
  转发与 compaction 判定改用 `effective_app`
- ⚠️ **服务串必须保持 `"codex"`**：`pi_api_matches_listen_app` 只认
  claude/codex/gemini，传 `"pi"` 会让全部候选落选 → `503 未配置供应商`

### 6. `providers/mod.rs` —— 适配器（已完成）
`AppType::Pi` 由 `return None` 改为 `Box::new(CodexAdapter::new())`
（与 GrokBuild 一致，具体 wire protocol 由 provider 级判定）。
**注意官方多一个 `Mcode` 变体，仍是 `return None`，不要顺手改掉。**

---

## 剩余工作

### A. `mode/controller.rs`（6121 行）—— 最后一个后端阻塞点
官方接管走这里的 `enter` / `enter_with_route` / `exit` / `write_proxy` /
`takeover`。文件里 **完全没有 `AppType::Pi` 分支**，需要：
- `enter` / `enter_with_route`：Pi 进入时把 `models.json` 投影到本地代理
  （`crate::pi_config::project_live_models_to_proxy`）
- `exit`：还原 `models.json`（`restore_live_models_document`）
- `takeover(live, provider)`：Pi 的 `models.json` 不是 env 结构，需要单独分支
- 启动恢复路径（`cleanup_pi_takeover_placeholders_in_live`）

### B. 其余后端文件
- `services/provider/pi.rs`（412+/50-）：Pi 原生 provider CRUD 同步
- `services/session_usage_pi.rs`（12+/2-）
- `session_manager/providers/pi.rs`（111+/84-）
- `proxy/providers/transform_codex_anthropic.rs` / `streaming_codex_anthropic.rs`
- `commands/pi.rs`（8 行）

### C. 前端
`PiProviderForm.tsx`、`PiNativePromptResources.tsx`（1320 行改动）、
`PiPromptPanel.tsx`、`piProviderPresets.ts`、`lib/api/{proxy,providers,sessions,pi}.ts`

### D. 收尾
`pnpm build:renderer` → `cargo build --release` → `pnpm tauri build --bundles nsis`
→ WSL Pi 1.0.4 实测 → 全量 `cargo test --lib`

---

## 踩过的坑
1. **`Out-File` 会加 UTF-8 BOM** → Rust 直接编译失败。从 git 取文件后必须清 BOM。
2. **`git switch` 会被未提交改动静默中止** → 先把补丁存成
   `scripts/pi-routing-patch.diff`（10274 字节）。
3. `pi_blocks.rs` / `useVisibleAppIds.ts` / `pi.messages.json` 是**官方文件**，
   不在 fork main 上，**不需要移植**（我最初误判）。
4. 官方 `AppType` 多了 `Mcode`，改 `get_adapter` 时容易漏掉它的 `return None`。

