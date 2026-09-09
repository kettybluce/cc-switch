# CC Switch v4.1.4 自测报告（对照 v3.0.1）

> **诚实声明（必读）**  
> 本报告**不是**「完整自测通过」，也**不是**用户本机已验。  
> 下面每一行的 **Fixture PASS** 只表示：云端 Linux 上的 `WslRunner` / 夹具 / 前端单元测试按脚本契约跑通。  
> **≠ 已安装 MSI。≠ 真 Tauri 桌面 GUI。≠ 用户 PC 上的真 `wsl.exe`。**  
> 云端 agent **从未**在 Windows 上安装或启动本应用。用户在桌面任务里看不到安装/运行，是因为我们没做那一步。

**发包装车是 v4.1.5，不是 v4.1.4。** v4.1.4 MSI 没有 read `set --`，不要装。本审计不通知用户换 v4.1.4。

| 项 | 值 |
| --- | --- |
| 审计日期 | 2026-09-09 |
| 对照基线 | `v3.0.1` (`dcab4465`) |
| 审计起点 | PR #18 tip `a7162776`（`v4.1.4` tag `b9c2b694` 是更早的 bump） |
| 本报告代码 | 已合入 PR #18（含 read 修复 + 本诚实声明） |
| 用户网络夹具（**编码进测试，不是连上用户电脑**） | mirrored + dnsTunneling + firewall；listen `127.0.0.1:15721`；`localhost` → HTTP 404；`172.30.213.1` 与 `10.255.255.254` refused |
| 实际执行环境 | Cursor Cloud **Linux** VM：`LocalBashRunner` / `UserMirroredTopologyRunner` 跑与生产相同的 **bash 脚本常量**；Vitest 跑 React 组件。**没有** MSI、**没有** Tauri 窗口、**没有** `wsl.exe` |

## 测了什么 / 没测什么

| | 云端这次做了 | 没做（真机缺口） |
| --- | --- | --- |
| 安装 | 无 | **未**下载、未安装 `CC-Switch-v4.1.4-Windows.msi` |
| 进程 | `cargo test` / `pnpm test:unit` | **未**启动 `cc-switch.exe` / Tauri WebView |
| WSL | `WslRunner` 替身：本机 `bash -c` + 丢掉 `$1` 的双 | **未**调用用户 PC 的 `wsl.exe -d Ubuntu-22.04 -e bash` |
| 网络 | canned 探测行 + 本 VM 上 `127.0.0.1` 的假 HTTP 404 监听 | **未**打到用户 Hyper-V 镜像网卡、`172.30.213.1`、`10.255.255.254` |
| 文件 | 临时目录里的假 `~/.pi/agent/models.json` | **未**写用户发行版里的真 `~/.pi` / `~/.claude` / `~/.codex` |
| UI | Testing Library 点 mock 按钮 | **未**点真 Settings / 供应商卡片 / 会话页 |

## 场景表

每一行的结果列都是 **Fixture PASS**。含义：**夹具通过 ≠ 用户本机已验。**

