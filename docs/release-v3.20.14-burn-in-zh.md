# v3.20.14 出包烧机清单（SCHEMA 18）

本页是 **v3.20.14 发 Windows x64 绿色版（Portable，优先）+ MSI** 之前的烧机清单。  
**现在不要打 `v3.20.14` 标签、不要 bump 版本、不要出包。** 当前已发布的是 [v3.20.13](https://github.com/kettybluce/cc-switch/releases/tag/v3.20.13)。`package.json` / `src-tauri/tauri.conf.json` / `Cargo.toml` 仍是 **3.20.13**。

`SCHEMA_VERSION` 保持 **18**。账号默认 / **Cursor 池模型**不变。不是官方 3.20.3 / 3.20.14。

## 硬门槛：夹具 PR 必须全部在 main

**只有当 main 同时含有 #49、#52、#53 之后，才允许打 `v3.20.14` 并出 Portable。** 缺任何一个就停。#51 往返夹具已在 main。#53 是 fail-closed 增量（#52 的独有部分）；不要把旧的 #52 再 squash 一遍。

| PR | 内容 | 写本页时的状态 | 合入 SHA（合入后填） |
| --- | --- | --- | --- |
| [#51](https://github.com/kettybluce/cc-switch/pull/51) | Linux 夹具：Claude/Codex 接管往返（SCHEMA 18） | **已合入** `bd644eaf` | `bd644eaf` |
| [#53](https://github.com/kettybluce/cc-switch/pull/53) | 接管 fail-closed（#52 变基后的独有增量） | **已合入** `b762b3aa` | `b762b3aa` |
| [#52](https://github.com/kettybluce/cc-switch/pull/52) | 旧基底往返 + fail-closed（draft） | **未合入** — 勿 squash；以 #53 为准 | _待关或变基_ |
| [#49](https://github.com/kettybluce/cc-switch/pull/49) | Docker 自测默认全量并写出全量报告 | **未合入**（draft） | _待填_ |

核对命令：

```bash
git fetch origin main
git merge-base --is-ancestor <sha-49> origin/main && echo '#49 on main'
git merge-base --is-ancestor bd644eaf origin/main && echo '#51 on main'
git merge-base --is-ancestor b762b3aa origin/main && echo '#53 on main'
```

#49 未在 main 之前：**禁止** `git tag v3.20.14`，禁止用新 tag 发 GitHub Release。#52 请关或变基，不要再 squash 重叠往返。

已 Salvage、不必再修：`release.yml` Apple 检测缺 `fi`、macOS matrix `continue-on-error`、`phf` / `phf_macros` path patch。本轮只加固 Windows Portable+MSI 出包路径与 CI 语法检查。

## 发版前 CI / 语法

每次改 workflow 或 `scripts/` 都应绿：

1. **`bash -n`**：`scripts/**/*.sh`，以及从 `.github/workflows/*.yml` 抽出的 bash `run:` 块（防再漏 `fi`）。
2. **actionlint v1.7.12**（checksum 钉死）：`scripts/ci/lint-shell-and-workflows.sh`。
3. **Windows Portable 打包 dry-run**（不编译 Tauri、不 bump 版本）：`scripts/ci/dry-run-windows-portable.sh`  
   用假 MSI / `cc-switch.exe` 跑 `scripts/package-windows-release.py`，断言：
   - `CC-Switch-<tag>-Windows.msi`
   - `CC-Switch-<tag>-Windows-Portable.zip`（根目录含 `cc-switch.exe` + 无 BOM 的 `portable.ini`，`portable=true`）
4. 仓库 frontend / backend CI（`pnpm typecheck` / `format:check` / `test:unit`；`cargo fmt` / `clippy` / `cargo test`）。

本地复现：

```bash
bash scripts/ci/lint-shell-and-workflows.sh
bash scripts/ci/dry-run-windows-portable.sh
```

Release workflow 现在支持 **只打包、不发 GitHub Release**：

- Actions → Release → Run workflow
- `tag`：填**已存在**的 tag（例如 `v3.20.13` 回归验证）
- `dry_run`：勾选

`dry_run` 仍会上传 `release-assets-Windows-x64` / `release-assets-macOS` artifact，**不会** `softprops/action-gh-release`，也**不会**改 `latest.json`。新版本必须先有 version bump PR，再打 tag；本清单本身不是 bump。

## Docker 全量自测（出包必须）

`1a605875` 已规定：自测工作之后出 Portable/MSI **必须** Docker **全量** + 报告。默认 `pnpm test:docker` 只是最低门槛。

```bash
pnpm test:docker:all
# 或 make test-docker-all / TEST_FILTER=all
```

报告至少要有：退出码、通过/失败/忽略计数、SCHEMA 仍为 18、未写宿主机 `C:`。#49 合入后报告路径为 `docs/self-test-reports/docker-self-test-YYYYMMDD-HHMM.md`。未合入 #49 时，把 compose / cargo 终端输出贴到发版 PR。

## Windows 打包（v3.20.12 已暴露的坑）

[v3.20.12 第一次 Release](https://github.com/kettybluce/cc-switch/actions/runs/35562823645)：Windows x64 **绿**，macOS **红**，`Publish GitHub Release` 被 **skip**。根因是 `needs.release.result == 'success'` 在 matrix 下一格失败时整段 job 仍算 failure（即使后来给 macOS 加了 `continue-on-error`）。

本轮加固（workflow only）：

| 项 | 做法 |
| --- | --- |
| 发布条件 | `always() && !cancelled() && inputs.dry_run != true`；真正闸门是 Windows Portable+MSI 断言 |
| 并发 | `cancel-in-progress: false`，避免重跑把正在打的 MSI/zip 掐掉 |
| Windows 构建 | `timeout-minutes: 60`；`pnpm tauri build --ci --bundles msi`（不跑 NSIS，减少无用失败） |
| 打包脚本 | `scripts/package-windows-release.py`（UTF-8 无 BOM `portable.ini`，zip 根目录成员） |
| 密钥 | secrets 走 `env:`，不再把 `${{ secrets.* }}` 嵌进 bash `if` |

装包仍 **Portable 优先**。MSI Error 5（`D:\Config.Msi` ACL）见 [windows-msi-error-5-zh.md](windows-msi-error-5-zh.md)。WiX 保持官方 per-user 模板，不为 Error 5 改安装范围。

## 允许打 tag 之后的步骤（三个夹具 PR 都在 main）

1. **Bump 到 3.20.14**（单独 PR，本页对应的 workflow hardening PR **不含** bump）：`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`。
2. 写 `docs/release-notes/v3.20.14.md`，必须有 `## 修复的问题` / `## 优化` / `## 自测`。写明：#51 往返夹具、#53 fail-closed、#49 Docker 全量报告、SCHEMA 18、Portable 优先、Cursor 池模型不变。
3. CHANGELOG `[3.20.14]`；把 `[Unreleased]` 里已合入项挪过去。
4. CI 全绿，workflow-lint + Portable dry-run 绿。
5. `pnpm test:docker:all` 报告贴到发版 PR。
6. squash 合入 bump PR 后：`git tag v3.20.14` 并 push（或在已合入 SHA 上打 tag）。
7. 等 Release workflow 绿。Windows 必须上传：
   - `CC-Switch-v3.20.14-Windows-Portable.zip`（推荐）
   - `CC-Switch-v3.20.14-Windows.msi`（次要）
   - 有则附 `CC-Switch-v3.20.14-Windows.msi.sig`
8. macOS `.dmg` / `.zip` / `.tar.gz` 尽力而为；缺了也要发出 Windows。
9. 打开 Release 页，确认 prerelease 与下载链接。Portable URL 形式：  
   `https://github.com/kettybluce/cc-switch/releases/download/v3.20.14/CC-Switch-v3.20.14-Windows-Portable.zip`

## 装上后请自己点（云端 ≠ 真机）

1. 解压绿色版，`cc-switch.exe` 旁有 `portable.ini`，应用识别为便携版。
2. MSI Error 5 → 改用绿色版，不要改 SCHEMA。
3. #51/#53：Claude / Codex 接管往返；缺文件 / 损坏 / 他处代理应 fail-closed；关一个不影响另一个。
4. Pi CRUD ↔ `~/.pi/agent/models.json`；删当前默认应改派。
5. Docker 全量自测在本机或云端能复现报告（#49）。
6. 账号默认 / Cursor 池模型与 v3.20.13 相同。
7. 未签名 macOS：右键打开或 `xattr -cr "CC Switch.app"`。

任一步 FAIL：把日志贴到 issue。SCHEMA 没升，回退 v3.20.13 / 官方 3.20.2 是安全的。

## 刻意不做

- 不升 SCHEMA 19 / MiniMax Code / `migrate_v18_to_v19`
- 不把官方版本号或整段官方 `main` 合进来
- 不为 Error 5 改 WiX
- 不在夹具 PR 未齐时打 `v3.20.14`
- 不在「仅 workflow 加固」PR 里 bump 3.20.14
