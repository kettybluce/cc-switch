# DSH 接入：需求拆解、实现进度与测试清单

> 面向 Windows + WSL 场景的实际使用。本文替代 `DSH-PLAN.md` 作为接手入口；
> 调研依据仍看 `DSH-INTEGRATION-RESEARCH.md`。
>
> 代码基线：`port/pi-on-4.0.4` @ `767ada67`（前端接入 DSH 那次提交）。
> 最后更新：2026-10-17。

---

## 一、需求拆解

你的原话拆成五条可验收的需求：

| # | 需求 | 验收标准 |
|---|---|---|
| R1 | 在 WSL 里装好 DSH，并能正常跑起来 | WSL 内 `dsh --profile headless "hello"` 有回答，`~/.dsh` 被创建 |
| R2 | 在 CC Switch 里改配置 → WSL 里 DSH 的模型配置跟着变 | CC Switch 里新增/编辑/删除卡片后，`~/.dsh/cordis.patch.yml` 的 `llm-pi-ai.providers` 同步变化；**原有 provider 不丢** |
| R3 | 切换默认模型 | CC Switch 里选某个供应商/模型后，`agent-default-model` 的 `provider` / `model` 正确更新 |
| R4 | 使用记录能在 CC Switch 里看到 | 用量页能按 `dsh` 筛选，能看到 DSH 的 token / 花费 / 请求数 |
| R5 | 更新日志、会话 | 「应用」页能看到 DSH 版本与更新；会话页能列出 DSH 会话并读内容 |

场景约束（会影响实现取舍，不是可选项）：

- CC Switch 跑在 **Windows**，DSH 跑在 **WSL**。配置文件在 UNC 路径
  `\\wsl.localhost\<distro>\home\<user>\.dsh` 上。
- WSL 里主力是 claude / codex / pi。**DSH 是新增的第四个，不能影响前三个**。
- DSH 在 WSL 里只有 `0.2.0-rc.2`；Windows 侧那个 `0.1.0-rc.6` 是生产环境，不许动。

---

## 二、实现进度

| 阶段 | 范围 | 对应需求 | 后端 | 前端 | 结论 |
|---|---|---|---|---|---|
| 一 | 模型配置同步 | R1 R2 R3 | 完成 | **本轮补齐** | 待验证 |
| 二 | 用量 | R4 | 未开始 | 未开始 | — |
| 三 | 会话 | R5 | 未开始 | 未开始 | — |
| 四 | 更新日志 | R5 | 未开始 | 未开始 | — |

后端的底子是真的打好了：`dsh_config.rs` 的「读全量 → 内存合并 → 整写回」+ 原子写 +
进程内文件锁 + 11 个单测，把「providers 整体替换」那个坑挡住了。`AppType::Dsh`
接入 51 处 match。这部分不用重写。

问题集中在前端接线和几个语义缺口上，已在本轮修掉（详见第七节）。

---

## 三、代码审查发现的问题

按能不能挡住你使用排序。**P0 = 不走一遍就用不了**。

### P0-1 「应用」页没有 DSH 行，侧栏打不开它 ✅ 已修

`DEFAULT_VISIBLE_APPS.dsh = false`（前端）与 `VisibleApps::default().dsh = false`
（后端）一致，DSH 默认不在侧栏。这是刻意设计——没装 DSH 的人不该看见它。

但 `AppsPage` 的行来自 `TOOL_NAMES`（`useToolManagement.ts:41-51`），里面**没有 `dsh`**。
「应用」页每行渲染的可见性开关只在这份列表里，于是：

- 侧栏不显示 DSH（默认 false）；
- 「应用」页又没有 DSH 那一行，**没有任何 UI 能把它打开**。

用户唯一的出路是手改 `~/.cc-switch/settings.json` 的 `visibleApps.dsh = true`。

**这几乎肯定就是你「干了一半」卡住的地方。**

修法：`AppsPage` 的 `Row` 类型加 `{ kind: "visibilityOnly"; app }`，DSH 走这条分支——
只显示名字、一句「不在此页管理安装与版本」和侧栏开关。DSH 装在 WSL、走 nightly 通道，
版本探测（T5）本来就不在这套 `TOOL_NAMES` 机制里，先不硬塞。

### P0-2 新建 DSH 卡片拿不到 provider key ✅ 已修

`src/lib/query/mutations.ts` 原来只给 opencode / openclaw / hermes / pi / mcode
透传 `providerKey`，`dsh` 落到 `else` 分支发 UUID。而 `AddProviderDialog` 的透传名单
同样没有 dsh。两处都得加。

