# CC Switch v4.1.4 自测报告（对照 v3.0.1）

**结论：本报告覆盖的全部场景均为 PASS。**  
Parent 决定何时通知 MSI。本审计**不打 tag、不发 MSI、不通知用户**。

| 项 | 值 |
| --- | --- |
| 审计日期 | 2026-09-09 |
| 对照基线 | `v3.0.1` (`dcab4465`) |
| 审计起点 | PR #18 tip `a7162776`（`v4.1.4` tag `b9c2b694` 是更早的 bump） |
| 本报告代码 | stacked PR #19 tip（read 修复 `aa6bcec1` + 本文件） |
| 用户网络夹具 | mirrored + dnsTunneling + firewall；listen `127.0.0.1:15721`；`localhost` → HTTP 404；`172.30.213.1` 与 `10.255.255.254` refused |
| 执行环境 | Cursor Cloud Linux（`LocalBashRunner` 执行与生产相同的 bash 脚本常量；**没有**真 `wsl.exe`） |

## 场景表

| 场景 | 结果 | 证据（测试名/命令） |
| --- | --- | --- |
| 1a. WSL 原子写 Pi `models.json`：拒绝空 bash；`mktemp` 在 `/tmp`；丢掉 `$1` 时 `set --` 回退 | **PASS** | `models_json_write_survives_wsl_dropping_positional_args`；`writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root`；`user_mirrored_firewall_dnstunnel_e2e_self_test`；`pi_runtime::wsl::tests::argv_rejects_an_empty_or_quote_only_script`；`dropped_arg_fallback_restores_positional_parameters` |
| 1b. WSL 原子写 Claude `settings.json` / Codex `config.toml`+`auth.json`：同上，且不走 UNC | **PASS** | `claude_codex_writes_survive_dropped_argv_on_user_topology`；`claude_and_codex_live_writes_use_wsl_runner_never_unc`；`wsl_cli::tests::overwrite_script_stages_under_tmp_never_at_root` |
| 1c. 空目标不得在 `/` 上 `mktemp` | **PASS** | `empty_wsl_target_fails_without_mktemp_at_root`（断言 `empty-target`，且 stderr 不含 `/.cc-switch-XXXXXX`） |
| 2. 代理探测与「检测代理」toast 一致：mirrored 下 `127.0.0.1` HTTP 404 = 成功；NAT 回退 IP refused 不能盖过 localhost | **PASS** | `detect_treats_localhost_http_404_as_success_like_the_toast`；`mocked_host_probe_treats_localhost_404_as_success_and_skips_nat_hosts`；`live_curl_404_on_loopback_is_a_reachable_route`；`any_http_status_counts_as_reachable`；前端 `tests/lib/proxyOffDetail.test.ts`（HTTP 404 不是「请先开启本地代理」；`/ping` refused 才是） |
| 3. Settings 计划快照：聚焦/点击不跑多主机 WSL curl | **PASS** | `plan_ui_snapshot_never_invokes_wsl_runner`（runner 一调用即 panic）；`ui_snapshot_does_not_need_a_wsl_probe`；`get_pi_proxy_plan` → `plan_ui_snapshot`；`usePiProxyPlan` 设 `refetchOnWindowFocus: false` |
| 4. 启用 Pi 供应商端到端（`switch` → `insert_pi_provider` → read+write） | **PASS**（审计中发现并已修复 1 处，见下） | `enable_pi_provider_on_user_topology_survives_dropped_argv`（用户拓扑 + 丢掉 `$1` 启用「百胜」）；`reading_survives_when_wsl_drops_positional_args` |
| 5. 会话刷新不依赖失败的代理探测 | **PASS** | `session_refresh_does_not_depend_on_proxy_probe`（三主机全 `000` 时 `sessions::sync` 仍镜像 4 个 JSONL） |
| 6. 相对 v3.0.1 写入契约无回归：stdin → 暂存 → sha256 → `mv` | **PASS** | `atomic_write_scripts_stage_under_tmp_never_at_root`（断言脚本仍含 `cat > "$tmp"` / `sha256sum < "$tmp"` / `mv -f -- "$tmp" "$target"`）；对照 `v3.0.1` `files.rs` 同三段仍在 |

### 命令与计数（本机实测）

```text
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness
  23 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib pi_runtime::
  131 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_cli::
  4 passed; 0 failed

pnpm test:unit tests/lib/proxyOffDetail.test.ts \
  tests/pi-wsl-harness.test.ts tests/components/CliRuntimeSettings.test.tsx
  16 passed; 0 failed
```

## 审计中发现并已修复（否则场景 4 会 FAIL）

启用「百胜」走 `ProviderService::switch` → `pi::enable` → `insert_pi_provider`，**先 `files::read` 再 write**。

v4.1.4 tip 只给 **WRITE / OVERWRITE** 加了 `with_dropped_arg_fallback`（`set --`）。`READ_SCRIPT` 仍直接读 `$1`。当 `wsl.exe` 丢掉位置参数时：