| 场景 | 结果 | 证据（测试名/命令） | 真机缺口 |
| --- | --- | --- | --- |
| 1a. WSL 原子写 Pi `models.json`：拒绝空 bash；`mktemp` 在 `/tmp`；丢掉 `$1` 时 `set --` 回退 | **Fixture PASS** ≠ 真机已验 | `models_json_write_survives_wsl_dropping_positional_args`；`writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root`；`user_mirrored_firewall_dnstunnel_e2e_self_test`；`argv_rejects_an_empty_or_quote_only_script`；`dropped_arg_fallback_restores_positional_parameters` | 未在用户 Ubuntu-22.04 里看真 `models.json` / `/tmp/cc-switch-*` |
| 1b. WSL 原子写 Claude `settings.json` / Codex `config.toml`+`auth.json`：同上，且不走 UNC | **Fixture PASS** ≠ 真机已验 | `claude_codex_writes_survive_dropped_argv_on_user_topology`；`claude_and_codex_live_writes_use_wsl_runner_never_unc`；`wsl_cli::overwrite_script_stages_under_tmp_never_at_root` | 未写用户 `~/.claude` / `~/.codex`；未观察真 `wsl.exe` 包装 |
| 1c. 空目标不得在 `/` 上 `mktemp` | **Fixture PASS** ≠ 真机已验 | `empty_wsl_target_fails_without_mktemp_at_root` | 未在用户发行版根目录确认没有 `/.cc-switch-*` |
| 2. 代理探测与「检测代理」toast：mirrored 下 `127.0.0.1` HTTP 404 = 成功；NAT 回退 IP refused 不能盖过 localhost | **Fixture PASS** ≠ 真机已验 | `detect_treats_localhost_http_404_as_success_like_the_toast`；`mocked_host_probe_treats_localhost_404_as_success_and_skips_nat_hosts`；`live_curl_404_on_loopback_is_a_reachable_route`（**云 VM loopback**，不是用户网卡）；`any_http_status_counts_as_reachable`；`tests/lib/proxyOffDetail.test.ts` | 未点真「检测代理」按钮；未看真 toast；未从用户 WSL curl `15721` |
| 3. Settings 计划快照：聚焦/点击不跑多主机 WSL curl | **Fixture PASS** ≠ 真机已验 | `plan_ui_snapshot_never_invokes_wsl_runner`；`ui_snapshot_does_not_need_a_wsl_probe`；代码：`get_pi_proxy_plan` → `plan_ui_snapshot`；`usePiProxyPlan` `refetchOnWindowFocus: false` | 未在真窗口里反复聚焦 Settings 计时 |
| 4. 启用 Pi 供应商（`switch` → `insert_pi_provider` → read+write） | **Fixture PASS** ≠ 真机已验（夹具里发现并修了 1 处 read `$1`，见下） | `enable_pi_provider_on_user_topology_survives_dropped_argv`；`reading_survives_when_wsl_drops_positional_args` | 未在 GUI 点「启用 / 百胜」；未打开用户真 `models.json` |
| 5. 会话刷新不依赖失败的代理探测 | **Fixture PASS** ≠ 真机已验 | `session_refresh_does_not_depend_on_proxy_probe` | 未点会话页刷新；未对照用户真实 JSONL |
| 6. 相对 v3.0.1 写入契约：stdin → 暂存 → sha256 → `mv` | **Fixture PASS** ≠ 真机已验 | `atomic_write_scripts_stage_under_tmp_never_at_root`；对照 `v3.0.1` `files.rs` 同三段仍在 | 契约是读源码 + 夹具执行；未在用户盘上看 `mv` 痕迹 |

### 命令与计数（仅云端 Linux 夹具）

这些数字**不能**写成「完整自测通过」。

```text
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness
  23 passed; 0 failed          # LocalBashRunner，无 wsl.exe

cargo test --manifest-path src-tauri/Cargo.toml --lib pi_runtime::
  131 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_cli::
  4 passed; 0 failed

pnpm test:unit tests/lib/proxyOffDetail.test.ts \
  tests/pi-wsl-harness.test.ts tests/components/CliRuntimeSettings.test.tsx
  16 passed; 0 failed          # jsdom，无 Tauri 窗口
```

## 用户本机冒烟清单（装完 MSI 后点 4 下）

请继续用 **v3.0.1** 直到 parent 通知新 MSI。装上**含 read 修复**的包之后，在**这台** Windows（Ubuntu-22.04 / 代理 `127.0.0.1:15721`）上点：

1. **设置 → Pi 运行时 →「检测代理」。** 期望：成功 toast（localhost 已应答）。**不要**再出现 `no route … tried 127.0.0.1, 172.30.213.1, 10.255.255.254`。Settings 上的 `http://127.0.0.1:15721` 与 toast 同一结论。
2. **供应商列表 → 启用「百胜」（或任意未写入 live 的 Pi 卡）。** 期望：无 `/bin/bash: line 1: '': No such file or directory`。在 WSL 里看 `~/.pi/agent/models.json` 与 `~/.pi/models.json` 仍有旧供应商，且新卡在，没有被写成空文档。
3. **会话页 → 刷新。** 期望：列表能出来。即使上一次探测失败过，也不应整页被探测拖死或报「没路由」。
4. **再点一次设置页（切走再切回）。** 期望：没有 1–9 秒卡住（那是旧的三主机 WSL curl）。绿灯可以先亮；以第 1 步「检测代理」为准。

任一步 FAIL：停用新包，回到 v3.0.1，把 toast / bash 原文发回。夹具 PASS 不能替这 4 下。

