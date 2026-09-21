# OpenCode / Hermes / OpenClaw provider surface tests (SCHEMA 18)

Offline smoke + fixture stubs for the three **additive** apps. No real network. No `SCHEMA_VERSION` bump.

累加式三应用的离线烟测与夹具：不访问真网、不升 SCHEMA（保持 **18**）。

## What exists / 现有实现

| Surface / 面 | Live file / 现场文件 | CRUD entry / 入口 | Proxy / 代理 |
| --- | --- | --- | --- |
| OpenCode | `opencode.json` (`provider` map) | `ProviderService::add/update/delete` + `get_opencode_live_provider_ids` | **None.** SCHEMA 18 `proxy_config` CHECK 不含 `opencode` |
| OpenClaw | `openclaw.json` (`models.providers`) | same + `get_openclaw_live_provider*` / `scan_openclaw_config_health` | **None.** |
| Hermes | `config.yaml` (`custom_providers`) | same + `get_hermes_live_provider_ids` | **None.** Pi-style takeover 也不适用 |

Pi reuses Claude listen via `settings.proxy_takeover_pi`. These three apps do **not**.

Pi 走 Claude listen + `proxy_takeover_pi`。这三套没有接管投影。

## Tests added / 本 PR 测试

Cargo integration: `src-tauri/tests/additive_provider_surface.rs` (isolated `CC_SWITCH_TEST_HOME`, config-dir overrides, POSIX stand-in under `profiles/additive/`).

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test additive_provider_surface -- --test-threads=1
```

Frontend (static catalogs, no fetch):

```bash
pnpm exec vitest run tests/config/additiveProviderSurface.smoke.test.ts tests/config/hermesProviderPresets.test.ts
```

Lib extras: `OpenCodeProviderConfig` unknown-field roundtrip; SCHEMA 18 CHECK rejects `proxy_config` rows for `opencode` / `openclaw` / `hermes` / `pi`.

## Fixtures / 夹具

`src-tauri/tests/fixtures/additive/`:

- `opencode_with_unknown_fields.json` — keeps `theme` / `customTopLevel` / `futureVendorFlag`
- `openclaw_with_unknown_fields.json` — keeps `customTopLevel` / `futureVendorFlag`; `tools.profile=default` for health scan
- `hermes_with_unknown_fields.yaml` — keeps `customTopLevel` / `rate_limit_delay` / `key_env` / `foo_bar`

## Gaps (highest-value tests still not claimed) / 缺口

These modules exist but are **out of this smoke**, or are incomplete in product:

这些模块存在但本烟测不覆盖，或产品本身未完成：

1. **Local proxy takeover / 本地代理接管** — no `supports_proxy_takeover()` for these apps. Do not invent `proxy_config` rows.
2. **OpenClaw MCP** — `McpService` still skips OpenClaw (`Issue #4834` in code comments). No MCP sync tests added.
3. **OpenClaw Skills** — `SkillApps` ignores OpenClaw. No skills projection.
4. **WSL UNC live paths** — unlike Pi/Claude/Codex, there is no WSL `\\wsl.localhost\…` projector for these three. Tests use isolated override dirs only.
5. **Fetch-models / stream-check / usage HTTP** — those talk to provider URLs. Not exercised here (would need a local stub server, never a public origin).
6. **Hermes `providers:` dict (v12+)** — read-only in CC Switch; edits belong in Hermes Web UI. This smoke only covers `custom_providers`.
7. **Oh My OpenCode / Slim** — exclusive-mode files, not `opencode.json` provider map. Existing `ProviderService` OMO tests remain the coverage; this smoke only asserts the preset stubs.
8. **Session JSONL / Hermes memory / OpenClaw workspace** — separate modules with their own unit tests; not provider CRUD.

## Hard no / 硬约束

- No `SCHEMA_VERSION` 19 / `migrate_v18_to_v19`
- No `proxy_config.app_type` in (`opencode`, `openclaw`, `hermes`, `pi`)
- No Portable / MSI from this test-only change
- No claim that Docker/Tauri GUI exercises these three apps
