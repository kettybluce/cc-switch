# Cloud fixture plan — Pi WSL / Claude badge / usage (no Windows host)

This branch cannot log into the user's PC. Evidence is Linux-cloud unit/integration tests plus path-string fixtures that match the live layout:

```
\\wsl.localhost\<distro>\home\tfdx8045\.claude
\\wsl.localhost\<distro>\home\tfdx8045\.pi\agent
\\wsl.localhost\<distro>\home\tfdx8045\.pi\agent\sessions
```

No C: `pi-wsl-sessions` mirroring. No SCHEMA bump. No separate Pi gateway. No MSI/Portable ship from this PR.

## What the fixtures stand in for

| Live Windows + WSL path | Cloud stand-in |
| --- | --- |
| Claude override `\\wsl.localhost\…\home\tfdx8045\.claude` | String parser + (Windows-only) `Path::components()` UNC tests |
| Pi override `\\wsl.localhost\…\home\tfdx8045\.pi` or `…\.pi\agent` | `canonicalize_pi_agent_dir` / `resolve_pi_agent_dir` UNC tests |
| Pi sessions `…\.pi\agent\sessions` | `session_roots()` under `TestAgentDir` + UNC canonicalize |
| Shared local proxy listen | `pi_takeover_projects_models_json_onto_claude_listen_without_schema_row` |
| Pi usage filter in UI | Vitest: always-visible BarChart2; `initialAppType: "pi"` |

Home username `tfdx8045` is only a fixture label in tests; distro name is taken from the UNC server share (`Ubuntu-22.04` in samples), same as official `wsl_distro_from_path`.

## Commands (this cloud Linux VM)

```bash
pnpm typecheck
pnpm format:check
pnpm test:unit
cd src-tauri && cargo fmt --check
cd src-tauri && cargo clippy -- -D warnings
# GTK/WebKit already installed (same packages as .github/workflows/ci.yml)
mkdir -p dist
cargo test --manifest-path src-tauri/Cargo.toml --lib \
  wsl_distro_from_path_str \
  canonicalize_dot_pi \
  wsl_localhost \
  usage_app_type_from_headers \
  projection_ \
  default_session_root \
  pi_takeover_projects \
  pi_header_selects \
  pi_selection_
# Full crate (CI equivalent) when time allows:
# cargo test --manifest-path src-tauri/Cargo.toml
```

Cargo's `TESTNAME` is a single substring filter; run the names above as separate invocations (or one regex if the toolchain accepts it).

## Mapped tests

### 1. Claude WSL badge (path-based, official override)

- Official match: `wsl_distro_for_tool` still reads `get_*_override_dir()` (farion1231), not PATH/binary location.
- Extra: `wsl_distro_from_path_str` so mixed slashes / `\\?\UNC\…` still yield the distro when `Path::components()` does not (Linux CI cannot exercise `Prefix::UNC`).
- Tests: `wsl_distro_from_path_str_parses_claude_and_pi_unc` (includes `home\tfdx8045\.claude` and `…\.pi\agent`).
- Windows-only: `wsl_distro_from_path_uses_official_unc_components`.

### 2. Pi Usage Stats entry always visible

- `appHasUsageDashboard("pi")` — not gated on takeover.
- `tests/integration/App.test.tsx`: Pi page, MSW takeover all `false`, title `使用统计` present.
- `tests/components/UsageDashboard.test.tsx`: `initialAppType: "pi"` filters queries.

### 3. Pi sessions = `.pi/agent/sessions`

- `canonicalize_dot_pi_to_agent_dir` (POSIX `.pi` → `.pi/agent`, UNC `.pi` → `.pi\agent`, including `tfdx8045`).
- `wsl_localhost_dot_pi_override_canonicalizes_to_agent` (no `pi-wsl-sessions`).
- `default_session_root_is_agent_sessions_not_a_windows_mirror`.

### 4. Usage can show Pi (proxy logs + session JSONL)

