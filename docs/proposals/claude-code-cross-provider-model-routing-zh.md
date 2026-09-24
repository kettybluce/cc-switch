# CC Switch：Claude Code `/model` 跨供应商模型路由 — 实现方案

> 调研基线：本地仓库 `origin/main` ≡ tag `v3.20.10`（commit `a5aed50a`）。  
> 约束：`SCHEMA_VERSION` 保持 **18**；复用现有本地代理；不镜像会话到 Windows 用户目录。  
> 调研时间：2026-09-24 CST。  
> **实现状态（feat/claude-cross-provider-model-routing）**：已按设计 A + 默认项落地（一期）。  
> - clientModel 默认：`{providerId}/{upstreamModel}`（碰撞安全；label 可用显示名）  
> - `replaceBuiltInOptions` 默认 false；`writeAvailableModels` / gateway discovery 默认 false（一期不实现 Anthropic `/v1/models`）  
> - 存储：settings KV `claude_model_routing`；无 SCHEMA / 迁移  
> - UI：设置 → 「Claude 跨供应商模型目录」手风琴  
> - 目录命中：单供应商、禁 failover、不热切换当前供应商；日志 `provider_id` = 目标供应商  

---

## 1. 结论先行

**可行。** Claude Code 在 `ANTHROPIC_BASE_URL` 指向本地代理时，会把任意 `model` 字符串原样发给网关（不做官方 API 那套识别校验）；再配合官方 `modelPicker` / `availableModels` /（可选）`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY`，就能在 `/model` 里列出并切换多供应商模型。CC Switch 侧已有两处强先例——**Pi 按请求 `model` + 供应商目录选上游**（`ProviderRouter::select_pi_providers`），以及 **Claude Desktop 的 `meta.claudeDesktopModelRoutes` 路由表**（`map_proxy_request_model`）——可在**不 bump SCHEMA** 的前提下，把「Claude Code 请求里的 client model」解析成「目标 `provider_id` + 上游真实 model」，走现有 listen 端口、鉴权、格式转换与 `proxy_request_logs` 管线。

**推荐路径**：在代理接管开启时，由 CC Switch 维护一张可编辑的 **clientModel → (providerId, upstreamModel)** 目录，投影到 WSL `~/.claude/settings.json` 的 `modelPicker`（+ 可选 `availableModels`）；代理在 `select_providers_for_request` 中命中目录则按表路由，**禁止**因此触发 `FailoverSwitchManager` 热切换「当前供应商」；未命中目录则保持今日「当前供应商 / 故障转移队列」语义。

---

## 2. Claude Code 能力事实（含文档引用）

文档主源：

- Model configuration：https://code.claude.com/docs/en/model-config  
- Environment variables：https://code.claude.com/docs/en/env-vars  
- Settings reference（`modelPicker` / `availableModels`）：https://code.claude.com/docs/en/settings-reference  
- Gateway protocol（`/v1/models` discovery）：https://code.claude.com/docs/en/llm-gateway-protocol  

### 2.1 `/model` 能做什么

| 能力 | 事实 | 版本依赖 |
| --- | --- | --- |
| 切换方式 | `/model <alias\|name>` 立即切换；无参打开 picker；`--model` / `ANTHROPIC_MODEL` / settings.`model` | 文档基线行为 |
| 别名 | `default` / `best` / `fable` / `sonnet` / `opus` / `haiku` / `opusplan` 及 `[1m]` 变体 | 持续演进；Fable/Opus5 等有最低版本要求 |
| 别名解析覆盖 | `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU,FABLE}_MODEL` | 文档「Environment variables」表 |
| 网关下任意字符串 | **自定义 `ANTHROPIC_BASE_URL` / LLM gateway 时，Claude Code 不对 model id 做「是否认识」校验，任意字符串透传** | model-config：「behind an LLM gateway or a custom ANTHROPIC_BASE_URL … passes any string through without checking it」 |
| 官方 API 校验 | 直连 Anthropic API 时，Remote Control / SDK 等路径会拒不认识的 id | v2.1.200+（Remote Control pick 另有 v2.1.260+）——**本方案走本地代理，不受此限** |

### 2.2 如何把「多模型」放进 picker

