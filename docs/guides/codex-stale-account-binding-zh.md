# Codex 托管账号绑定悬空：选择账号恢复（SCHEMA 18）

适用版本：本 fork **v3.20.11+**（Wave B 官方 #7395 / `CodexLiveAuthSwitchGuard` 已落地）。`SCHEMA_VERSION` 仍为 **18**，没有库迁移。账号默认 / Cursor 池模型不变。

本文说明：**删除认证中心里的 ChatGPT 账号再登录后**，供应商卡片上的 `authBinding.accountId` 可能仍指向旧的本地 ID。这不是「账号丢了」，而是绑定悬空。恢复入口是卡片上已有的 **选择账号**，不是改数据库、也不是升 SCHEMA。

## 会看到什么

- 卡片提示 **绑定的账号不可用**
- 主操作旁出现 **选择账号**
- 切路由 / 开接管 / 启动恢复可能报错，文案含 **选择账号**（而不是永久的「账号不存在」死锁）

官方卡、当前卡、非当前卡都可能出现。Claude / Gemini / Pi 接管不受这条路径影响。

## 怎么恢复

1. 完全退出 CC Switch（含托盘）后再打开，避免内存里的旧绑定被写回。
2. 在 **OAuth 认证中心** 确认新登录已经出现（本地 `account_id` 会变；`chatgpt_account_id` / 邮箱通常不变）。
3. 打开出问题的 Codex 官方卡 → **选择账号** → 绑到当前那条账号 → 保存。
4. 也可以先切到一张**未绑定**或第三方卡（切走失效当前卡），再回来绑定。
5. 需要本地路由时再开 Codex 接管；启动恢复与手动开接管走同一入口。

不要在应用还在运行时手工改 `providers.meta`。必须改时：退出进程，备份 `~/.cc-switch/cc-switch.db`，把 `authBinding.accountId` 改成 `codex_oauth_auth.json` 里**当前**的 `account_id`。

## 后端两态（实现者）

`CodexLiveAuthSwitchGuard` 把「账号已经从仓库删掉」和「账号还在、只是 live token 对不上」分开：

| 状态 | 含义 | 切走时清理 |
| --- | --- | --- |
| `MissingAccount` | 内存和通过校验的磁盘仓库里都没有这个本地 ID | **只**丢掉该账号的 ownership marker，**不**删 `~/.codex/auth.json`（避免误删后来的原生登录） |
| `ExistingAccount(None)` | 账号还在，但没有匹配的托管 live refresh | 按既有目标写入策略清理；对不上的原生 / 第三方 `auth.json` 留下 |
| `ExistingAccount(Some(refresh))` | 账号还在，且磁盘 refresh 已采纳 | 比较后再删匹配的托管 live；切换窗口里 Codex CLI 又轮换了就取消本次操作 |

损坏或版本不对的 `codex_oauth_auth.json` **不能**当成「账号已删除」。目标卡绑定悬空时，**即便接管已经开着**也会报「选择账号」，不会当作成功。

## 云端覆盖了什么 / 没覆盖什么

覆盖（Linux 夹具 + lib 单测，不写用户 `C:`，SCHEMA 18）：

- `MissingAccount` / `ExistingAccount(None|Some)` 的 prepare / `ensure_unchanged` / `clear_outgoing`
- 仓库损坏拒绝恢复
- workspace 与磁盘凭据不一致 fail-closed
- 失效目标在「接管已开启」时仍要求选择账号
- 失效当前卡切到第三方，只丢 marker、保留后来的原生登录
- 卡片：当前卡与非当前卡都能点 **选择账号**

未覆盖（不要当成已在用户电脑上点过）：

- 真机 Windows Device Code 登录 / 杀进程再开
- 认证中心「删除账号时顺带解绑所有供应商」（生产者侧，#7395 明确留给后续 PR）
- SCHEMA 19 / MiniMax Code
- 改 Cursor 池模型

## 相关

- 官方修复：`farion1231/cc-switch#7395`（fixes #7392）
- 本 fork 落地：PR #41（Wave B）
- 用户手册：[2.2 切换供应商](../user-manual/zh/2-providers/2.2-switch.md)、[FAQ](../user-manual/zh/5-faq/5.2-questions.md)
