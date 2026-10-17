# DSH 接入工作计划与进度

> 目标：让 CC Switch 能像管理 claude/codex/pi 那样管理 DeepSeek Harness（DSH）的模型配置。
> 本文记录完整计划、已完成部分、剩余工作与关键约束，供后续接手。

## 一、总体目标与阶段划分

| 阶段 | 范围 | 状态 |
|---|---|---|
| 阶段一 | 模型配置同步：CC Switch 改卡片 → DSH 的 `cordis.patch.yml` 生效 | **代码已完成并通过测试，等真实环境端到端验证** |
| 阶段二 | 使用记录（用量） | 未开始 |
| 阶段三 | 会话列表 | 未开始 |
| 阶段四 | 更新日志（DSH 走 nightly 通道） | 未开始 |

用户已确认：
- 采用**方案 A** 处理密钥——CC Switch 只管 `baseURL` / `api` / `models`，密钥由用户自己在 WSL 环境变量里维护；
- DSH 装在 **WSL**，由用户手动初始化（跑一次 `dsh`）。

## 二、环境现状

| 项 | 值 |
|---|---|
| WSL 发行版 | Ubuntu-22.04（`networkingMode=mirrored`，`autoProxy=false`） |
| WSL 内 DSH | `@deepseek-ai/dsh` **0.2.0-rc.2**，命令 `/home/<user>/.npm-global/bin/dsh` |
| WSL 的 `~/.dsh` | **尚不存在**（等用户跑一次 `dsh` 初始化） |
| Windows 侧 DSH | 0.1.0-rc.6，**属生产环境，禁止改动** |
| CC Switch 写入点 | `$DSH_HOME/cordis.patch.yml`（home 级 patch，优先级最高） |

### ⚠️ 不可违背的规则

**验证 DSH 配置必须用隔离的 `DSH_HOME`，绝不可写入用户真实的 `~/.dsh`。**

```bash
export DSH_HOME=/tmp/dsh-sandbox
```

此前曾因直接写用户 Windows 侧的真实 `~/.dsh/cordis.patch.yml`，用测试桩覆盖了其正在使用的供应商配置，导致 DSH 无法工作。

## 三、核心技术结论（均已实测）

### 1. 配置树叠加顺序

后者覆盖前者：

```
空根
 └ bundles 的 patch
   └ profile 的 cordis.patch.yml
     └ $DSH_HOME/cordis.patch.yml      ← CC Switch 写这里
       └ --patch 命令行
```

### 2. providers 是整体替换，不是深合并

实测：profile 层写 `user-real`、home 层写 `ccs-new`，合并后**只剩 `ccs-new`**。

**因此所有写操作必须「读全量 → 内存合并 → 整体写回」，绝不能只写自己那一项。**

### 3. 模型配置结构

```yaml
- id: llm-pi-ai
  config:
    providers:
      <key>:
        displayName: 显示名
        apiKeyEnv: 环境变量名      # 密钥只存变量名
        api: openai-completions
        baseURL: http://host/v1
        models:
          - id: 模型 ID
- id: agent-default-model
  config:
    provider: <key>                # 当前选中
    model: <modelId>
```

## 四、已完成工作

### 提交记录（阶段一）

| 提交 | 内容 |
|---|---|
| `445d64fe` | 调研结论文档化 |
| `90b5b2ae` | `dsh_config` 模块：读/合并/写/保真 + 11 个单测 |
| `2e7837d3` | `AppType::Dsh` 接入后端 27 处 match（14 文件） |
| `cd891343` | `services/provider/dsh.rs` + ProviderService 7 个路由点 |
| `767ada67` | 前端接入：应用标识、目录设置、图标、i18n（17 文件） |
| `9b375f7f` | 阶段一收尾：DSH 专用表单、命令层、应用页接入（24 文件） |
| `4c7888d8` | 本文档 |

### 后端（全部完成）

| 提交 | 内容 |
|---|---|
| `445d64fe` | 调研结论文档化 |
| `90b5b2ae` | `dsh_config` 模块：读/合并/写/保真 + **11 个单测** |
| `2e7837d3` | `AppType::Dsh` 接入后端 **27 处 match**（14 文件） |
| `cd891343` | `services/provider/dsh.rs` + ProviderService **7 个路由点** |

**`dsh_config` 的关键 API**

| 函数 | 作用 |
|---|---|
| `resolve_dsh_home()` | 优先级：设置里的 `dshConfigDir` → `DSH_HOME` → `~/.dsh` |
| `read_providers()` | 以 JSON 形式读出全部供应商 |
| `upsert_provider(key, config)` | **唯一写入口**，内部走读改写全流程 |
| `remove_provider(key)` | 删除（幂等） |
| `write_default_model(provider, model)` | 设置默认模型 |
| `validate_provider_json` | 写库前校验 |

**安全设计**
- `PATCH_FILE_LOCK` 串行化并发写；
- `write_patch_atomic` 先写临时文件再改名；
- 遇到结构异常（如 `config: oops`）拒绝改写而非猜测修复；
- 未知字段与未知条目全程保留。

**单测防线**（`cargo test --lib dsh`，12 通过）
- `apply_providers_patch_never_drops_existing_providers` ← 防「整体替换」事故
- `apply_providers_patch_preserves_unknown_fields`
- `refuses_to_rewrite_non_mapping_nodes`
- `writes_atomically_and_round_trips_on_disk`

**`services/provider/dsh.rs` 与 Pi 的差异**
1. 无投影/还原概念（阶段一不做本地代理接管）；
2. `list` 以 patch 为准而非数据库，避免「库里有卡但 DSH 看不见」；
3. `delete`（彻底删）与 `remove`（移除但留卡）都映射到「从 patch 移除」；
4. `enable` 顺带把该 provider 的首个模型设为默认；
5. `add` 在数据库保存失败时回滚已写入 patch 的项。