后果：卡片 `id` 是 UUID，后端 `dsh::add` 拿它当 patch 的映射键，你会看到
`a3f2e8c1-...` 这种供应商名；且 key 一旦写进 patch 就不能改名（`dsh::update` 拒绝）。

**这一条比 P0-1 更根本**：即使你能进页面，建出来的卡片也没法用。

修法：新增 `DshProviderForm`（参照 `McodeProviderForm`），提供 key 输入 + 校验 +
重复检测；两处透传名单都加 `dsh`。表单收 `apiKeyEnv`（变量名）、`api`、`baseURL`、
`models`，底部保留完整 JSON 作逃生舱，并对「自定义 route 缺 api/baseURL/models」就地报错。

### P0-3 数据库里有卡、patch 里没有时，卡片会消失 ✅ 已修

`dsh::list` 原来**只以 patch 为准**。而 `dsh::remove`（只摘 patch 留卡）和
`add(add_to_live = false)` 都会产生「库里有卡、patch 里没有」，这种卡再也看不见。

对比 `pi::list`：它先 `sync_native_locked` 把 native 并进 db，再返回 `db` 的内容。

修法：`dsh::list` 合并两边——patch 里的按 patch 顺序置顶（配置以 patch 为准，
身份以库为准），纯库的卡片排在后面。两者都标 `meta.liveConfigManaged`
（true / false），前端据此分区，与 OpenCode / OpenClaw / MCode 同一字段。

### P0-4 前后端对 DSH 的 additive 判定不一致 ✅ 已修

| 位置 | 原来 | 现在 |
|---|---|---|
| 后端 `AppType::is_additive_mode()` | 是 | 是 |
| 前端 `ADDITIVE_APP_IDS` | **否** | 是 |

后果是 `ProviderService::current()` 对 additive 返回空串 → `isCurrent` 恒 false →
`ProviderCard` 的 `shouldAutoQuery = isAdditiveAppId ? isInConfig : isCurrent` 走到
`isCurrent` → 用量脚本永不自动查询，卡片也拿不到「当前」标记。

### P1-1 `dsh::add` 里的凭证归一化作用在临时克隆上，等于没做 ✅ 已修

```rust
ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut {
    provider.clone()
});
```

传的是 `&mut { provider.clone() }` —— 改一份临时克隆然后丢掉。对比 `pi::add:43`
传 `&mut provider`。已改为 `mut provider: Provider` + `&mut provider`。

### P1-2 「导入当前配置」对 DSH 无效 ✅ 已修

`import_dsh_providers_from_live` 写好了但没有 Tauri command；`ProviderList` 的
`importMutation` 落到 `importDefault`，而它对 additive 应用直接 `return Ok(false)`。

修法：新增 `src-tauri/src/commands/dsh.rs`，含 `get_dsh_live_provider_ids`、
`import_dsh_providers_from_live`、`set_dsh_default_model` 三个命令，注册进
`commands/mod.rs` 与 `lib.rs` 的 `generate_handler!`；前端加对应 API 与 importMutation 分支。

### P1-3 `apiKeyEnv` 语义没接住 ⬜ 缺口，非 bug

DSH 用 `apiKeyEnv: <环境变量名>`，配置里存的是变量名不是密钥。但
`provider.rs:233` 的 `resolve_usage_credentials` 对 Dsh 读 `settings.apiKey`（明文），
实际配置里没这个字段 → `{{apiKey}}` 永远是空串。

这是方案 A 的固有结果，不是缺陷。阶段二做用量前必须先决定 DSH 的用量走哪条路：
代理记账（阶段一没做本地代理，走不通）还是读 `session.v3.jsonl.zstd`。
若是后者，压根不需要 apiKey。

### P1-4 默认模型只有一个入口，且是强制的 ✅ 已修

`dsh::enable` 原来无条件把 `models[0]` 写成默认，用户不能选具体模型；卡片没填
`models` 时只写 provider、不写 default，DSH 仍指旧家，**看起来切了实际没切**。

修法：

- `enable` 的选择顺序改为「已指向这家且模型仍有效 → 保留；否则 `defaultModel`；
  否则 `models[0]`」。三者都无时返回 warning，不再静默。
- 新增 `set_dsh_default_model` 命令，前端 `setAsDefaultModel` 对 dsh 走它，
  `presentation.ts` 给 DSH 卡片加「设为默认」按钮，多模型时用菜单选。
