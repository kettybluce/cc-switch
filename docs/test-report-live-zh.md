# 云端 Linux fixture 实测 — 不是 Windows MSI / 真 wsl.exe

> **标签（必须读）**：本次是 Cursor Cloud Agent 上的 **Ubuntu 24.04.4 Linux fixture 实测**。  
> **不是** Windows MSI 安装验收，**不是** 真机 `wsl.exe`，**没有** 用户本机 Ubuntu-22.04 / `machineId`。  
> **未打 tag、未发 MSI。** 请继续用 v3.0.1，等后续中文报告全表 PASS 后再决定是否通知安装。

| 项 | 值 |
| --- | --- |
| 仓库 / PR | `kettybluce/cc-switch` [PR #18](https://github.com/kettybluce/cc-switch/pull/18) |
| 分支 | `cursor/wsl-detect-mktemp-4.1.4-739a` |
| 实测 SHA | `a7162776d815a95447c691171cedd8eaf8a034c4`（与 `origin` 尖端一致） |
| 主机 | `cursor` · Linux 6.12.94+ x86_64 · Ubuntu 24.04.4 LTS |
| 时间（UTC） | 2026-09-09 03:15:03 — 03:18:30 |
| 工具链 | rustc/cargo 1.95.0 · node v22.14.0 · pnpm 10.12.3 · bash 5.2.21 · curl 8.5.0 · GNU find 4.9.0 |
| `wsl.exe` | **不存在**（`command -v wsl.exe` → no） |
| `msiexec` | **不存在** |
| 替身 | `LocalBashRunner` + `UserMirroredTopologyRunner`（本机 `bash` 执行编译期脚本常量；丢掉 `$1`；回放 mirrored + firewall + dnsTunneling 探测矩阵） |
| 夹具 | `tests/fixtures/pi-wsl/` 假 `~/.pi/agent` |
| 总评 | **本表全部 PASS。** 覆盖 #18 的 detect toast / Settings 快照 / 丢掉 `$1` 的 write / `/tmp` mktemp。**不含** stacked PR #19 的 `files::read` 回退。 |

完整命令日志副本：

- 云端 artifacts：`/opt/cursor/artifacts/test-report-live-zh.md`、`/opt/cursor/artifacts/logs/`、`/opt/cursor/artifacts/test-logs-live/`
- 仓库：`docs/test-logs-live/`（不含 crate 下载刷屏；编译尾部见 `01-cargo-compile-tail.txt`）

---

## 这台机器实际在测什么

产品路径是 Windows CC Switch → `wsl.exe -d Ubuntu-22.04 -- bash …`。云端没有 `wsl.exe`，所以按 `docs/dev-pi-wsl-test-harness-zh.md` 把「发行版里的 bash」换成 **本机 bash**：

```text
Windows 真机                         本次云端 Linux
─────────────────                    ─────────────────────────
CC Switch (Windows)                  cargo test --lib wsl_linux_harness
    │                                     │
    │ wsl.exe -d Ubuntu-22.04 -- bash     │ LocalBashRunner + UserMirroredTopologyRunner
    ▼                                     │   HOME=<temp> bash -c <同一段脚本常量>
Ubuntu-22.04  ~/.pi/agent/                ▼
                                          临时目录 ~/.pi/agent/（fixtures）
```

默认拓扑仍是用户机器上的矩阵（编码在 `USER_MIRRORED`，不是泛化 NAT）：

```
WSL 2.7.10, Ubuntu-22.04
networkingMode=mirrored, dnsTunneling=true, firewall=true, autoProxy=false
curl 127.0.0.1:15721/ → HTTP 404 SUCCESS
172.30.213.1 / 10.255.255.254 → connection refused
```

另外有一条 **真 Linux loopback**：`live_curl_404_on_loopback_is_a_reachable_route` 在本机 `127.0.0.1:0` bind 一个返回 HTTP 404 的 fixture，再用 harness 里的真实 `curl` 探测。这是云端 Linux 网络栈，不是 Windows。

---

## 跑过的命令

预备（编译 Tauri lib 所需，不是测 Windows）：

```bash
sudo apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev \
  libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev
pnpm install --frozen-lockfile
mkdir -p dist
```

实测（`RUST_BACKTRACE=1`，`--nocapture --test-threads=1`）：

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
cargo test --manifest-path src-tauri/Cargo.toml --lib user_mirrored_firewall_dnstunnel_e2e_self_test -- --nocapture --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness -- --nocapture --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib pi_runtime:: -- --nocapture --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib mirrored_topology -- --nocapture --test-threads=1
pnpm test:unit tests/pi-wsl-harness.test.ts tests/components/PiRuntimeSettings.test.tsx tests/lib/proxyOffDetail.test.ts
```

---

## PASS / FAIL 总表

| # | 命令 / 套件 | 结果 | 计数 | 耗时 |
| --- | --- | --- | --- | --- |
| 0 | 环境探测：Linux / 无 `wsl.exe` / 无 `msiexec` | **PASS** | — | — |
| 1 | `cargo test --lib --no-run`（拉 crate + 编译） | **PASS** | compile ok | 2m 45s（首次无 cache） |
| 2 | `user_mirrored_firewall_dnstunnel_e2e_self_test` | **PASS** | 1 passed; 0 failed | 0.05s（+ 13.74s 链 test bin） |
| 3 | `--lib wsl_linux_harness` | **PASS** | 20 passed; 0 failed | 2.79s |
| 4 | `--lib pi_runtime::` | **PASS** | 127 passed; 0 failed | 4.20s |
| 5 | `--lib mirrored_topology` | **PASS** | 1 passed; 0 failed | 0.00s |
| 6 | `pnpm test:unit` 相关 3 个文件 | **PASS** | 23 passed; 0 failed | 1.45s |
| — | **合计（去重后）** | **PASS** | cargo 127 + pnpm 23，0 fail | — |
| — | Windows MSI / 真 `wsl.exe` | **未跑（本环境没有）** | — | — |
| — | 打 tag / 发 MSI | **未做（按任务禁止）** | — | — |
| — | PR #19 `files::read` 丢掉 `$1` | **不在本次范围** | — | — |

首次 `cargo test --lib --no-run --offline` 因空 registry 找不到 `arboard` 失败，已去掉 `--offline` 重跑成功。这是编译环境问题，不是产品测试失败。

---

## E2E 断言（本次实际执行的那条）

`pi_runtime::wsl_linux_harness::user_mirrored_firewall_dnstunnel_e2e_self_test` 在本机跑过并 **ok**：

| 检查 | 期望 | 实测 |
| --- | --- | --- |
| 拓扑 | `networkingMode=mirrored` + firewall + dnsTunneling，distro `Ubuntu-22.04` | PASS |
| Settings 快照 | `plan_ui_snapshot` 绿灯，不 multi-host curl WSL；host=`127.0.0.1`，strategy=`MirroredLoopback` | PASS |
| 检测代理 | `resolve_gateway` 把 localhost HTTP 404 当成功；error 为空（不能 toast `no route`） | PASS |
| 丢掉 `$1` 写 Pi `models.json` | `UserMirroredTopologyRunner.drop_write_args=true`；stdin→stage→sha256→`mv`；agent 与 top 双写一致 | PASS |
| mktemp | `ATOMIC_STAGE_SNIPPET` 含 `mktemp /tmp/cc-switch-XXXXXX` 或 `mktemp -p`；不含 `$dir/.cc-switch-XXXXXX` | PASS |
| 空 bash | runner 拒绝空脚本 / `''` | PASS（写入未触发 empty bash） |

---

## `wsl_linux_harness` 20 条逐条

全部 **ok**（摘自 `04-wsl-linux-harness.txt`）：

| 测试 | 结果 |
| --- | --- |
| `claude_and_codex_live_writes_use_wsl_runner_never_unc` | PASS |
| `cwd_encode_decode_round_trips_the_documented_pi_layout` | PASS |
| `detect_treats_localhost_http_404_as_success_like_the_toast` | PASS |
| `jsonl_line_parse_prices_zero_embedded_cost_from_wsl_models_json` | PASS |
| `live_curl_404_on_loopback_is_a_reachable_route` | PASS（真 bind + curl） |
| `mocked_host_probe_falls_back_to_nat_gateway_when_loopback_is_dead` | PASS |
| `mocked_host_probe_prefers_mirrored_loopback_when_it_answers` | PASS |
| `mocked_host_probe_treats_localhost_404_as_success_and_skips_nat_hosts` | PASS |
| `models_json_write_survives_wsl_dropping_positional_args` | PASS |
| `probe_and_manifest_share_one_session_jsonl_maxdepth` | PASS |
| `probe_session_count_includes_nested_jsonl_not_just_the_sessions_root` | PASS |
| `project_and_restore_dual_write_wsl_agent_and_top_level_models` | PASS |
| `project_from_diverge_replaces_stale_top_proxy_then_restore_dual_writes` | PASS |
| `project_from_only_agent_creates_the_top_mirror_and_restore_keeps_lockstep` | PASS |
| `runner_discovers_nested_cwd_group_and_task_sessions_that_a_flat_glob_misses` | PASS |
| `session_refresh_does_not_depend_on_proxy_probe` | PASS |
| `user_mirrored_firewall_dnstunnel_e2e_self_test` | PASS |
| `writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root` | PASS |
| `wsl_cli_rejects_unc_overrides_in_harness` | PASS |
| `wsl_read_heals_identical_diverge_and_only_agent_models_mirrors` | PASS |

`pi_runtime::` 另外还包括 detect / files / proxy / rewrite / sessions / wsl argv 等，合计 127，其中与本次修复直接相关的还有：

- `files::tests::atomic_write_scripts_stage_under_tmp_never_at_root` … ok
- `files::tests::empty_wsl_target_fails_without_mktemp_at_root` … ok
- `proxy::tests::any_http_status_counts_as_reachable` … ok
- `proxy::tests::ui_snapshot_does_not_need_a_wsl_probe` … ok
- `proxy::tests::mirrored_firewall_dns_tunneling_topology_still_lists_localhost_first` … ok
- `wsl::tests::dropped_arg_fallback_restores_positional_parameters` … ok
- `wsl::tests::argv_rejects_an_empty_or_quote_only_script` … ok
- `mirrored_topology::tests::default_profile_is_the_user_mirrored_firewall_dnstunnel_matrix` … ok

---

## 附录：环境

```text
===== ENV =====
2026-09-09T03:15:03Z
hostname=cursor
uname=Linux 6.12.94+ x86_64
PRETTY_NAME="Ubuntu 24.04.4 LTS"
VERSION_ID="24.04"
pwd=/workspace
user=uid=1000(ubuntu) gid=1000(ubuntu) groups=1000(ubuntu),4(adm),20(dialout),24(cdrom),25(floppy),27(sudo),29(audio),30(dip),44(video),46(plugdev)
HOME=/home/ubuntu
git=cursor/wsl-detect-mktemp-4.1.4-739a a7162776d815a95447c691171cedd8eaf8a034c4
remote=a7162776d815a95447c691171cedd8eaf8a034c4
rustc=rustc 1.95.0 (59807616e 2026-04-14)
cargo=cargo 1.95.0 (f2d3ce0bd 2026-03-21)
node=v22.14.0
pnpm=10.12.3
bash=GNU bash, version 5.2.21(1)-release (x86_64-pc-linux-gnu)
find=find (GNU findutils) 4.9.0
curl=curl 8.5.0 (x86_64-pc-linux-gnu) libcurl/8.5.0 OpenSSL/3.0.13 zlib/1.3 brotli/1.1.0 zstd/1.5.5 libidn2/2.3.7 libpsl/0.21.2 (+libidn2/2.3.7) libssh/0.10.6/openssl/zlib nghttp2/1.59.0 librtmp/2.3 OpenLDAP/2.6.10
sha256sum=sha256sum (GNU coreutils) 9.4
HAS_WSL_EXE=no
HAS_MSIEXEC=no
webkit=2.52.6
gtk=3.24.41
```

## 附录：E2E 全日志

```text
===== CMD =====
cargo test --manifest-path src-tauri/Cargo.toml --lib user_mirrored_firewall_dnstunnel_e2e_self_test -- --nocapture --test-threads=1
===== START 2026-09-09T03:18:07Z =====
   Compiling cc-switch v4.1.4 (/workspace/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.74s
     Running unittests src/lib.rs (src-tauri/target/debug/deps/cc_switch_lib-64ddfb7ab989794f)

running 1 test
test pi_runtime::wsl_linux_harness::user_mirrored_firewall_dnstunnel_e2e_self_test ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2989 filtered out; finished in 0.05s

===== EXIT=0 END 2026-09-09T03:18:21Z =====
```

## 附录：`wsl_linux_harness` 全日志

```text
===== CMD =====
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness -- --nocapture --test-threads=1
===== START 2026-09-09T03:18:21Z =====
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.34s
     Running unittests src/lib.rs (src-tauri/target/debug/deps/cc_switch_lib-64ddfb7ab989794f)

running 20 tests
test pi_runtime::wsl_linux_harness::claude_and_codex_live_writes_use_wsl_runner_never_unc ... ok
test pi_runtime::wsl_linux_harness::cwd_encode_decode_round_trips_the_documented_pi_layout ... ok
test pi_runtime::wsl_linux_harness::detect_treats_localhost_http_404_as_success_like_the_toast ... ok
test pi_runtime::wsl_linux_harness::jsonl_line_parse_prices_zero_embedded_cost_from_wsl_models_json ... ok
test pi_runtime::wsl_linux_harness::live_curl_404_on_loopback_is_a_reachable_route ... ok
test pi_runtime::wsl_linux_harness::mocked_host_probe_falls_back_to_nat_gateway_when_loopback_is_dead ... ok
test pi_runtime::wsl_linux_harness::mocked_host_probe_prefers_mirrored_loopback_when_it_answers ... ok
test pi_runtime::wsl_linux_harness::mocked_host_probe_treats_localhost_404_as_success_and_skips_nat_hosts ... ok
test pi_runtime::wsl_linux_harness::models_json_write_survives_wsl_dropping_positional_args ... ok
test pi_runtime::wsl_linux_harness::probe_and_manifest_share_one_session_jsonl_maxdepth ... ok
test pi_runtime::wsl_linux_harness::probe_session_count_includes_nested_jsonl_not_just_the_sessions_root ... ok
test pi_runtime::wsl_linux_harness::project_and_restore_dual_write_wsl_agent_and_top_level_models ... ok
test pi_runtime::wsl_linux_harness::project_from_diverge_replaces_stale_top_proxy_then_restore_dual_writes ... ok
test pi_runtime::wsl_linux_harness::project_from_only_agent_creates_the_top_mirror_and_restore_keeps_lockstep ... ok
test pi_runtime::wsl_linux_harness::runner_discovers_nested_cwd_group_and_task_sessions_that_a_flat_glob_misses ... ok
test pi_runtime::wsl_linux_harness::session_refresh_does_not_depend_on_proxy_probe ... ok
test pi_runtime::wsl_linux_harness::user_mirrored_firewall_dnstunnel_e2e_self_test ... ok
test pi_runtime::wsl_linux_harness::writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root ... ok
test pi_runtime::wsl_linux_harness::wsl_cli_rejects_unc_overrides_in_harness ... ok
test pi_runtime::wsl_linux_harness::wsl_read_heals_identical_diverge_and_only_agent_models_mirrors ... ok

test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 2970 filtered out; finished in 2.79s

===== EXIT=0 END 2026-09-09T03:18:24Z =====
```

## 附录：`pi_runtime::` 结果行

127 条全部 ok。完整名单见 `/opt/cursor/artifacts/logs/05-pi-runtime.txt`。结尾：

```text
test pi_runtime::wsl_linux_harness::user_mirrored_firewall_dnstunnel_e2e_self_test ... ok
test pi_runtime::wsl_linux_harness::writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root ... ok
test pi_runtime::wsl_linux_harness::wsl_cli_rejects_unc_overrides_in_harness ... ok
test pi_runtime::wsl_linux_harness::wsl_read_heals_identical_diverge_and_only_agent_models_mirrors ... ok

test result: ok. 127 passed; 0 failed; 0 ignored; 0 measured; 2863 filtered out; finished in 4.20s

===== EXIT=0 END 2026-09-09T03:18:29Z =====
```

## 附录：`mirrored_topology`

```text
===== CMD =====
cargo test --manifest-path src-tauri/Cargo.toml --lib mirrored_topology -- --nocapture --test-threads=1
===== START 2026-09-09T03:18:30Z =====
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.31s
     Running unittests src/lib.rs (src-tauri/target/debug/deps/cc_switch_lib-64ddfb7ab989794f)

running 1 test
test pi_runtime::mirrored_topology::tests::default_profile_is_the_user_mirrored_firewall_dnstunnel_matrix ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2989 filtered out; finished in 0.00s

===== EXIT=0 END 2026-09-09T03:18:30Z =====
```

## 附录：pnpm

```text
===== CMD =====
pnpm test:unit tests/pi-wsl-harness.test.ts tests/components/PiRuntimeSettings.test.tsx tests/lib/proxyOffDetail.test.ts
===== START 2026-09-09T03:15:15Z =====

> cc-switch@4.1.4 test:unit /workspace
> vitest run tests/pi-wsl-harness.test.ts tests/components/PiRuntimeSettings.test.tsx tests/lib/proxyOffDetail.test.ts


 RUN  v2.1.9 /workspace

[baseline-browser-mapping] The data in this module is over two months old.  To ensure accurate Baseline data, please update: `npm i baseline-browser-mapping@latest -D`
 ✓ tests/pi-wsl-harness.test.ts (8 tests) 8ms
 ✓ tests/lib/proxyOffDetail.test.ts (3 tests) 5ms
 ✓ tests/components/PiRuntimeSettings.test.tsx (12 tests) 246ms

 Test Files  3 passed (3)
      Tests  23 passed (23)
   Start at  03:15:16
   Duration  1.45s (transform 201ms, setup 886ms, collect 399ms, tests 259ms, environment 995ms, prepare 167ms)

===== EXIT=0 END 2026-09-09T03:15:17Z =====
```

## 附录：编译（节选）

`--offline` 第一次失败（空 cargo registry），随后联网编译成功：

```text
===== CARGO COMPILE 2026-09-09T03:15:15Z =====
error: no matching package named `arboard` found
location searched: crates.io index
required by package `cc-switch v4.1.4 (/workspace/src-tauri)`
As a reminder, you're using offline mode (--offline) which can sometimes cause surprising resolution failures, if this error is too confusing you may wish to retry without `--offline`.
===== retry without --offline 2026-09-09T03:15:16Z =====
    Updating crates.io index
 Downloading crates ...
…
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2m 45s
COMPILE_EXIT=0
```

---

## 结论

1. **云端 Linux fixture 实测全表 PASS**（cargo 127 + pnpm 23）。  
2. 这 **不是** Windows MSI / 真 `wsl.exe` 验收；本机确认没有这两样东西。  
3. #18 声称的 mirrored detect toast、Settings 不 curl WSL、丢掉 `$1` 仍能写 `models.json`、mktemp 只在 `/tmp` —— 在这套 harness 上成立。  
4. **不要装正在打的 MSI。不要打 tag。** enable 路径的 `files::read` 回退以 [PR #19](https://github.com/kettybluce/cc-switch/pull/19) 的报告为准。