### 前端（已完成）

| 文件 | 改动 |
|---|---|
| `src/lib/api/types.ts` | `AppId` 加 `"dsh"` |
| `src/config/appConfig.tsx` | `APP_IDS`、`DEFAULT_VISIBLE_APPS`（**dsh: false，默认隐藏**）、`APP_ICON_MAP`（青色主题）、`McpAppId` 排除 dsh |
| `src/types.ts` | `VisibleApps.dsh`、`Settings.dshConfigDir` |
| `src/hooks/useDirectorySettings.ts` | 目录元信息 `.dsh`、各处 Record 补 dsh |
| `src/components/settings/sections/AppConfigSection.tsx` | `dshConfigDir` 字段与占位符 |
| `src/components/shell/AppGlyph.tsx` | 应用名 `DeepSeek Harness`、图标 `deepseek` |
| `src/components/usage/RequestLogTable.tsx` | 短名 `DSH` |
| `src/components/providers/forms/EndpointSpeedTest.tsx` | 超时 8s |
| `src/lib/api/skills.ts` | `SkillApps.dsh` 可选 |
| `src/components/providers/forms/DshProviderForm.tsx` | **新增**，DSH 专用表单（不收 API Key，只收环境变量名） |
| `src/components/providers/forms/ProviderForm.tsx` | `appId === "dsh"` 时返回 DshProviderForm |
| `src/components/providers/AddProviderDialog.tsx`、`EditProviderDialog.tsx`、`ProviderList.tsx`、`presentation.ts`、`hooks/useProviderActions.ts`、`utils/providerConfigUtils.ts`、`lib/api/providers.ts`、`lib/query/mutations.ts`、`apps/AppsPage.tsx`、`App.tsx` | 识别 dsh |
| `src-tauri/src/commands/dsh.rs` | **新增**，`import_dsh_providers_from_live` / `get_dsh_live_provider_ids` / `set_dsh_default_model` |
| `src-tauri/src/commands/mod.rs`、`lib.rs` | 注册上述命令 |
| `src/i18n/locales/*.json` | 5 个语言补 DSH 表单与提示文案 |
| `tests/msw/state.ts`、`SessionManagerPage.test.tsx`、`NewLayoutDialog.test.tsx`、`useDirectorySettings.test.tsx` | mock 与断言适配 |

### 已验证

- `cargo check --lib` 无 error
- `pnpm typecheck` 通过（EXIT=0）
- `pnpm test:unit` **227 文件 / 2968 用例全部通过**
- `cargo test --lib dsh` **12 通过**

## 五、剩余工作

### 5.1 前端

无阻塞项。F1–F4 均已完成。

### 5.2 端到端验证（阻塞中，唯一阻塞点）

**依赖**：WSL 的 `~/.dsh` 存在。用户需先执行：

```bash
dsh --profile headless "hello"
```

然后验证：
1. CC Switch 设置里把 DSH 的目录指向 `\\wsl.localhost\Ubuntu-22.04\home\<user>\.dsh`；
2. 在 CC Switch 里新增一张 DSH 卡片 → 检查 `~/.dsh/cordis.patch.yml` 被正确写入；
3. 原有的 provider 未被覆盖（这是防护重点）；
4. 切换默认模型 → `agent-default-model` 正确更新；
5. 在 WSL 里跑 `dsh` 确认读到了新配置。

详细步骤见 `docs/DSH-TEST-PLAN.md`。

### 5.3 阶段二/三/四（未开始）

| 阶段 | 需要先搞清的事 |
|---|---|
| 用量 | `~/.dsh/sessions/*.jsonl.zstd` 为 zstd 压缩，格式待分析 |
| 会话 | 同上；另需确认 DSH 是否有稳定的会话索引 |
| 更新日志 | DSH 是 nightly 通道（`download.deepseek.com/dsh-desk/feeds/win-x64/`），与现有工具版本检测机制不同 |

## 六、已知遗留

- **端到端未验证**：阶段一所有代码已写完并通过静态检查与测试，但**从未在真实 DSH 上跑过**。唯一的阻塞是 WSL 的 `~/.dsh` 不存在（等用户初始化）。
- **新增应用会拖挂计数型断言**：`NewLayoutDialog` 曾硬编码「getAll 被调 10 次」，加 dsh 后失败；已改为 `APP_IDS.length`。但项目里可能还有同类硬编码，下次新增应用时注意。
- **DSH 默认隐藏**：`VisibleApps.dsh` 为 false，用户需在「应用」页手动打开才会在侧栏出现。
- **密钥需用户自管**：方案 A 下 CC Switch 不写 key，用户要在 WSL 环境变量里设置 `apiKeyEnv` 指向的变量名。

## 七、关键文件索引

| 文件 | 作用 |
|---|---|
| `src-tauri/src/dsh_config.rs` | 配置读写核心 + 单测 |
| `src-tauri/src/services/provider/dsh.rs` | 业务服务 |
| `src-tauri/src/services/provider/mod.rs` | 7 个路由点 |
| `src-tauri/src/commands/dsh.rs` | Tauri 命令 |
| `src/components/providers/forms/DshProviderForm.tsx` | DSH 专用表单 |
| `src-tauri/src/app_config.rs` | `AppType::Dsh` |
| `src-tauri/src/settings.rs` | `VisibleApps.dsh`、`dsh_config_dir` |
| `docs/DSH-INTEGRATION-RESEARCH.md` | 调研结论（含实测证据） |
| `docs/DSH-TEST-PLAN.md` | 端到端测试计划 |
| 本文 | 计划与进度 |
