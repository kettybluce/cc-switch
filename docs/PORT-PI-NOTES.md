# Pi 移植到官方 4.0.4 —— 进度与剩余工作

分支：`port/pi-on-4.0.4`（基线 = 官方 `a7f66764` / upstream/main / v4.0.4）
对照源：`main`（3.20.15，你的定制线）
目标：除 Pi 外全部跟官方走，保留 Pi 的代理接管与 WSL 能力

---

## 已完成

### 1. 纯新增文件（从 main 检出，无冲突）
| 文件 | 说明 |
|---|---|
| `src-tauri/src/pi_config/proxy.rs` | **Pi 代理接管核心**（767 行）|
| `src-tauri/src/pi_config/fixtures/models_with_unknown_fields.json` | 测试夹具 |
| `src/lib/api/config.ts` | 前端 API |
| `src/lib/api/connectivity-check.ts` | 前端 API |

### 2. `src-tauri/src/pi_config/mod.rs` 整file替换
- 官方 576 行 → fork 986 行
- 新增：`mod proxy;`、`resolve_pi_agent_dir`、`inferred_wsl_pi_agent_dir`、
  `wsl_unc_home_from_app_config_dir`、`first_model_id`、
  `write_pi_native_defaults`、`set_pi_default_provider`、
  `reassign_pi_default_if_current`、`lock_models_file`、
  `read_models_document_with_revision`、`write_models_document`、`test_support`
- 依赖检查：`settings.rs` 的 `get_pi_override_dir` / `resolve_override_path` /
  `get_claude_override_dir` / `get_codex_override_dir` **官方 4.0.4 已存在**，无需移植

### 3. 验证
```
cargo check  ->  Finished (编译通过)
```
剩余 39 个 warning 多为 `never used`，正是"接线未做"的证据：
`pi_api_matches_listen_app` / `pi_provider_has_model` /
`pi_provider_is_forwardable` / `copy_pi_base_url_for_adapters` 尚未被调用。

---

## 剩余工作（按依赖顺序）

### A. 后端接线（关键路径）
1. **`services/proxy.rs`** —— 官方**完全没有** Pi 接管逻辑。
   需按 fork 版补 6 处 `AppType::Pi` 分支：
   - `takeover_live_config` / `best_effort` → `project_live_models_to_proxy(&proxy_url)`
   - restore 路径 → `restore_live_models_document(config)`
   - 读取路径 → `read_pi_models_document()`
   - `cleanup_pi_takeover_placeholders_in_live()`
2. **`proxy/provider_router.rs`** —— `select_providers_for_request` /
   `select_pi_providers`（按 `x-cc-switch-app` 头走 Pi 目录）
3. **`proxy/handler_context.rs`** —— `request_is_pi` 分支、usage 归因
4. **`proxy/handlers.rs` + `proxy/providers/mod.rs`** —— 我上游修好的补丁
   （已存 `scripts/pi-routing-patch.diff`，10274 字节）
5. **`services/provider/pi.rs`** —— 412+/50-，Pi 原生 provider 同步
6. **`commands/pi.rs`** —— 8 行
7. **`services/session_usage_pi.rs`** —— 12+/2-
8. **`session_manager/providers/pi.rs`** —— 111+/84-
9. **`proxy/providers/transform_codex_anthropic.rs` / `streaming_codex_anthropic.rs`**
   —— Pi 的 Anthropic 协议转译

### B. 前端（可延后，不影响 Pi 命令行可用）
- `PiProviderForm.tsx`（55+/161-）
- `PiNativePromptResources.tsx`（541+/779-）
- `PiPromptPanel.tsx`（199+/518-）
- `piProviderPresets.ts`（68+/237-）
- `src/lib/api/{proxy,providers,sessions,pi}.ts` 等
- 相关 tests

### C. 收尾
- `pnpm build:renderer` + `cargo build --release`
- `pnpm tauri build --bundles nsis`
- WSL Pi 1.0.4 实测（`--provider cpa` / 不带 provider）
- 全量 `cargo test --lib`

---

## 说明
- `pi_blocks.rs` / `useVisibleAppIds.ts` / `pi.messages.json` 三项是我最初误判：
  它们**不在 fork main 上**，属官方文件或已删除文件，**不需要移植**。
- 移植时注意：用 `Out-File` 从 git 取文件会加 **UTF-8 BOM**，Rust 会报错，
  必须清掉（已确认 `pi_config/mod.rs` 已清）。