- `update` 时若编辑把默认模型从卡片里删了，自动回退到卡片里第一个模型，
  避免 `agent-default-model` 指向不存在的模型导致 DSH 启动报错。

### P1-5 写入前不校验 DSH 的可加载性 ◐ 前端已拦，后端待补

`validate_provider_json` 只要求「key 非空 + 是对象」，而 DSH 对目录外的自定义 route
要求 `api` + `baseURL` + 非空 `models`，否则拒载，且错误只在 DSH 启动时暴露。

修法（表单层）：`DshProviderForm` 检测「三项填了任意一项但没填全」时报
`dsh.form.incompleteRoute`，挡住保存。

**后端仍未校验**：如果以后有别的写入路径（deep-link、云同步恢复），还需要在
`validate_provider_settings` 的 `AppType::Dsh` 分支里补同样的检查。见第八节。

### P2-1 DSH 目录默认为 Windows 的 `~/.dsh`，静默落错位置 ⬜ 未修

`resolve_dsh_home()` 第三步解析成 `C:\Users\<user>\.dsh`。没在设置里填 UNC 路径时，
写入会在 Windows 侧静默创建 `.dsh` 并写进去，WSL 里什么都没有。

建议：`~/.dsh` 不存在、且能探测到 WSL 时，在 UI 上提示把目录指向
`\\wsl.localhost\<distro>\home\<user>\.dsh`。探测方式需要先跟你确认，未动手。

### P2-2 `apps.dsh` 的 i18n 文案缺失 ✅ 已修

5 个语言的 `apps` 段都补了 `dsh: "DeepSeek Harness"`，另加 `appsPage.noToolManagement`
与整套 `dsh.form.*`。

### P2-3 用量页与会话页没有 DSH ⬜ 阶段二/三，符合进度

`src/types/usage.ts:235` 的 `KNOWN_APP_TYPES` 不含 `dsh`，
`session_manager/mod.rs:56` 的 `scan_sessions` 也不含。见 T3 / T4。

---

## 四、测试清单

按「能不能测」排序。**打 ✅ 才算过**。

### T0 环境准备（WSL 侧，一次性）

| ID | 项 | 命令 / 观察点 |
|---|---|---|
| T0-1 | 确认 WSL 发行版与网络模式 | `wsl -l -v`；`cat /etc/wsl.conf` 看 `networkingMode` |
| T0-2 | 确认 DSH 已安装 | `/home/<user>/.npm-global/bin/dsh --version` → `0.2.0-rc.2` |
| T0-3 | 初始化 DSH home | `dsh --profile headless "hello"`，等它跑完 |
| T0-4 | 确认目录生成 | `ls -la ~/.dsh` → 至少应有 `profiles/`；`cordis.patch.yml` 可能还没有 |
| T0-5 | 备份原始配置 | `cp ~/.dsh/cordis.patch.yml{,.bak-$(date +%s)}`（文件不存在则跳过） |
| T0-6 | 确认 UNC 可从 Windows 读 | 资源管理器打开 `\\wsl.localhost\<distro>\home\<user>\.dsh` |

> 注：`networkingMode=mirrored` 时 WSL 的 `127.0.0.1` 与 Windows 互通，
> 这对阶段二（如果走本地代理）很关键；`autoProxy=false` 则说明不会有残留代理问题。

### T1 配置同步（阶段一，R1/R2/R3）

| ID | 项 | 期望 | 状态 |
|---|---|---|---|
| T1-1 | 设置 → 应用配置 → DSH 目录填 `\\wsl.localhost\<distro>\home\<user>\.dsh`，保存 | 重启后仍在 | 待测 |
| T1-2 | 「应用」页找到 DSH 行，打开侧栏显示 | 侧栏出现 DeepSeek Harness | 待测 |
| T1-3 | 点 DSH → 新增供应商，填 key / baseURL / api / apiKeyEnv / models | 新建成功 | 待测 |
| T1-4 | 检查 provider key | **不是 UUID**，是用户填的 | 待测 |
| T1-5 | `cat ~/.dsh/cordis.patch.yml` | `llm-pi-ai.config.providers` 下有这条 | 待测 |
| T1-6 | **保真测试**：先在 WSL 里手写第二条 provider（带未知字段），再在 CC Switch 编辑第一条 | 手写那条与未知字段**原样保留** | 待测 |
| T1-7 | 编辑卡片改 baseURL | patch 里同步更新 | 待测 |
| T1-8 | 「从配置移除」 | 卡片留在「可添加」区，patch 里消失 | 待测 |
| T1-9 | 彻底删除 | patch 与卡片都消失 | 待测 |
| T1-10 | 「设为默认」（多模型） | 能选具体模型；`agent-default-model` 同步 | 待测 |
| T1-11 | WSL 里跑 `dsh --profile headless "hi"` | 用的是新配置的模型，请求成功 | 待测 |
| T1-12 | 存一条缺 `api` 或空 `models` 的配置 | 保存时被表单拦下 | 待测 |
| T1-13 | 「导入当前配置」 | 把 WSL 里已有的 provider 导进 CC Switch | 待测 |
| T1-14 | 复制卡片 | key 带 `-copy` 后缀，不与已有键冲突 | 待测 |
| T1-15 | 把 DSH 目录指向不存在的路径后新增 | 报错清晰（**已知会落到 Windows 侧**，见 P2-1） | 待测 |

