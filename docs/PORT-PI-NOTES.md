# Pi 移植到官方 4.0.4 —— 进度与剩余工作

分支：`port/pi-on-4.0.4`（基线 = 官方 `a7f66764` / upstream/main / v4.0.4）
对照源：`main`（3.20.15，你的定制线）
目标：除 Pi 外全部跟官方走，保留 Pi 的代理接管与 WSL 能力

---

## 当前状态（已验证）

```
cargo check                                 -> OK
cargo test --lib                            -> 3370 passed, 2 failed（均 pre-existing）
pnpm run build:renderer                      -> OK（需先 pnpm install，官方 4.0.4 的
                                                tailwindcss-animate 在 fork 的 node_modules 里没有）
cargo build --release                        -> OK
pnpm tauri build --bundles nsis              -> CC Switch_4.0.4_x64-setup.exe (11.9 MB)
```

两个测试失败与本次移植无关，已用 worktree 在纯净 `a7f66764` 上复现：
- `services::provider::...proxy_takeover_is_active`：需要能用的 15721 端口（CC Switch 在跑时占用）
- `commands::misc::...concurrent_lifecycle_runs_use_independent_scripts`：纯净基线同样失败

### ⚠️ 未解决：4.0.4 上 Pi 请求仍按 Codex 路由

现象：`models.json` 已正确投影（7 个 provider、`baseUrl=http://127.0.0.1:15721/v1`、
`apiKey=PROXY_MANAGED`、每个都带 `headers={'x-cc-switch-app': 'pi'}`），但
带 `x-cc-switch-app: pi` 的请求返回：

```
502 {"provider":"ZeroAPI","model":"gpt-6-astra","endpoint":"/responses",
     "cause":"转发失败: 上游连接失败"}
```

即走了 **Codex** 的 `ZeroAPI`，不是 Pi 目录。

已排除：`server.rs:340` 的 `/v1/responses` 确实指向 `handlers::handle_responses`
→ `handle_responses_for_app`（我打过补丁的那个），所以不是路由表问题。

**结论：`crate::pi_config::request_is_pi_client(&headers)` 返回了 false**，
于是 `effective_app` 仍是 `AppType::Codex`。

下一步排查：
1. 在 `handle_responses_for_app` 里加临时 `log::warn!` 打印
   `headers.get("x-cc-switch-app")`，确认头是否真的到达 handler
2. 检查 `decode_codex_request_body(&mut headers, ...)` 是否动过/删掉了该头
   （它在 `RequestContext::new` 之前被调用）
3. 确认 `pi_config::request_is_pi_client` 的实现是精确匹配
   `x-cc-switch-app == "pi"`（大小写不敏感），而不是读别的头名
4. 若头确实到达但没生效，检查 `mode/controller.rs:708` / `:1044` 的
   `official_provider_supports_proxy_takeover` 是否在进入前就把 Pi 挡掉了


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