| 机制 | 能力 | 限制 | 版本 |
| --- | --- | --- | --- |
| `ANTHROPIC_CUSTOM_MODEL_OPTION`（+ `_NAME` / `_DESCRIPTION`） | **只能追加 1 条**自定义行 | 不够「任意多家」 | 文档现有 |
| `modelPicker.options[]` | 自定义多行、自定 label/description；`replaceBuiltInOptions: true` 可只显示自定义行 + Default + 当前会话模型 | 仅 **User / managed / `--settings`**，不读 project/local；多源不合并 | **v2.1.242+** |
| `availableModels` | 允许列表；可让「无内置行的完整 id」出现为独立 labeled row | 企业 managed 列表会覆盖用户扩展 | v2.1.199 起 full-id 行行为更完整 |
| `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` | 启动时 `GET {ANTHROPIC_BASE_URL}/v1/models?limit=1000`，结果进 picker（标 From gateway / description） | **默认关**；**id 必须包含 `claude` 或 `anthropic`（大小写不敏感）才会保留**；3s 超时（可用 `CLAUDE_CODE_GATEWAY_MODEL_DISCOVERY_TIMEOUT_MS`）；不跟随 redirect | discovery **v2.1.129+**；过滤放宽到「包含」为 **v2.1.223+** |

**对方案的硬含义**：

1. **非 Claude 品牌模型**（如 `deepseek-chat`、`gpt-4o`）**不能指望**仅靠 gateway discovery 进 picker——会被过滤；应用 **`modelPicker`（主）+ `availableModels`（辅）**。  
2. 若仍想走 discovery，client id 需刻意带 `claude`/`anthropic` 子串（例如 Claude Desktop 式安全 route id），再由代理映射到真实上游 id。  
3. 用户也可直接 `/model <任意字符串>`（网关模式），但产品体验要求必须有可见 picker 行。

### 2.3 用户实际怎么选模型（本目标场景）

Windows 上 Claude Code 跑在 WSL，`settings.json` 在  
`\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude\settings.json`（CC Switch 已支持 WSL UNC 写路径，见 `config.rs`）。  
接管模式下 `env.ANTHROPIC_BASE_URL` 指向本地代理；用户在会话内 `/model` → 选 `modelPicker` 行（或别名）→ 请求 body.`model` = 该行的 model 字符串 → 代理按表路由。

---

## 3. 当前 CC Switch 机制（文件 + 函数）

### 3.1 Claude 请求如何选上游（今日：单活跃供应商）

- `ProviderRouter::select_providers(app_type)`（`src-tauri/src/proxy/provider_router.rs`）  
  - failover 关：仅 `get_effective_current_provider` / `get_current_provider`  
  - failover 开：仅按 `get_failover_queue` 顺序，**忽略「当前」是否在队中**  
- `ProviderRouter::select_providers_for_request(listen_app, headers, request_model)`  
  - 若 `pi_config::request_is_pi_client(headers)` → `select_pi_providers`  
  - **否则忽略 `request_model`，走 `select_providers(listen_app)`** ← Claude Code 正落在此分支  
- `RequestContext::new`（`proxy/handler_context.rs`）调用上述选择；非 Pi 时 `current_provider_id` 取自 settings 当前供应商，供后续热切换判断。

### 3.2 `model` 字段如何改写（今日：供应商内角色映射）

- `ModelMapping::from_provider` / `apply_model_mapping`（`proxy/model_mapper.rs`）  
  - 读该供应商 `settings_config.env` 的 `ANTHROPIC_DEFAULT_*` / `ANTHROPIC_MODEL`  
  - 按请求 model 是否含 `fable|haiku|opus|sonnet` 关键词映射；无命中用 `ANTHROPIC_MODEL`；再 `strip_one_m_suffix_for_upstream`  
- **只在「已选定的那一个 Provider」内改名，不跨供应商。**

### 3.3 接管时写入 Claude live config

- `ProxyService::apply_claude_takeover_fields_*`（`services/proxy.rs`）  
  - `ANTHROPIC_BASE_URL` → 本地 proxy origin  
  - token 键改为 `PROXY_MANAGED`  
  - 角色 env 写成**稳定客户端别名**（如 `claude-sonnet-5` / `claude-opus-5` / `claude-haiku-4-5` / `claude-fable-5`），显示名用上游真实名（`*_MODEL_NAME`）  
  - 真正上游 model 仍靠请求时 `apply_model_mapping`  