```bash
[ ! -e "$1" ]   # $1 为空 → 当成 missing
```

随后 `write_models_document` 会按「空文档」插入新供应商，**有清空现有 providers 的风险**。这不是 UI 问题，是写入路径缺口。

修复（仅此）：`read_wsl` 与 write 一样用 `set --` 补回已校验的绝对路径和字节上限。未改写入契约，未做 UI / 新功能。

## 对照 v3.0.1 写入契约（读代码，不是口头）

v3.0.1（`src-tauri/src/pi_runtime/files.rs`）生产写是：

1. stdin → `cat > "$tmp"`
2. `sha256sum < "$tmp"` 对账
3. `mv -f -- "$tmp" "$target"`
4. 再 `sha256sum < "$target"` 校验

暂存位置当时是 `mktemp -- "$dir/.cc-switch-XXXXXX"`。当 `$1`/`$dir` 被 WSL 丢掉时，这会变成 `/.cc-switch-XXXXXX`（用户报告的根目录 mktemp）。

v4.1.4 仍是同一四步；**只改暂存目录** 为 `$TMPDIR` 或 `/tmp`（`mktemp -p`，失败则 `mktemp /tmp/cc-switch-XXXXXX`），并增加 `empty-target` 与 `set --` 回退。`wsl.exe` 从 `-d <distro> -- bash` 改为 `-d <distro> -e bash`，避免登录壳先展开 `$1`。

## 相对 v3.0.1、触及 WSL 写 / 探测的文件

| 文件 | 相对 3.0.1 | 与写/探测的关系 |
| --- | --- | --- |
| `src-tauri/src/pi_runtime/files.rs` | M | Pi `models.json` / settings 的 read·write·overwrite 脚本；`/tmp` mktemp；read/write 的 `set --` |
| `src-tauri/src/pi_runtime/wsl.rs` | M | `wsl.exe -e bash`；拒空脚本；`with_dropped_arg_fallback` |
| `src-tauri/src/pi_runtime/proxy.rs` | M | 探测脚本（`/` 上 1xx–5xx 含 404 = 可达）；`plan_ui_snapshot` 不 curl |
| `src-tauri/src/wsl_cli/mod.rs` | **A**（3.0.1 无此模块） | Claude / Codex 经 `wsl.exe` 的 overwrite（同一 stdin→sha256→mv） |
| `src-tauri/src/commands/pi.rs` | M | `get_pi_proxy_plan` 用快照；`test_pi_proxy` 用 `verify`（与探测同一成功规则） |
| `src-tauri/src/services/pi_proxy.rs` | M | 投影 `resolve_origin`：404 算可达；启用时 `projected_live_config` → `insert_pi_provider` |
| `src-tauri/src/services/provider/live.rs` | M | Claude live 写走 `wsl_cli::write_claude_settings` |
| `src-tauri/src/services/proxy.rs` | M | Codex/Claude 接管写走 `wsl_cli`；`rewrite_proxy_origin` |
| `src-tauri/src/pi_runtime/mirrored_topology.rs` | **A** | 用户夹具：Ubuntu-22.04 / 15721 / 172.30.213.1 / 10.255.255.254 |
| `src-tauri/src/pi_runtime/wsl_linux_harness.rs` | **A** | Linux 云端 harness |
| `src/lib/query/pi.ts` | M | toast：`isProxyOffDetail`；Settings 查询 `refetchOnWindowFocus: false` |
| `src/components/settings/CliRuntimeSettings.tsx` | **A**（3.0.1 无此文件） | Settings 绿灯来自快照；「检测代理」按钮走 `test_pi_proxy` |
| `tests/lib/proxyOffDetail.test.ts` | **A** | toast 映射契约 |

## 剩余风险（未在本报告中标 FAIL）

1. **本机没有真 `wsl.exe`。** 脚本与 argv 在 Linux bash 替身上跑通；Windows 上 `wsl.exe -e bash` 的真实包装仍需装 MSI 后在用户机抽检一次（parent 门禁）。
2. **会话脚本（manifest / fetch / delete）没有 `set --` 回退。** 场景 5 只证明刷新不依赖探测失败。若真机对会话脚本也丢 `$1`，刷新可能失败，但不会走启用供应商的 models.json 路径。未扩修（不在本次 FAIL 列表）。
3. **Settings 快照在 port≠0 时把 localhost 标为可达，不现场 curl。** 这是刻意的：避免聚焦卡 1–9 秒。监听未开时，「检测代理」仍应 toast「请先开启本地代理」。
4. **投影路径（接管开启 / 代理启动）仍会 `resolve_gateway` 多主机探测。** 这不是 Settings 点击路径。
5. **已发布的 v4.1.4 tag / 已挂的 MSI 不含本 PR 的 read 修复。** 若 parent 要发 MSI，应基于含 `aa6bcec1`（或其后）的 tip，而不是旧 tag `b9c2b694`。

## 给 parent 的一句话

场景表全部 PASS。可以决定是否在合并 PR #19 → #18 之后再打 Windows x64 MSI；在书面报告之前不要通知用户换包。
