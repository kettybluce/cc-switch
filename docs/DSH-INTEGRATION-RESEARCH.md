# DSH 接入调研结论

> 为 CC Switch 接入 DeepSeek Harness 做的前期调研。所有结论均在**隔离沙箱**中实测得出，未触碰任何真实配置。

## ⚠️ 首先：一条不可违背的规则

**验证 DSH 配置时，必须使用隔离的 `DSH_HOME`，绝不可写入用户真实的 `~/.dsh`。**

```bash
export DSH_HOME=/tmp/dsh-sandbox
mkdir -p $DSH_HOME
dsh --profile headless --dump-config
```

**为什么**：`$DSH_HOME/cordis.patch.yml` 是**最高优先级**的配置层，写入即覆盖。曾因直接写用户 Windows 侧的真实
`~/.dsh/cordis.patch.yml`，用测试桩覆盖了用户正在使用的供应商配置，导致用户的 DSH 无法工作。

**当前环境**：Windows 侧的 DSH 属用户生产环境，**划为禁区**；WSL 侧的 `~/.dsh` 为干净试验场。

---

## 一、安装

| 项 | 值 |
|---|---|
| 包名 | `@deepseek-ai/dsh` |
| WSL 版本 | `0.2.0-rc.2` |
| Windows 版本 | `0.1.0-rc.6`（**勿动**） |
| WSL 安装路径 | `/home/<user>/.npm-global/lib/node_modules/@deepseek-ai/dsh` |
| WSL 命令 | `/home/<user>/.npm-global/bin/dsh` → `../lib/node_modules/@deepseek-ai/dsh/lib/bin.js` |
| 安装耗时 | 约 39 分钟（540 包） |

两个版本的配置格式一致，但版本不同，接入时需注意兼容。

---

## 二、配置体系

### DSH_HOME

默认 `~/.dsh`，可用环境变量 `DSH_HOME` 覆盖。

- 来源：`@deepseek-ai/dsh-home-paths`
- 常量：`DSH_HOME_DIR_NAME = ".dsh"`、`DSH_HOME_ENV = "DSH_HOME"`

### 配置树叠加顺序

后者覆盖前者：

```
空根
 └ dsh.profile.bundles 中各组合包的 patch
   └ profile 自身的 cordis.patch.yml       （$DSH_HOME/profiles/<name>/cordis.patch.yml）
     └ home 级 patch                        （$DSH_HOME/cordis.patch.yml）  ← 最高优先级
       └ --patch 命令行覆盖层
```

**接入点**：CC Switch 应写 **home 级 patch**，因为：
- 优先级最高，能覆盖 profile 的设定；
- 不必修改 profile 目录，风险最小。

### profile 类型

| 命令 | 用途 |
|---|---|
| `dsh --profile <name>` | 启动 `$DSH_HOME/profiles/<name>` |
| `dsh --profile headless "job"` | 运行一次性持久会话，打印答案后退出 |
| `dsh web` | `--profile web` 的别名 |
| `dsh --dump-config` | 导出合并后的配置（**仅查看，不启动**） |

---

## 三、模型配置结构

### 关键分节

```yaml
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      <providerKey>:
        displayName: 显示名
        apiKeyEnv: 环境变量名        # 密钥只存变量名，不存明文
        api: openai-completions      # 或 anthropic-messages / openai-responses
        baseURL: http://host:port/v1
        models:
          - id: 模型 ID
            name: 显示名
            contextWindow: 1000000
            maxTokens: 32000
            input: [text, image]

- id: agent-default-model
  name: "@deepseek-ai/dsh-agent-default-model"
  config:
    provider: <providerKey>          # 当前选中的 provider
    model: <modelId>                 # 当前选中的模型
```

### 密钥处理

DSH 用 **`apiKeyEnv`** 引用环境变量名，**不在配置文件中存明文密钥**。

这与 Pi 的 `models.json` 不同（Pi 直接存 `apiKey` 字段）。接入时对 key 有两种处理：

- **方案 A**：CC Switch 只写 `baseURL` / `api` / `models`，密钥由用户在 WSL 环境变量中维护；
- **方案 B**：CC Switch 同时维护环境变量（需改 shell 配置文件），实现换卡片即换 key。

**待定**，需用户选择。

---

## 四、⚠️ 核心陷阱：providers 是整体替换

### 实测结论

在沙箱中分两层写 provider：

- profile 层写 `user-real`
- home 层写 `ccs-new`

合并结果：**只剩 `ccs-new`**，`user-real` 被完全覆盖。

### 含义

`providers` 遵循**整体替换**语义，**不是深合并**。若 CC Switch 只写自己管理的 provider，
会把用户原有的 provider **全部弄丢** —— 这正是前述事故的机制。

### 正确做法（已实测可行）

```
1. 读全量：dsh --dump-config 或直接解析各层，拿到当前所有 providers
2. 内存合并：在读到的基础上增删改 CC Switch 管理的那一项
3. 整体写回：把完整 providers 写进 $DSH_HOME/cordis.patch.yml
```

实测验证：把 `user-real` 与 `ccs-new` 一起写回 home 层，两者共存，未丢失。

---

## 五、对 CC Switch 实现的要求

| 要求 | 说明 |
|---|---|
| 必须读-改-写全量 | 严禁只写差分 |
| 写入前备份 | 写 `cordis.patch.yml` 前先备份原文件 |
| 保留未知字段 | 用户可能有 CC Switch 不认识的配置项，写回时不能丢 |
| 原子写入 | 避免写一半崩溃导致配置损坏 |
| 区分 WSL / Windows | 通过 `DSH_HOME` 或 UNC 路径决定操作目标 |
| 不碰生产环境 | 测试一律用临时 `DSH_HOME` |

---

## 六、待解决

1. **密钥策略**：方案 A（只管地址与模型）还是方案 B（连环境变量一起管）。
2. **WSL 侧首次初始化**：`~/.dsh` 尚不存在，CC Switch 是否需要负责创建 profile。
3. **用量与会话**：`~/.dsh/sessions/session.jsonl.zstd` 为 zstd 压缩，格式待分析。
4. **更新日志**：DSH 走 nightly 通道（`download.deepseek.com/dsh-desk/feeds/win-x64/`），
   与 CC Switch 现有的工具版本检测机制不同，需单独设计。