- 写入路径：`write_claude_live` / `sync_claude_live_from_provider_while_proxy_active` → `get_claude_settings_path()`（可指向 WSL UNC）。

### 3.4 Pi 路由先例（最接近的「按 model 选供应商」）

- 投影：`pi_config/proxy.rs::project_provider_node` — 多供应商写入 `~/.pi/agent/models.json`，`baseUrl`→共享 listen，`apiKey`→`PROXY_MANAGED`，注入头 `x-cc-switch-app: pi`  
- 选择：`select_pi_providers` — `pi_provider_is_forwardable` + `pi_api_matches_listen_app` + **`pi_provider_has_model(settings, request_model)`**  
- 记账：`usage_app_type_from_headers` → `"pi"`；**关闭**继承 Claude failover / 热切换（`RequestContext` 内 `auto_failover_enabled = false`）  
- SCHEMA 18 注释明确：Pi **不能**占用 `proxy_config.app_type='pi'`，复用 Claude listen。

### 3.5 Claude Desktop 路由先例（单供应商内的 route 表）

- 存储：`ProviderMeta.claude_desktop_model_routes: HashMap<routeId, ClaudeDesktopModelRoute>`（`provider.rs`，**meta JSON，无迁移**）  
- 解析：`claude_desktop_config::proxy_model_routes` / `map_proxy_request_model`  
- `/claude-desktop/v1/models` → `model_list_response`（Anthropic 风格 `data[]`）  
- 转发：`forwarder.rs` 仅在 `AppType::ClaudeDesktop` 时调用 `map_proxy_request_model`  
- **仍绑定「当前 Claude Desktop 供应商」**，不是跨 Claude 供应商目录。

### 3.6 API 格式转换（Claude 供应商已支持）

- `get_claude_api_format` / `claude_api_format_needs_transform` / `transform_claude_request_for_api_format`（`proxy/providers/claude.rs`）  
- 格式：`anthropic`（透传）| `openai_chat` | `openai_responses` | `gemini_native`  
- SSOT：`meta.apiFormat`（不写进 Claude settings.json；`sanitize_claude_settings_for_live` 会剥掉）  
- 跨供应商路由时：**以目标供应商的 apiFormat 决定转换**，与今日单供应商逻辑一致。

### 3.7 请求日志 / 用量归因

- 表：`proxy_request_logs`（已有 `provider_id`, `app_type`, `model`, `request_model`, `pricing_model`, …）— **无需新列**  
- 写入：`response_processor::log_usage_internal`；`provider_id = ctx.provider.id`；计价锚点优先 `outbound_model`  
- 跨供应商路由后，只要 `ctx.provider` 是真实目标供应商，日志与用量自然归因到正确 `provider_id`。

### 3.8 `/v1/models` 现状（重要缺口）

- 路由：`server.rs` 将 `/v1/models` → `handlers::handle_models`  
- 实现：**只服务 Codex catalog**（`{"models":[...]}`），不是 Anthropic discovery 的 `{"data":[{id,display_name,description}]}`  
- 因此：**不能**在未改 handler 的情况下指望 Claude Code gateway discovery 工作；应优先 `modelPicker`，discovery 作为可选增强（需分流 Claude vs Codex 响应格式）。

### 3.9 SCHEMA 18 可复用存储（无迁移）

| 载体 | 现状 | 用途建议 |
| --- | --- | --- |
| `settings(key,value)` | KV，已存 rectifier/optimizer/log 等 JSON | 全局开关 + 聚合路由表，如 `claude_model_routing` |
| `providers.meta` TEXT JSON | 已有 `claudeDesktopModelRoutes`、`apiFormat`、… | 每供应商「暴露到 Claude `/model`」条目 / 开关 |
| `providers.settings_config` TEXT JSON | env、模型档等 | 可放 `modelCatalog` 暴露列表（与现有 Codex/Zen 目录风格对齐） |
| `proxy_config` | per-app，**无 `pi` 行** | 可选布尔「启用跨供路由」——但 **app_type 枚举扩列会碰迁移**；更稳妥放 `settings` KV |

**禁止**：新表、新列、`user_version` bump。

---

## 4. 候选设计

### 设计 A（推荐）：可配置目录路由 + `modelPicker` 投影

#### 用户体验（Claude Code）

