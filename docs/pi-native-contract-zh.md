# Pi 原生契约与实现边界

> 验证原则：开发和验收使用当时最新发布的 Pi；CC Switch 不绑定某个 Pi 版本。

这份文档记录 CC Switch 当前实际消费的 Pi 原生契约。它不是 Pi 配置格式的完整镜像，也不承诺 OAuth、故障转移或所有兼容字段。实现和测试只覆盖前端已经提供的能力。

## 验证方式

供应商字段、继承顺序和配置值解析以开发时最新 Pi 的源码入口和真实 CLI 行为为准。资源与会话行为通过 Pi 的 `DefaultResourceLoader`、`loadSkills`、`loadPromptTemplates` 和 `SessionManager` 实际运行确认。仓库只保留产品真正消费的最小契约和普通测试，不维护上游源码快照、哈希或版本证据库。

## 当前消费的契约

| 资源                            | CC Switch 行为                                                | 状态来源         |
| ------------------------------- | ------------------------------------------------------------- | ---------------- |
| `models.json`                   | 管理 `providers` 中的全部显式供应商节点；精确新增、替换和移除 | 文件中的实际条目 |
| 全局 `settings.json`            | 只读 `defaultProvider`、`defaultModel`、`sessionDir`          | Pi 原生设置      |
| `auth.json`                     | 不读、不写、不刷新                                            | Pi `/login`      |
| `AGENTS.md`                     | 提示库中与文件内容精确匹配的项视为正在使用                    | 文件存在及内容   |
| `SYSTEM.md`、`APPEND_SYSTEM.md` | 直接编辑固定原生文件；不存在即未配置                          | 文件存在         |
| `prompts/*.md`                  | 管理顶层斜杠命令模板；空模板是有效原生文件                    | 文件存在         |
| `skills/<目录>`                 | 目录存在即被 Pi 发现                                          | 原生 Skills 目录 |
| Sessions JSONL                  | 读取 Pi 的会话头、树分支、消息和会话名称                      | 原生会话文件     |

### 运行位置

Pi 可能装在 CC Switch 所在的机器上，也可能装在 WSL2 发行版里。两者是各自独立的安装，拥有各自的 `models.json` 和会话目录，因此运行位置是设备级设置，不做合并。缺省即本机运行时。

`piConfigDir` 是 Pi **主目录**（默认 `~/.pi`，与 `~/.claude` 同形）。Pi 实际读取的文件在下一层：`{piConfigDir}/agent/models.json`、`agent/settings.json`、`agent/sessions/`。`PI_CODING_AGENT_DIR` 指向的是 agent 目录本身。若顶层 `{piConfigDir}/models.json` 存在，写入时与 agent 文件保持同步，不得分叉。

`models.json` 等接管写入仍走 `wsl.exe`（本地代理可保留）。**会话列表与用量扫描**则与开源版 Claude 的配置目录一致：在 Windows 上读

`\\wsl.localhost\<发行版>\home\<linux 用户>\.pi\agent\sessions`

不再默认镜像到 `%USERPROFILE%\.cc-switch\pi-wsl-sessions`，恢复命令也不得指向 `\\?\C:\Users\…`。Linux 夹具用同一套 POSIX 路径作为等价物。

### 残留风险（诚实说明）

