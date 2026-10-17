# 本机测试清单（Windows + WSL 场景）

> 依据本机实际配置生成。场景变化时同步更新。

## 一、实际使用场景

**形态**：Windows 上运行 CC Switch（GUI），agent 全部在 **WSL2 Ubuntu-22.04** 里工作，CC Switch 通过 `\\wsl.localhost` UNC 路径接管 WSL 侧配置。

### 配置指向（CC Switch 设置中的实际值）

| 应用 | CC Switch 读取/写入的目录 |
|---|---|
| claude | `\\wsl.localhost\Ubuntu-22.04\home\<user>\.claude` |
| codex | `\\wsl.localhost\Ubuntu-22.04\home\<user>\.codex` |
| pi | `\\wsl.localhost\Ubuntu-22.04\home\<user>\.pi\agent` |

### WSL 侧数据规模

| 目录 | 文件数 | 说明 |
|---|---|---|
| `~/.codex` | 5603 | 会话与状态库 |
| `~/.hermes` | 54367 | 数据量大 |
| `~/.claude` | 612 | 会话与配置 |
| `~/.pi` | 73 | 配置与模型表 |

### CLI 安装位置

### CLI 实际解析（重要，与直觉不同）

| 命令 | `command -v` 解析结果 | 实际是什么 | 说明 |
|---|---|---|---|
| `claude` | `/mnt/c/Users/.../npm/claude` | **Windows 版 claude.exe** | WSL 里装的是 Windows 构建，未被链接 |
| `codex` | `/home/<user>/.npm-global/bin/codex` | WSL 原生 | 正常 |
| `pi` | `/home/<user>/.npm-global/bin/pi` | WSL 原生 | 正常 |

**claude 的具体情况**：`/home/<user>/.npm-global/lib/node_modules/@anthropic-ai/claude-code`
版本 `2.1.296`，其 `package.json` 的 `bin` 指向 `bin/claude.exe`，
该文件为 Windows 可执行且**无 Linux 执行位**，因此 npm 未创建
`.npm-global/bin/claude` 链接。PATH 中 `/mnt/c/.../npm` 排在前面，
`claude` 遂回落到 Windows 版。

**由此产生的影响**（必须测）：

- WSL 的 `~/.claude`（612 文件）**不是** Windows 版 claude.exe 读取的目录；
- Windows 的 `C:\Users\<user>\.claude`（729 文件）才是当前实际生效的配置；
- 而 CC Switch 的 `claudeConfigDir` 指向 **WSL 侧**，
  可能出现「切换了卡片但实际使用没变」的现象。

### 其他环境

- WSL 网络模式：`mirrored`，`autoProxy=false`
- 本地代理端口：`15721`
- 系统代理：`7897`
- 数据库 schema：v20
- 卡片数：codex 27 / claude 21 / claude-desktop 2 / grokbuild 2 / gemini 1 / hermes 1 / pi 1
- 当前模式：仅 `claude-desktop` 为 proxy（attached），其余 direct

### 由此决定

测试重心是 **WSL 路径下的读写**，而非 Windows 本地配置。凡是涉及"配置是否写到了 agent 真正读取的位置"的事项，权重最高。

---

## 二、测试清单

标记：`☐` 待测、`✅` 通过、`❌` 失败、`⏭` 跳过、`⚠️` 通过但有隐患。

### A. 基础可用性（每版必测）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| A1 | 启动应用 | 进程起来，托盘出现，主窗口按设置隐藏 | ☐ |
| A2 | 从托盘唤出主窗口 | UI 完整渲染，非空白页 | ☐ |
| A3 | 数据库读取 | 无"数据库版本过新"阻断页 | ☐ |
| A4 | 卡片列表加载 | codex 27 / claude 21 / pi 1 等数量正确 | ☐ |
| A5 | 本地代理端口 | 启用后 `127.0.0.1:15721` 监听 | ☐ |
| A6 | 关闭应用 | 干净退出，无残留 webview 进程 | ☐ |

### B. WSL 路径读写（**核心**）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| B1 | 读取 WSL 侧 claude 配置 | 能读到 `\\wsl.localhost\...\.claude\settings.json` 内容 | ☐ |
| B2 | 读取 WSL 侧 codex 配置 | 能读到 `config.toml` / `auth.json` | ☐ |
| B3 | 读取 WSL 侧 pi 配置 | 能读到 `models.json` | ☐ |
| B4 | 切换卡片写入 WSL | 切换 claude 卡片后，WSL 内 `~/.claude/settings.json` 真的变了 | ☐ |
| B5 | WSL 内验证生效 | 在 WSL 里跑 `claude -p 'reply OK'` 能正常返回 | ☐ |
| B6 | UNC 路径大文件读取 | 读 `~/.codex`（5603 文件）不卡死、不超时 | ☐ |
| B7 | 路径含中文/空格 | 配置目录路径含中文时不报错 | ☐ |
| B8 | **claude 写入位置验证（新增）** | 切换 claude 卡片后，确认写入的是 `claudeConfigDir` 指向的 WSL 目录，而非 Windows 的 `~/.claude` | ☐ |
| B9 | **claude 实际生效验证（新增）** | 在 WSL 里执行 `claude -p 'reply OK'`，确认它读取的是被 CC Switch 改写的那份配置（若读到 Windows 侧配置则说明存在错位） | ☐ |