1. CC Switch UI：代理面板新增「Claude 跨供应商模型目录」（显式入口，非隐藏）。  
2. 用户勾选某 Claude 供应商下要暴露的模型（或从该供应商已配置的默认档 / 拉取的 models 列表多选），生成 **clientModel**（见下）与 label。  
3. 接管开启并同步 live 后，WSL `settings.json` 写入：  
   - 现有 `ANTHROPIC_BASE_URL` / placeholder auth  
   - `modelPicker: { options: [...], replaceBuiltInOptions: <可配> }`  
   - 建议同时写 `availableModels: [<所有 clientModel>, "sonnet","opus","haiku","fable"]`（若用户开启「限制列表」）  
   - 可选：`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`（仅当实现 Anthropic 风格 `/v1/models` 且 id 策略兼容过滤时）  
4. 用户在 Claude Code：`/model` → 看到「DeepSeek / xxx」「ProviderB / sonnet」等行 → 发请求。  
5. 角色别名 `sonnet`/`opus`/…：默认仍指向**当前活跃供应商**的 `ANTHROPIC_DEFAULT_*` 映射（兼容旧行为）；目录里也可单独覆盖。

**clientModel 命名（二选一，需用户拍板）**：

- **A1 前缀式**：`{providerId}/{upstreamModel}`（清晰、碰撞少；discovery 过滤器可能丢掉不含 claude/anthropic 的 id → **依赖 modelPicker**）。  
- **A2 安全别名式**（对齐 Claude Desktop）：对外 `claude-*` / 含 `anthropic` 的 route id，对内映射真实上游（discovery 友好，但「看起来像 Claude」易混淆，且别名池有限）。

推荐默认 **A1 + modelPicker**；A2 作「需要 discovery」时的可选项。

#### 代理路由逻辑

1. 在 `select_providers_for_request` 中，非 Pi 且 `listen_app=="claude"` 且功能开启时：  
   - 查路由表 `request_model`（先 strip `[1m]`）  
   - 命中 → 返回 **仅含该 provider** 的 `Vec`（可再套该 provider 是否允许 failover——默认否）  
   - 未命中 → 现有 `select_providers("claude")`  
2. 转发前：将 body.`model` 改为表中 `upstreamModel`，再跑目标供应商的 `apply_model_mapping`（建议：**目录命中时跳过角色折叠**，或映射仅在 upstream 仍是别名时生效——开放问题）。  
3. 格式转换：沿用目标 `meta.apiFormat`。  
4. **关键**：命中目录路由时，对齐 Pi / #6601 警告——`FailoverSwitchManager::try_switch` / `hot_switch_provider` **不得**把 UI「当前供应商」改成路由目标（否则每次 `/model` 换模型都会永久改首页选中）。

#### 请求日志 / 用量

- `ctx.provider.id` = 目标供应商 → `proxy_request_logs.provider_id` 正确。  
- `request_model` = 客户端 clientModel；`outbound_model` / `model` = 上游真实 id。  
- `app_type` 仍为 `"claude"`（与 Pi 改写为 `"pi"` 不同）。

#### 配置存储（无 SCHEMA 变更）

推荐组合：

```json
// settings key: claude_model_routing
{
  "enabled": true,
  "replaceBuiltInOptions": false,
  "writeAvailableModels": true,
  "enableGatewayDiscovery": false,
  "entries": [
    {
      "clientModel": "prov_deepseek/deepseek-chat",
      "providerId": "prov_deepseek",
      "upstreamModel": "deepseek-chat",
      "label": "DeepSeek Chat",
      "description": "via DeepSeek"
    }
  ]
}
```

可选冗余（便于按供应商编辑）：`meta.claudeCodeModelExpose: [{ upstreamModel, label, clientModel? }]`，同步时由服务层合并进 `entries`（冲突策略：providerId+upstream 唯一）。

`ProviderMeta` **可新增 serde 字段**（旧库缺字段 = Default），**不算** DB migration。

#### UI（CC Switch，可见）

- `src/components/proxy/` 新面板或嵌入 `ProxyPanel`：开关、目录表格（供应商下拉、上游 model、clientModel 只读预览、label）、保存后提示「需重启 Claude Code 会话以重载 settings env / modelPicker」。  
- 供应商表单 Claude 页：复选「暴露到 Claude `/model`」。  
- i18n 文案中英齐全。

#### WSL `settings.json` 写入