- **`wsl.localhost` 会唤醒发行版**：9P 访问可能慢，发行版停止时列表/用量会失败（界面应显示不可用，而不是在 C: 建空目录）。
- **VHD 仍可能在 C:**：逻辑数据在 WSL 家目录（`~/.pi`），但发行版虚拟磁盘通常仍落在 Windows 用户目录下的 `ext4.vhdx`。UNC 只避免把会话 JSONL 再复制一份到 `%USERPROFILE%\.cc-switch\`，并不能把 VHD 搬离 C:。
- **不要用 Windows 用户名 / machineId 当默认路径**：发行版名与 Linux 用户来自探测或用户填写，不得写死本机 Windows 账号。

### 供应商

结构化表单只验证并编辑常用字段：

- 供应商级 `name`、`baseUrl`、`apiKey`、`api`、`headers`
- 模型级 `id`、`name`、`api`、`reasoning`、`input`、`contextWindow`、`maxTokens`

已有配置中的其他字段原样保留。供应商是否可管理只取决于节点是否显式存在于 `models.json.providers`：`anthropic`、`openai`、`deepseek` 等 Pi 内置 ID，以及带有未知字段的节点，都按普通显式配置同步。CC Switch 不把 Pi 运行时合并出的内置模型复制回配置，也不解析或执行 `apiKey`、Header 中的环境变量和命令表达式。请求仍由 Pi 自己发出。

Pi 在全局设置中保存的当前供应商和模型不进入供应商列表状态。启用供应商只把条目加入 `models.json`，不会写 `defaultProvider` 或 `defaultModel`。移除或删除全局默认供应商时，前端在原有确认框内给出非阻塞提醒，后端允许继续且不改写默认项。编辑显式供应商同样不修改当前供应商或模型；失效引用和回退由 Pi 原生处理。

项目级 `.pi/settings.json` 会按启动 Pi 时的工作目录覆盖全局默认项。供应商页没有项目上下文，因此不扫描项目目录或猜测活动会话；条件提醒只读取全局默认供应商。项目级默认项继续由对应 Pi 会话和 `/model` 管理。

用量查询脚本属于 CC Switch 元数据，只更新数据库，不重写 `models.json`。

### 代理

Pi 与 Claude / Codex 走同一条产品路径：**接管 + 本地代理**。在「设置 → 代理」中打开 Pi 接管后（会按需启动本地代理），已管理供应商的请求经本地代理转发：

```
Pi → CC Switch 本地代理 → 供应商 API
```

做法是改写运行中的 `~/.pi/agent/models.json` 里每个已管理节点的 `baseUrl`（以及模型级 `baseUrl`），指向 `http://<可达主机>:<本地代理端口>/pi/<provider_id>`（OpenAI / Azure Responses 风格再加 `/v1`，Google 若上游带 `/v1beta` 或 `/v1` 则保留该后缀）。真实上游地址只保存在 CC Switch 数据库；关闭 Pi 接管或停止本地代理时，把 `models.json` 恢复成数据库中的上游 URL。

这就是 Pi 的接管实现（additive 应用不能整文件备份 `models.json`）。以前的 Option B「本地代理一开就自动改写 baseUrl、没有 Pi 接管开关」已降级：仅启动本地代理、不打开 Pi 接管时，不再改写 Pi。`flags.wsl_proxy` 仍可作为高级退出。

`api` 可写在供应商级，也可只写在模型级（Pi 0.85 起允许）。字段决定走哪条已有路由：`anthropic-messages` → `/v1/messages`，`openai-completions` → `/v1/chat/completions`，`openai-responses` 与 `azure-openai-responses` → `/v1/responses`，`google-generative-ai` → `/v1beta/*` 或 `/v1/*`。同一供应商下模型 `api` 不一致时，只给协议会撞路径后缀的模型写入各自的投影 `baseUrl`。`bedrock-converse-stream`、`pi-messages`、`google-vertex`、`openai-codex-responses`、`mistral-conversations` 等本地代理没有对应处理器的协议不会改写，Pi 仍直连上游。

这不是进程级 `HTTP_PROXY` / `HTTPS_PROXY` 注入，也不是发行版全局代理，也不写入 Pi 全局 `settings.json` 的 `httpProxy`。CC Switch 不修改 `/etc/environment`、`/etc/profile`、`~/.bashrc`。密钥仍通过 `models.json` 的既有字段写入，从不出现在 `wsl.exe` 命令行或日志里。

