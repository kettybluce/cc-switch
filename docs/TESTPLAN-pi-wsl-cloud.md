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

# Docker (no host GTK install, no Windows C:):
# docker compose -f docker-compose.test.yml run --build --rm backend-self-test
# See docs/docker-backend-self-test.md
```

Cargo's `TESTNAME` is a single substring filter; run the names above as separate invocations (or one regex if the toolchain accepts it).

## Mapped tests

### 1. Claude WSL badge (path-based, official override)

- Official match: `wsl_distro_for_tool` still reads `get_*_override_dir()` (farion1231), not PATH/binary location.
- Extra: `wsl_distro_from_path_str` so mixed slashes / `\\?\UNC\…` still yield the distro when `Path::components()` does not (Linux CI cannot exercise `Prefix::UNC`).
- Tests: `wsl_distro_from_path_str_parses_claude_and_pi_unc` (includes `home\tfdx8045\.claude` and `…\.pi\agent`).
- Windows-only: `wsl_distro_from_path_uses_official_unc_components`.

### 1b. Pi live models.json WSL home + default provider (this PR)

- Path order: `pi_config_dir` → `PI_CODING_AGENT_DIR` → inferred WSL UNC home from Claude/Codex override → `~/.pi/agent`.
- Tests: `wsl_unc_home_follows_claude_or_codex_home_style_dirs`, `pi_config_dir_still_wins_over_inferred_wsl_home`.
- Live CRUD writes `get_pi_agent_dir()/models.json`; optional legacy `~/.pi/models.json` mirror only if that file already exists (`legacy_root_models_json_is_mirrored_only_when_it_already_exists`).
- Delete/update refresh Pi full-file takeover backup: `live_delete_refreshes_takeover_backup_without_restoring_deleted_nodes`, `live_update_refreshes_takeover_backup_with_unprojected_url`.
- Default provider: `write_defaults_preserves_unknown_settings_fields`, `removing_the_default_reassigns_or_clears`, `set_default_provider_writes_settings_and_a_model_id`. SCHEMA 18: no `proxy_config` `pi` row; takeover remains `settings.proxy_takeover_pi`. `auth.json` untouched.

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
- Isolated-HOME JSONL scan (`src-tauri/tests/session_usage_scan.rs`): Claude `projects/` + sub-agent, Codex `sessions/YYYY/MM/DD`, Pi `.pi/agent/sessions/<project>`. Asserts usage aggregation, stored TTFT dash (`latency_ms=0`, `first_token_ms` NULL), empty dirs, corrupt lines. No `pi-wsl-sessions`, no SCHEMA bump.

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

### 7. CodexLiveAuthSwitchGuard stale ChatGPT bindings (Wave B #7395 follow-up)

Wave B already landed `CodexLiveAuthSwitchGuard`. This coverage adds the two-state edges that the cherry-pick tests did not name explicitly. **No SCHEMA bump. Default Cursor pool model unchanged.**

| Case | Test |
| --- | --- |
| `MissingAccount` vs corrupt store | `missing_account_recovery_requires_valid_persisted_state` |
| `ExistingAccount(None)` does not delete native `auth.json` | `existing_account_without_matching_live_token_is_distinct_from_missing`, `existing_account_without_live_token_can_switch_away_from_stale_binding` |
| `ExistingAccount(Some)` compare-before-delete / rotated refresh | `existing_account_with_matching_live_refresh_carries_disk_token`, `existing_account_ensure_unchanged_rejects_rotated_live_refresh` |
| MissingAccount only drops its marker | `missing_account_clear_outgoing_only_drops_its_ownership_marker`, `missing_account_current_can_switch_away_to_third_party` |
| Workspace mismatch fail-closed | `workspace_mismatch_between_store_and_live_auth_is_rejected` |
| Stale **target** while takeover already enabled | `stale_target_binding_reports_choose_account_even_when_takeover_already_enabled`, `linux_standin_stale_codex_oauth_target_reports_choose_account_during_takeover` |
| Linux MissingAccount current switch-away | `linux_standin_missing_codex_oauth_current_can_switch_away` |
| Card **选择账号** on current and non-current | `ProviderCard.codexAccount.test.tsx` |

Chinese recovery: `docs/guides/codex-stale-account-binding-zh.md`, FAQ, `2.2-switch.md`.

## Untested (honest)

| Item | Why cloud cannot claim it |
| --- | --- |
| MSI Error 5 on a real `D:\Config.Msi` ACL | Needs the user's Windows Installer cache; documented Portable-first only |
| Real Win32 `Path::components()` on live `\\wsl.localhost\…` from Explorer | Linux CI; Windows-only unit test exists but this VM is Linux |
| About-tab badge pixels on a real CC Switch window | No user desktop; frontend still uses official `env_type` + `wsl_distro` |
| Live Pi CLI traffic through the shared listen on Windows | E2E projection + header logging unit tests only |
| GitHub Actions `cargo test` full crate on this PR | Recorded in the PR after this VM run; Actions may still re-run |

## Recorded on this cloud Linux VM (2026-09-11, Pi CRUD / fetch A / default)

| Command | Result |
| --- | --- |
| `pnpm typecheck` | pass |
| `pnpm format:check` (`src/**`) | pass |
| `pnpm test:unit` | 135 files / 1102 tests pass |
| `cargo fmt --check --manifest-path src-tauri/Cargo.toml` | pass |
| `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` | pass |
| `cargo test --lib pi_config::` | ok (33) |
| `cargo test --lib services::provider::pi::` | ok (21) |

Full `cargo test --manifest-path src-tauri/Cargo.toml` was **not** re-run in full on this VM; GitHub Actions CI on the PR is the full crate gate.

## Recorded on this cloud Linux VM (2026-09-10, Pi provider select)

| Command | Result |
| --- | --- |
| `cargo fmt --check --manifest-path src-tauri/Cargo.toml` | pass |
| `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` | pass |
| `cargo test --lib pi_header_selects` | ok (1) |
| `cargo test --lib pi_selection_` | ok (2) |
| `cargo test --lib pi_api_matches` | ok (1) |
| `cargo test --lib pi_forwardable` | ok (1) |
| `cargo test --lib copy_pi_base_url` | ok (1) |
| `cargo test --lib extract_base_url_from_pi_native` | ok (3) |
| `cargo test --lib usage_app_type_from_headers` | ok (1) |
| `cargo test --lib projection_` | ok (8) |
| `cargo test --lib pi_takeover_projects` | ok (1) |
| `cargo test --lib provider_router` | ok (11) |

Full `cargo test --manifest-path src-tauri/Cargo.toml` (lib + integration, ~2800+ lib tests) was **not** re-run in full on this VM after the last fixture-string edit; clippy rebuilt the lib cleanly and the mapped tests above passed. GitHub Actions CI on the PR is the full crate gate.

### 7. Claude / Codex takeover roundtrip (Wave D, parallel to Pi)

Linux stand-in tests in `src-tauri/tests/proxy_projection_linux.rs` (same POSIX tree as #43/#46):

- `linux_standin_claude_takeover_roundtrip_preserves_unknown_settings_fields` — extra `customTopLevel` / `permissions` / `CC_SWITCH_KEEP` survive projection; disable restores the real token/URL
- `linux_standin_codex_takeover_roundtrip_preserves_toml_and_auth` — extra `[projects."/tmp/cc-switch-keep"]` survives; `auth.json` extra fields are byte-identical through enable/disable
- `linux_standin_claude_switch_during_takeover_refreshes_backup` / `linux_standin_codex_switch_during_takeover_refreshes_backup` — hot-switch refreshes `proxy_live_backup` so disable restores the **new** card (Pi delete-during-takeover analogue)
- `linux_standin_independent_disable_leaves_the_other_app_projected` — disable Claude while Codex stays projected (and the reverse)

### 8. Claude / Codex takeover fail-closed (unique remainder of #52)

Happy-path roundtrips landed in #51. This layer only adds:

- `live_points_at_foreign_local_proxy` — enable refuses when Live already aims at another local listen (`127.0.0.1:9999`)
- `linux_standin_*_fails_closed_when_*_missing` — missing `settings.json` / `config.toml`
- `linux_standin_*_fails_closed_on_malformed_*` — broken JSON/TOML
- `linux_standin_*_fails_closed_when_proxy_already_pointing_elsewhere` — foreign local proxy (Claude / Codex / **Pi**)
- `linux_standin_pi_takeover_fails_closed_on_malformed_models_json` — broken Pi `models.json`

Enable must error, leave Live unchanged, persist no backup, and leave the takeover flag off (`proxy_takeover_pi` for Pi).

`SCHEMA_VERSION` stays 18. No `proxy_config` row for `pi`.

Docker: default `proxy_projection_linux` + `session_usage_scan` + `provider_profile_race`; full crate via `TEST_FILTER=all` / `pnpm test:docker:all` / `make test-docker-all` (see `docs/docker-backend-self-test.md`).

## Hard no (must stay true after merge)

- No `SCHEMA_VERSION` 19 / `migrate_v18_to_v19`
- No `/pi/` gateway or extra listen port
- No writes to `auth.json`
- No C: session mirror directory
- No release tag / MSI / Portable artifact from this change