- 仅在 **Claude 代理接管激活** 时投影；关闭接管时移除本功能写入的 `modelPicker` / `availableModels` / discovery env（**勿误删用户自有字段**——需 merge/标记策略，见风险）。  
- 继续不写 Windows `%USERPROFILE%\.claude`；会话路径保持 WSL home。

#### 与 failover / 当前供应商关系

| 请求 model | 行为 |
| --- | --- |
| 命中目录 | 固定目标供应商；不热切换当前；不走全局 failover 队列（默认） |
| 未命中（含内置别名） | **完全保持今日行为**：当前供应商 + 可选 failover 队列 + `ModelMapping` |
| 功能总开关关 | 零行为变化 |

#### 向后兼容

- 默认 `enabled: false`。  
- 不改变 Pi / Codex / Desktop 路径。  
- 不改变 SCHEMA_VERSION。

---

### 设计 B：纯前缀约定、弱目录

- 约定：凡 `model` 匹配 `^([^/]+)/(.+)$` 且 capture1 是已有 `claude` provider id → 路由到该供应商，upstream=`capture2`。  
- UX：主要靠用户手打 `/model id/model` 或自己维护 `modelPicker`；CC Switch 可提供「一键生成 modelPicker」但仍以约定为核心。  
- 优点：实现面更小。  
- 缺点：无显式目录时易误路由、难做 label/用量可读性、与 upstream #3703 UI 期望不一致；前缀与真实上游名冲突时含糊。

**可作为设计 A 的解析策略子集**（clientModel 采用 A1 时），不宜单独作为产品形态。

---

## 5. 推荐设计与理由

**推荐设计 A（目录表 + modelPicker 投影 + 命中则禁热切换），clientModel 默认 A1 前缀式。**

理由：