回环地址在 WSL 边界两侧含义不同：镜像网络模式下 `127.0.0.1` 与 Windows 共享，默认 NAT 模式下则不是，此时 Windows 主机表现为发行版的默认网关。因此候选地址按优先级探测（镜像回环 → 默认网关 → `resolv.conf`），并把命中的方式与解析出的端点回报给界面。探测失败时不会把 `127.0.0.1` 写入 WSL 的 `models.json`（NAT 下那是发行版自己），真实上游地址保持不变。NAT 模式下若本地代理只监听 `127.0.0.1`，发行版无法经网关连上，需要把监听地址改成 `0.0.0.0`。「检测代理」只确认本地代理 `/health` 可连；供应商 401 是密钥问题，与代理可达性分开显示。

CC Switch 仍不为 Pi 做故障转移队列或网关状态机；请求由 Pi 自己发给本地代理，本地代理按路径中的 provider id 选用对应卡片再转发。不管理 `auth.json`。

### 并发与外部修改

供应商标识是成员身份：`models.json.providers` 中存在该标识即为已启用。CC Switch 每次进入列表及应用启动时同步全部显式节点，不按内置 ID、认证字段或模型完整度过滤。原生配置发生修改后，以原生内容更新同 ID 的已保存档案；原生删除只改变启用状态。移除前保存目标节点的最新内容，数据库中的卡片和完整配置继续保留。

`models.json`、系统提示文件和模板使用原子写入。进程内写操作串行化；同一次读改写期间的跨进程变化通过 revision 比较发现。列表刷新会先把外部修改同步到数据库；保存时只替换目标 provider 节点，启用和移除只增删目标节点，其他 provider 与顶层字段保持不变。不再维护编辑快照或额外 ownership 状态。

### Sessions

全局会话页只枚举绝对 `sessionDir`、`~` 路径或 Pi 默认目录。相对 `sessionDir` 依赖启动 Pi 时的项目工作目录，CC Switch 没有可靠上下文，因此明确显示“需要项目上下文”，不会猜测目录。

会话解析只消费 UI 所需字段。未知条目被忽略；删除前会验证文件仍在已解析的 Pi 会话根目录中，并核对会话 ID。会话文件在 `agent/sessions/--<cwd-encoded>--/` 下：cwd 组目录里的 `*.jsonl` 会话文件，以及同名会话目录的 `tasks/*.jsonl`。用量导入读取助手消息、toolResult、compaction 与 branch_summary 上的 `message.usage` 令牌；**不信任** JSONL 里的 `cost.total`（实际几乎总是 0），费用按 `models.json` 中模型的单价 × 令牌计算，缺失时再回退数据库定价。Pi 0.85 起把 `cacheWrite1h` 并入缓存写入，仅有 `reasoning`、没有 `output` 的回合也会计入完成量。

## 明确不做

- Pi `/login`、`auth.json` 中的 OAuth/API Key 登录、令牌保存和刷新
- 默认供应商或默认模型写入
- 故障转移队列和网关状态机；本地代理按 `models.json` 的 `baseUrl` 投影转发，不改写 Pi 发出的请求体协议（除已有 Claude/Codex/Gemini 转换路径外）
- 修改发行版的 `/etc/environment`、`/etc/profile`、`~/.bashrc` 等共享环境
- 以 `HTTP_PROXY` / `HTTPS_PROXY` 作为 Pi 的主路径（默认关闭，不注入进程环境）；不写入 Pi `settings.json` 的 `httpProxy`
- Pi 运行时内置供应商与内置模型目录的复制
- 完整 `compat`、`modelOverrides` 和费用编辑器；思考档位只提供 Pi 原生 `thinkingLevelMap` 的轻量入口
- 相对会话目录的全局猜测

Pi 发布新版本时，通过正常开发和验收重新运行供应商、提示词、Skills 和 Sessions 契约测试。没有进入产品界面的上游字段不应仅为了“覆盖完整”而扩展后端。