### C. 卡片管理

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| C1 | 切换 claude 卡片 | 生效，WSL 侧配置更新 | ☐ |
| C2 | 切换 codex 卡片 | 生效，`config.toml` + `auth.json` 正确 | ☐ |
| C3 | 编辑卡片保存 | 入库，列表刷新 | ☐ |
| C4 | 复制卡片 | 生成副本不污染原卡片 | ☐ |
| C5 | 删除卡片 | 确认弹窗，删除后列表移除 | ☐ |
| C6 | 拖拽排序 | 持久化，重启保持 | ☐ |
| C7 | 搜索过滤 | 按名称/模型过滤正确 | ☐ |

### D. Pi 定制功能（重点）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| D1 | Pi 卡片正常显示 | 出现 1 张，不报错 | ☐ |
| D2 | Pi 直连模式 | WSL `models.json` 保持真实上游 baseUrl | ☐ |
| D3 | Pi 切路由模式 | baseUrl 变 `127.0.0.1:15721`，apiKey 变 `PROXY_MANAGED` | ☐ |
| D4 | 路由下发请求 200 | `POST /v1/responses`（带 `x-cc-switch-app: pi`）非 503 | ☐ |
| D5 | 切回直连还原 | 按数据库卡片还原真实 baseUrl + key | ☐ |
| D6 | **投影守卫** | 重启后数据库卡片仍是真实值，未被占位符覆盖 | ☐ |
| D7 | Pi「从配置移除」入口 | 入口存在可点击，弹窗正常 | ☐ |
| D8 | Pi 用量归因 | 计入 `pi` 而非 `codex` | ☐ |
| D9 | Pi 会话列表 | 能列出 WSL 内 `~/.pi` 会话 | ☐ |
| D10 | WSL 内调用 Pi | WSL 里 `pi` 经代理正常响应 | ☐ |

### E. WSL CLI 执行与代理（定制项 C6）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| E1 | CC Switch 调 WSL CLI | 检测版本/安装状态不超时 | ☐ |
| E2 | 残留代理清除 | `autoProxy=false` 生效，WSL 内无 `127.0.0.1:10808` 残留 | ☐ |
| E3 | 代理可达时不清 | 手动设可用 `http_proxy`，调用 CLI 后仍保留 | ☐ |
| E4 | 代理不可达时清 | 设指向关闭端口的 `http_proxy`，调用后自动清除 | ☐ |
| E5 | WSL 发行版停止时 | 发行版未启动时给出可读错误，不崩溃 | ☐ |

### F. 用量与会话

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| F1 | 用量页加载 | 读到历史用量 | ☐ |
| F2 | 会话列表 | claude / codex / pi 会话均能列出 | ☐ |
| F3 | 大目录扫描性能 | `~/.codex` 5603 文件扫描不卡 UI | ☐ |
| F4 | 用量同步时效 | 新请求 ≤20s 入库 | ☐ |
| F5 | 成本计算 | 有定价的模型显示非零成本 | ☐ |

### G. 回归项（v4.0.14 合并新增）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| G1 | Claude Desktop 模式迁移 | 日志出现迁移提示 | ☐ |
| G2 | 用量面板币种切换 | 可切换并持久化 | ☐ |
| G3 | 请求日志新增列 | Cache Write / Hit Rate / Total Time / Status Code 可选 | ☐ |
| G4 | 设置页不崩溃 | 进出正常 | ☐ |

### H. 构建与打包

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| H1 | `pnpm typecheck` | 通过 | ☐ |
| H2 | `pnpm test:unit` | 227 文件 / 2968 用例通过 | ☐ |
| H3 | `cargo check --all-targets` | 无 error | ☐ |
| H4 | `cargo build --release` | 版本 4.15.0 | ☐ |
| H5 | NSIS 打包 | 产出 `CC Switch_4.15.0_x64-setup.exe` | ☐ |

---

## 三、执行顺序

1. **H1–H3**：代码基座回归
2. **A 组**：基础可用性
3. **B 组**：WSL 路径读写（不通则后续无意义）
4. **D 组**：Pi 定制
5. **E 组**：WSL CLI 与代理
6. **C / F / G 组**：卡片、用量、回归
7. **H4–H5**：打包

## 四、风险提示

- **所有改动落在 WSL 的真实配置上**：`~/.claude`、`~/.codex`、`~/.pi`。动手前务必备份。
- **D3/D5 会改动 WSL 的 `models.json`**：这是日常在用的，切后必须验证能正确还原。
- **B4 写错会影响 WSL 内的正常使用**：测试后需确认 WSL 里 `claude` 仍可用。
- **D6 若失败**：说明投影守卫失效，Pi 路由会静默 503，属本次合并的回归。
- **E1/E5 依赖 WSL 处于运行状态**：发行版停止时相关用例应记为 ⏭ 而非 ❌。