- Projection injects `headers["x-cc-switch-app"] = "pi"`; existing `headers` preserved.
- `usage_app_type_from_headers` → logging `app_type=pi` without calling `get_proxy_config_for_app("pi")`.
- Header stripped hop-by-hop in `forwarder.rs` (not sent upstream).
- Shared listen still uses Claude / Codex / Gemini handlers; `x-cc-switch-app: pi` selects forwardable Pi catalog providers (not Claude/Codex current). Projected `PROXY_MANAGED` nodes are skipped.
- Session importer already writes `app_type = "pi"` from agent JSONL (`session_usage_pi.rs`). `proxy_request_logs.app_type` has no CHECK; SCHEMA stays 18.

### 5. Claude-parity local proxy projection

- `pi_takeover_projects_models_json_onto_claude_listen_without_schema_row`:
  - `SCHEMA_VERSION == 18`
  - zero `proxy_config` rows with `app_type='pi'`
  - `models.json` projected onto existing Claude listen (`0.0.0.0` → `127.0.0.1`)
  - `auth.json` / `settings.json` defaults untouched
  - `x-cc-switch-app: pi` present on projected Anthropic node
- `pi_header_selects_pi_providers_not_claude_or_codex`: header routes Claude listen → Pi anthropic card, Codex listen → Pi openai card; without header Claude current stays selected.
- `pi_selection_skips_projected_placeholder_and_prefers_model`
- `pi_selection_does_not_read_or_insert_proxy_config_pi_row` (`SCHEMA_VERSION == 18`)

### 6. MSI Error 5

- Docs only: `docs/windows-msi-error-5-zh.md`, pointers in `README_ZH.md` and `docs/user-manual/zh/1-getting-started/1.2-installation.md`.
- WiX unchanged (`InstallScope=perUser`). This PR does **not** tag, ship MSI, or ship Portable.

## Untested (honest)

| Item | Why cloud cannot claim it |
| --- | --- |
| MSI Error 5 on a real `D:\Config.Msi` ACL | Needs the user's Windows Installer cache; documented Portable-first only |
| Real Win32 `Path::components()` on live `\\wsl.localhost\…` from Explorer | Linux CI; Windows-only unit test exists but this VM is Linux |
| About-tab badge pixels on a real CC Switch window | No user desktop; frontend still uses official `env_type` + `wsl_distro` |
| Live Pi CLI traffic through the shared listen on Windows | E2E projection + header logging unit tests only |
| GitHub Actions `cargo test` full crate on this PR | Recorded in the PR after this VM run; Actions may still re-run |

## Recorded on this cloud Linux VM (2026-09-09)

| Command | Result |
| --- | --- |
| `pnpm typecheck` | pass |
| `pnpm format:check` | pass (via `pnpm format` then typecheck; src formatted) |
| `pnpm test:unit` | **1094 passed**, 135 files |
| `cargo fmt --check --manifest-path src-tauri/Cargo.toml` | pass |
| `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` | pass |
| `cargo test --lib wsl_distro_from_path_str` | ok (1) |
| `cargo test --lib canonicalize_dot_pi` | ok (1) |
| `cargo test --lib wsl_localhost` | ok (3) |
| `cargo test --lib usage_app_type_from_headers` | ok (1) |
| `cargo test --lib projection_` | ok (8) |
| `cargo test --lib default_session_root` | ok (1) |
| `cargo test --lib pi_takeover_projects` | ok (1) |

Full `cargo test --manifest-path src-tauri/Cargo.toml` (lib + integration, ~2800+ lib tests) was **not** re-run in full on this VM after the last fixture-string edit; clippy rebuilt the lib cleanly and the mapped tests above passed. GitHub Actions CI on the PR is the full crate gate.

## Hard no (must stay true after merge)

- No `SCHEMA_VERSION` 19 / `migrate_v18_to_v19`
- No `/pi/` gateway or extra listen port
- No writes to `defaultProvider` / `defaultModel` / `auth.json`
- No C: session mirror directory
- No release tag / MSI / Portable artifact from this change