## 审计中发现并已修（否则场景 4 的夹具会 FAIL）

启用「百胜」走 `ProviderService::switch` → `pi::enable` → `insert_pi_provider`，**先 `files::read` 再 write**。

v4.1.4 tip 只给 **WRITE / OVERWRITE** 加了 `set --`。`READ_SCRIPT` 仍直接读 `$1`。丢掉位置参数时：

```bash
[ ! -e "$1" ]   # $1 为空 → 当成 missing
```

随后按「空文档」插入，**有清空现有 providers 的风险**。

修复（仅此）：`read_wsl` 与 write 一样用 `set --` 补回已校验路径。未改 v3.0.1 写入契约，未做 UI。  
**这只在夹具里复现并修过；用户本机是否还会丢 `$1`，要靠上面第 2 下。**

## 对照 v3.0.1 写入契约（读代码）

v3.0.1 生产写：

1. stdin → `cat > "$tmp"`
2. `sha256sum < "$tmp"` 对账
3. `mv -f -- "$tmp" "$target"`
4. 再 `sha256sum < "$target"`

暂存当时是 `mktemp -- "$dir/.cc-switch-XXXXXX"`。`$1`/`$dir` 被丢掉时变成 `/.cc-switch-XXXXXX`。

v4.1.4 仍是这四步；**只改暂存目录** 为 `$TMPDIR` 或 `/tmp`，并加 `empty-target` 与 `set --`。`wsl.exe` 从 `-d <distro> -- bash` 改为 `-d <distro> -e bash`。  
夹具执行了这些脚本；**用户内核是否按同样 argv 跑，未验。**

## 相对 v3.0.1、触及 WSL 写 / 探测的文件

| 文件 | 相对 3.0.1 | 与写/探测的关系 |
| --- | --- | --- |
| `src-tauri/src/pi_runtime/files.rs` | M | Pi read·write·overwrite；`/tmp` mktemp；read/write 的 `set --` |
| `src-tauri/src/pi_runtime/wsl.rs` | M | `wsl.exe -e bash`；拒空脚本；`with_dropped_arg_fallback` |
| `src-tauri/src/pi_runtime/proxy.rs` | M | `/` 上 1xx–5xx（含 404）= 可达；`plan_ui_snapshot` 不 curl |
| `src-tauri/src/wsl_cli/mod.rs` | **A** | Claude / Codex 经 `wsl.exe` 的 overwrite |
| `src-tauri/src/commands/pi.rs` | M | 快照 vs `test_pi_proxy` |
| `src-tauri/src/services/pi_proxy.rs` | M | 投影 / 启用写入 |
| `src-tauri/src/services/provider/live.rs` | M | Claude live → `wsl_cli` |
| `src-tauri/src/services/proxy.rs` | M | Codex/Claude 接管 → `wsl_cli` |
| `src-tauri/src/pi_runtime/mirrored_topology.rs` | **A** | 把用户拓扑**编码成夹具**（不是连用户机） |
| `src-tauri/src/pi_runtime/wsl_linux_harness.rs` | **A** | Linux 云端 harness |
| `src/lib/query/pi.ts` | M | toast 映射；`refetchOnWindowFocus: false` |
| `src/components/settings/CliRuntimeSettings.tsx` | **A** | Settings 文案 /「检测代理」按钮 |
| `tests/lib/proxyOffDetail.test.ts` | **A** | toast 字符串契约（jsdom） |

## 剩余风险

1. **最大缺口：从未装/跑 Windows 应用。** Fixture PASS 不能代替冒烟清单。
2. 会话 manifest/fetch/delete 仍无 `set --`。夹具场景 5 只证明「刷新不读探测结果」。
3. Settings 快照在 port≠0 时把 localhost 标可达、不现场 curl（刻意）。真机以「检测代理」为准。
4. 投影路径仍会 `resolve_gateway`；那不是 Settings 点击路径。
5. 旧 tag / 旧 MSI（`a7162776`）不含 read 修复。含修复的 tip 在 PR #18；换包后仍须走本机 4 下冒烟。

## 给 parent 的一句话

**夹具审计通过；用户本机未验。不要对外说「完整自测通过」。** 通知换包之后，至少走完上面 4 下冒烟。