### T2 回归（不影响 claude / codex / pi）

| ID | 项 | 期望 |
|---|---|---|
| T2-1 | Claude 卡片增删改、切换 | 行为与改前一致 |
| T2-2 | Codex 卡片增删改、切换 | 同上 |
| T2-3 | Pi 卡片增删改、切换、路由接管 | 同上；`models.json` 投影与还原正常 |
| T2-4 | 用量页现有应用数据 | 数字不变 |
| T2-5 | 会话页现有应用列表 | 数量不变 |
| T2-6 | 启动耗时、托盘、静默启动 | 无退化 |
| T2-7 | `pnpm test:unit` | 227 文件 / 2968 用例基线不降 |

### T3 用量（阶段二，未实现）

| ID | 项 | 备注 |
|---|---|---|
| T3-1 | 用量页筛选器出现 DSH | 需先把 `dsh` 加进 `KNOWN_APP_TYPES` |
| T3-2 | 一次 DSH 请求能计入 | 需先定数据源：代理记账 or 会话日志 |
| T3-3 | 会话日志解析 | zstd + v3 格式，解压后按 JSONL 读 |
| T3-4 | 花费计算 | 复用 `model_pricing` |

> 已确认的技术前提：会话落在 `<root>/--<cwd>--/<id>/session.v3.jsonl.zstd`，
> 按 `compression: 'zstd'` 存。Rust 侧已有 `zstd` 依赖（`Cargo.toml:46`），可用。

### T4 会话（阶段三，未实现）

| ID | 项 | 备注 |
|---|---|---|
| T4-1 | 会话页出现 DSH 分组 | 需写 `session_manager/providers/dsh.rs` |
| T4-2 | 列出会话（标题、时间、消息数） | zstd 解压后读 header + 事件流 |
| T4-3 | 打开会话看消息 | DSH 事件是 `session/*` / `step/*` / `tool/*`，需映射到 `SessionMessage` |
| T4-4 | 删除会话 | 删整个会话目录 |

### T5 更新日志（阶段四，未实现）

| ID | 项 | 备注 |
|---|---|---|
| T5-1 | 「应用」页显示 DSH 版本 | 依赖给 `TOOL_NAMES` 加 dsh 或另开探测路径 |
| T5-2 | 版本检测 | DSH 在 WSL，需走 `wsl` 探测（参考 `probeToolInstallations`） |
| T5-3 | 更新通道 | nightly：`download.deepseek.com/dsh-desk/feeds/win-x64/` |

---

## 五、修复顺序

先能看见，再能用，最后才谈扩展。本轮按此顺序做完：

1. **P0-1** — 「应用」页加 DSH 行。否则后面全部测不了。✅
2. **P0-2** — DSH 卡片接 provider key（用户手填，与 Pi 一致）。✅
3. **P0-3** — `dsh::list` 合并数据库卡片；`isInConfig` 按 patch 判定。✅
4. **P0-4** — `ADDITIVE_APP_IDS` 加 `dsh`，让前后端判定一致。✅
5. **P1-1** — `&mut { provider.clone() }` → `&mut provider`。✅
6. **P1-2** — 加 `import_dsh_providers_from_live` 的 Tauri command + 前端分支。✅
7. **P1-4** — 默认模型选择入口。✅
8. **P1-5** — 可加载性校验（前端已拦，后端待补）。◐
9. **P2-2** — i18n 文案。✅ / **P2-1** 目录陷阱 ⬜

阶段二/三/四不在本轮范围，但 T3-3 的技术前提已经确认可行。

---

## 六、验证方式

改完后按这个顺序过：