1. 与官方 Claude Code 能力对齐：`modelPicker` 专为「网关自定义多模型」设计（v2.1.242+）；discovery 对非 Claude id 不友好。  
2. 与仓库内 Pi / Desktop 模式同构：显式目录、代理改写、共享 listen、日志按真实 provider 归因。  
3. 呼应 upstream 需求 [#3703](https://github.com/farion1231/cc-switch/issues/3703)、[#5109](https://github.com/farion1231/cc-switch/issues/5109)，且可复用 #6601 已指出的「分流不得热切换当前供应商」坑。  
4. 关闭开关即完全回到 v3.20.10 行为，风险可控。  
5. 存储只碰 `settings` KV + `meta` JSON，满足 SCHEMA 18。

---

## 6. 测试计划（可在 Linux fixture 跑，不碰用户机）

### Rust（`cargo test`，tempdir / in-memory DB）

| 用例 | 要点 |
| --- | --- |
| `resolve_claude_model_route` | 精确命中、`[1m]` strip、未命中回落、开关关闭 |
| `select_providers_for_request` | 命中目录只返回目标；未命中走 failover 队列顺序（复用现有 router 测试风格） |
| 热切换抑制 | 目录路由成功后 `FailoverSwitchManager` / current provider **不变** |
| `apply_model_mapping` 交互 | 目录 upstream 为真实 id 时不被默认 `ANTHROPIC_MODEL` 覆盖 |
| 格式转换 | 目标 `openai_chat` / `openai_responses` / `anthropic` 各一条 fixture body |
| takeover 投影 | `build_claude_takeover_*` 写入/清除 `modelPicker`；保留无关用户键（fixture JSON） |
| `/v1/models`（若做） | Claude Accept/User-Agent 或独立路径返回 `data[]`；Codex 仍 `models[]` |
| WSL 路径 | 现有 UNC 单测保持；投影逻辑用普通 temp path 即可 |

### 前端（vitest）

- 目录表格：增删改、clientModel 预览、与 provider 列表联动。  
- 开关默认 off；保存 payload 形状。

### 手工（用户机，批准实现后）

- WSL Claude Code：`/model` 列表、切换、请求日志 `provider_id`、用量、failover 开/关交叉、关闭接管后 settings 恢复。

---

## 7. 风险与需用户拍板的开放问题

1. **最低 Claude Code 版本**：`modelPicker` 需 **≥ 2.1.242**。更老版本只能靠 discovery（且非 Claude id 进不去）或单条 `ANTHROPIC_CUSTOM_MODEL_OPTION`。是否在 UI 标明版本门槛？  
2. **clientModel 命名**：A1 前缀 vs A2 Claude-safe？是否允许用户自定义 clientModel？  
3. **`replaceBuiltInOptions`**：默认 false（保留 sonnet/opus/…）还是 true（只显示目录）？  
4. **目录命中后是否允许「该供应商的 failover」**：默认否；是否要「同供应商多 Key」以后再做（#6781 相关）？  
5. **与「当前供应商」文案**：首页仍显示一个 current——目录路由时托盘/首页是否显示「本请求实际供应商」？仅日志准还是 UI 也要临时徽章？（#6601：不要永久改 current）  
6. **settings 合并**：投影 `modelPicker` 时如何对待用户手写的同名字段？建议：接管时备份到 `proxy_live_backup` / 专用 settings 键，关闭接管还原。  
7. **子代理 / 分类器请求**：`CLAUDE_CODE_SUBAGENT_MODEL`、Auto Mode 分类器是否走同一目录？（#6601 想要单独分类器队列——建议 **本期不做**，避免范围膨胀。）  
8. **同名 upstream 多供应商**：仅靠 A1 前缀可区分；若选 A2 必须禁止碰撞。  
9. **gateway discovery**：本期是否实现 Anthropic `/v1/models`？建议 **二期**；一期只写 modelPicker。  
10. **官方仓库状态**：[#3703](https://github.com/farion1231/cc-switch/issues/3703)、[#5109](https://github.com/farion1231/cc-switch/issues/5109) 仍 open，**无现成合并实现**可直接搬；本 fork 需自研并对齐其风格。

**未核实**：用户本机 Claude Code 精确版本号；Claude Code 对 `settings.json` 中 `modelPicker` 热更新是否无需重启（文档称部分 env 可热更新，但 modelPicker 读取时机未在本调研中做运行时验证）。

---

## 8. 工作量粗估（files touched）

| 区域 | 估计文件数 | 示例 |
| --- | --- | --- |
| 路由核心 | 4–6 | `provider_router.rs`, `handler_context.rs`, `forwarder.rs`, 新 `claude_model_routing.rs` |
| 接管投影 | 2–3 | `services/proxy.rs`, `services/provider/live.rs` |
| 存储 / commands | 3–4 | `database/dao/settings.rs`, `commands/proxy.rs` 或 `settings.rs`, `provider.rs`（meta 字段） |
| UI | 4–8 | `ProxyPanel` / 新 panel、types、i18n、供应商表单勾选 |
| 测试 | 3–5 | router/routing unit、takeover fixture、vitest |
| 文档 | 1 | 本方案 / 用户说明 |

**合计约 15–25 个文件**；核心后端约中等 PR，UI 中等。不含二期 discovery handler 分流。

---

## 9. 建议落地顺序（批准后）

1. 数据模型 + `resolve` 纯函数 + 单测  
2. 接入 `select_providers_for_request` + 禁热切换  
3. takeover 投影 `modelPicker`  
4. UI 目录编辑  
5. 端到端手工（WSL）  
6.（可选）Anthropic `/v1/models` + discovery  

---

## 10. Upstream 相关议题摘要

| Issue | 摘要 | 状态（调研时） |
| --- | --- | --- |
| [#3703](https://github.com/farion1231/cc-switch/issues/3703) | 希望路由按模型映射到不同供应商组合 | open，+7 |
| [#5109](https://github.com/farion1231/cc-switch/issues/5109) | 内置 New API 式多模型聚合网关 | open，+3；范围更大（LB/Key 池） |
| [#6601](https://github.com/farion1231/cc-switch/issues/6601) | 分类器请求跨供应商；强调勿热切换 current | open；实现教训可直接复用 |

本方案覆盖 #3703 的核心（按模型选供应商），**不**一次性做成 #5109 全量网关。

---

**下一步**：请用户确认第 7 节开放问题（尤其 2、3、5、9）后，再开工编码。

---

## 11. 已拍板（实现时）

| 项 | 决定 |
| --- | --- |
| clientModel | A1：`{providerId}/{upstreamModel}` |
| replaceBuiltInOptions | 默认 false |
| 首页徽章 | 不做（一期） |
| gateway discovery / Anthropic `/v1/models` | 不做（一期） |
| 目录命中 failover | 否 |
| 热切换当前供应商 | 否（#6601） |

