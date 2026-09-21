# v3.20.14 出包烧机清单（SCHEMA 18）

本页是 **v3.20.14 发 Windows x64 绿色版（Portable，优先）+ MSI** 的烧机清单。  
版本 bump 在 `chore(release): v3.20.14` PR。`SCHEMA_VERSION` 保持 **18**。账号默认 / **Cursor 池模型**不变。不是官方 3.20.3 / 3.20.14。

用户后续指示：#53/#60 已在 main 即可出包；#56 若仍未合入则**本版不含**并在发版说明里写明。#49/#54 Docker 默认全量仍未合入，本云端无 Docker daemon，全量报告记为缺口而非硬拦截。

## 门槛：已在 main vs 本版不含

#51 往返与 #53/#60 fail-closed 已在 main。#53 是 fail-closed 增量（#52 的独有部分）；**不要把旧的 #52 再 squash 一遍。**

| PR | 内容 | 写本页时的状态 | 合入 SHA |
| --- | --- | --- | --- |
| [#51](https://github.com/kettybluce/cc-switch/pull/51) | Linux 夹具：Claude/Codex 接管往返 | **已合入** | `bd644eaf` |
| [#53](https://github.com/kettybluce/cc-switch/pull/53) | Claude/Codex 接管 fail-closed | **已合入** | `b762b3aa` |
| [#60](https://github.com/kettybluce/cc-switch/pull/60) | Pi 接管 fail-closed | **已合入** | `73bf520f` |
| [#61](https://github.com/kettybluce/cc-switch/pull/61) | 日志/托盘/投影密钥脱敏 | **已合入** | `3addd46d` |
| [#62](https://github.com/kettybluce/cc-switch/pull/62) | 四语键 + Pi live preset lock | **已合入** | `67475045` |
| [#63](https://github.com/kettybluce/cc-switch/pull/63) | SCHEMA 18 护栏 | **已合入** | `329589a2` |
| [#55](https://github.com/kettybluce/cc-switch/pull/55) | Portable dry-run + release.yml | **已合入** | `a162343a` |
| [#57](https://github.com/kettybluce/cc-switch/pull/57) | 前端 Pi 表单单测 | **已合入** | `8c1f8a64` |
| [#50](https://github.com/kettybluce/cc-switch/pull/50) | Docker 全量出包文档 | **已合入** | `1a605875` |
| [#52](https://github.com/kettybluce/cc-switch/pull/52) | 旧基底往返 + fail-closed | **未合入** — 勿 squash | — |
| [#56](https://github.com/kettybluce/cc-switch/pull/56) | 代理热路径硬化 | **本版不含**（未合入） | — |
| [#54](https://github.com/kettybluce/cc-switch/pull/54) / [#49](https://github.com/kettybluce/cc-switch/pull/49) | Docker 默认全量报告 | **未合入** | — |

核对命令：

```bash
git fetch origin main
git merge-base --is-ancestor bd644eaf origin/main && echo '#51 on main'
git merge-base --is-ancestor b762b3aa origin/main && echo '#53 on main'
git merge-base --is-ancestor 73bf520f origin/main && echo '#60 on main'
git merge-base --is-ancestor 3addd46d origin/main && echo '#61 on main'
git merge-base --is-ancestor a162343a origin/main && echo '#55 on main'
```

#52 请关或变基，不要再 squash 重叠往返。#56 未合入则发版说明必须写明本版不含代理热路径硬化。#49/#54 全量 Docker 报告本云端无 daemon，记缺口。

已 Salvage、不必再修：`release.yml` Apple 检测缺 `fi`、macOS matrix `continue-on-error`、`phf` / `phf_macros` path patch。#55 已加固 Windows Portable+MSI 出包路径与 CI 语法检查。

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

`dry_run` 仍会上传 `release-assets-Windows-x64` / `release-assets-macOS` artifact，**不会** `softprops/action-gh-release`，也**不会**改 `latest.json`。v3.20.14 的 version bump + tag 由发版 PR 完成。

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

## 打 tag / 出包步骤（#51/#53/#60/#61/#55 已在 main）

1. **Bump 到 3.20.14**：`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`Cargo.lock`。
2. 写 `docs/release-notes/v3.20.14.md`，必须有 `## 修复的问题` / `## 优化` / `## 自测`。写明：#51 往返、#53/#60 fail-closed、#61 脱敏、#55 发版加固、**#56 未合入**、SCHEMA 18、Portable 优先、Cursor 池模型不变。
3. CHANGELOG `[3.20.14]`；把 `[Unreleased]` 里已合入项挪过去。
4. workflow-lint + Portable dry-run 绿。
5. 无 Docker daemon 时在发版说明记录 `pnpm test:docker:all` 缺口。
6. 在 bump 提交上 `git tag v3.20.14` 并 push，触发 Release workflow。
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
5. Docker 全量自测在有 daemon 的机器上补报告（#54/#49 尚未合入）。
6. 账号默认 / Cursor 池模型与 v3.20.13 相同。
7. 未签名 macOS：右键打开或 `xattr -cr "CC Switch.app"`。
8. 本版无 #56：代理 panic / 截断 SSE / 4xx 对齐若仍复现，等后续版本。

任一步 FAIL：把日志贴到 issue。SCHEMA 没升，回退 v3.20.13 / 官方 3.20.2 是安全的。

## 刻意不做

- 不升 SCHEMA 19 / MiniMax Code / `migrate_v18_to_v19`
- 不把官方版本号或整段官方 `main` 合进来
- 不为 Error 5 改 WiX
- 不把未合入的 #56 说成已出货
- 不在「仅 workflow 加固」PR（#55）里 bump 3.20.14