```
pnpm typecheck                 # 必须 0 error
pnpm test:unit                 # 基线 227 文件 / 2968 用例
cargo check --all-targets
cargo test --lib dsh_config    # 11 个单测
```

然后手工过 T1-1 → T1-11，T2-1 → T2-3。

> ⚠️ **本轮未能跑通上述任何一条命令**：连接 `\\wsl.localhost\...` 目录后，
> 沙箱 shell 报 `UNC paths are not supported`，所有 `bash` 调用失败（子代理同样失败）。
> 以下改动只做过静态交叉核对，**未经编译与运行验证**，请先跑一遍上面的命令。
> 需要重开会话（只连 `C:\code\cc-switch`）才能恢复 shell。

---

## 七、本轮改动清单

### 新增文件

| 文件 | 作用 |
|---|---|
| `src/components/providers/forms/DshProviderForm.tsx` | DSH 专用表单：key 校验、apiKeyEnv、api/baseURL、models、完整 JSON 逃生舱、route 完整性预检 |
| `src-tauri/src/commands/dsh.rs` | `get_dsh_live_provider_ids` / `import_dsh_providers_from_live` / `set_dsh_default_model` |
| `docs/DSH-TEST-PLAN.md` | 本文 |

### 修改文件

| 文件 | 改动 |
|---|---|
| `src/config/appConfig.tsx` | `ADDITIVE_APP_IDS` 加 `dsh` |
| `src/components/apps/AppsPage.tsx` | `Row` 加 `visibilityOnly` 分支，DSH 有侧栏开关 |
| `src/components/providers/forms/ProviderForm.tsx` | 分发到 `DshProviderForm` |
| `src/components/providers/AddProviderDialog.tsx` | 透传 `providerKey`、DSH 关掉通用供应商页、`baseURL` 进 custom endpoints、表单 ready 判定 |
| `src/components/providers/EditProviderDialog.tsx` | DSH 不读 live（用库里的）、表单 ready 判定 |
| `src/components/providers/ProviderList.tsx` | `isInConfig` 的 dsh 分支、导入分支 |
| `src/components/providers/presentation.ts` | DSH 卡片「设为默认」+ 模型菜单 |
| `src/hooks/useProviderActions.ts` | `setAsDefaultModel` 的 dsh 分支、切换成功文案 |
| `src/App.tsx` | `onRemoveFromConfig` 加 dsh、`onSetAsDefault` 加 dsh、复制卡片读 DSH live 键 |
| `src/lib/query/mutations.ts` | providerKey 透传名单加 dsh |
| `src/lib/api/providers.ts` | 三个 DSH API |
| `src/utils/providerConfigUtils.ts` | `extractProviderBaseUrl` 认 `baseURL` |
| `src/i18n/locales/*.json`（5 个） | `apps.dsh`、`appsPage.noToolManagement`、`dsh.form.*` |
| `src-tauri/src/services/provider/dsh.rs` | `list` 合并数据库、`add` 修归一化、`enable` 模型选择顺序、`update` 修悬空默认、新增 `set_default_model` |
| `src-tauri/src/services/provider/mod.rs` | 暴露 `set_dsh_default_model` |
| `src-tauri/src/commands/mod.rs` | `mod dsh` + `pub use dsh::*` |
| `src-tauri/src/lib.rs` | 注册三个新命令 |

---

## 八、遗留与待确认

1. **编译未验证**（见第六节）。这是最要紧的一条。
2. **后端未做 route 可加载性校验**：`validate_provider_settings` 的 `AppType::Dsh`
   分支仍只调 `validate_provider_json`。前端拦得住表单，拦不住 deep-link 导入和
   云同步恢复。要补的话，加一条「填了 `api`/`baseURL`/`models` 任一就必须三项齐全」。
3. **P2-1 目录陷阱**未处理：需要在「Windows 侧 `~/.dsh` 不存在但探测到 WSL」时给提示。
   探测方式（读 `wslpath`？扫 `.wslconfig`？）需要你定。
4. **`resolve_usage_credentials` 对 Dsh 读 `apiKey`**：阶段二定数据源后再决定是否要改。
5. **`dsh::update` 拒绝改名**：表单已把 key 输入框设为 `disabled={isEdit}`，
   但后端仍会在真被改名时报一句英文错。可接受，未做中英文化。
6. **DSH 版本探测**：`TOOL_NAMES` 没加 dsh，所以 T5-1/T5-2 做不了。
   DSH 在 WSL + nightly 通道，与现有 npm 探测机制不同，需单独设计。
