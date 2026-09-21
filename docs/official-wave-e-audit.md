# Official main vs fork main — Wave E audit (SCHEMA 18)

Date: 2026-09-21. Fork `main` at audit start: `49495c35`. Official `farion1231/cc-switch` **main HEAD is still `8272707d`** (`#6381`, same as Wave C / Wave D).

This wave **does not cherry-pick code**. Official `main` has not moved since Wave C. Every merged commit after official 3.20.2 (`b6254432`) is either already on this fork (Wave 1–3 / A+B / C) or remains skipped for schema / product-scope / marketing. Unmerged official PRs are listed so the next wave can re-check after they land on official `main`.

Hard constraints held:

- `SCHEMA_VERSION` stays **18**. No `migrate_v18_to_v19`.
- No wholesale merge / reverse-diff of official `main` onto fork `main` (that would drop Pi / WSL / `proxy_takeover_pi`).
- Default Cursor pool model unchanged.
- Cherry-picks, when they exist, are taken only from **merged official `main`**.

## Verdict

| Bucket | Count this wave | Action |
| --- | --- | --- |
| **SAFE** (merged, SCHEMA 18, helps providers / presets / bugfixes, clean on fork) | **0 new** | Nothing to land |
| **Conflict-risk** | see below | Do not pick |
| **Skip-schema** | `#7383` + unmerged harness PRs | Permanent until the fork deliberately leaves SCHEMA 18 |

Wave D already recorded the same official HEAD. This document is the full post-Wave-D re-audit.

## SAFE — already on the fork (do not re-pick)

Official `b6254432` (3.20.2) → `8272707d` (HEAD). These merged commits are on fork `main` via Wave 1–3 (`#39`), Wave A+B (`#41`), salvage (`#42`), or Wave C (`#45`).

| Official SHA | Official PR / note | Fork path |
| --- | --- | --- |
| `8272707d` | #6381 skillId ≠ dirname | Wave C #45 |
| `fdbe3a85` | #7515 OpenCode batch-add models | Wave B #41 |
| `1408f382` | #7526 Kimi Global presets | Wave A #41 |
| `a659440b` | #7194 refresh prompts after external edits | Wave A #41 |
| `33c80626` | #7489 skill archive entry limit | Wave A #41 |
| `15884b20` | #7395 CodexLiveAuthSwitchGuard | Wave B #41 |
| `bd247a4a` | #7319 omit null tool descriptions | Wave 1 #39 |
| `c6286e14` | #7318 grok-4.6 / `xhigh` | Wave 3 #39 |
| `f49c7d68` | #7330 Zhipu Responses model lists | Wave 3 #39 |
| `556bb2ca` | #7346 npm dist-tags endpoint + timeout | Wave A #41 |
| `dc0febe5` | #3369 tokens/s in request log | Wave 3 #39 |
| `1d5d90f4` | APIKEY.FUN → apikey.fan | Wave B #41 |
| `746e2288` | DeepSeek V4 → V4.1 Flash pricing | Wave 1 #39 |
| `6867b64d` | Token Plan DeepSeek V4 `inputModalities: ["text"]` | Wave 1 #39 (present in `codexProviderPresets.ts` + `codexProviderPresets.tokenPlanTextOnly.test.ts`) |
| `f874803f` | DouBaoSeed display-name i18n | Wave C #45 (Pi `providerKey` `cc-switch-dou-bao-seed` unchanged) |
| `45b9a952` | skip unchanged incomplete rollout tails | Wave 1 #39 |
| `5c053626` | Claude “apply to all roles” form order | Wave C #45 |
| `3b1292fe` | tray bound ChatGPT quota | Wave 3 #39 |
| `deb0e874` | Disable Artifact Tool toggle | Wave 1 #39 |
| `5e129273` | aggregator Codex catalogs | Wave 3 #39 |
| `08984ba5` | Kimi Codex native Responses | Wave 3 #39 |
| `bfbbf15c` | Fable weekly limit from `limits[]` | Wave B #41 |
| `7726c834` | #7286 DeepSeek vision catalog | Wave 1 #39 |
| `6e4b0e6e` | #7287 `max_tokens` floor 16 | Wave 3 #39 |
| `5e0f3442` | #7280 coalesce commentary + tool_calls | Wave 1 #39 |
| `b78192e8` | #7227 empty `reasoning_content` | Wave 1 #39 |
| `45f9e819` | #7219 rollout byte cursor | Wave 1 #39 |
| `e0982799` | #7177 Images API follow-ups | Wave 2 #39 |
| `11317c62` | #7210 per-app proxy settings on shutdown | Wave 2 #41 / #39 (`proxy_takeover_pi` kept) |
| `e0c2fd2b` | #7212 preserve child metadata on sync | Wave 2 #39 |
| `2f3c0262` | #7183 千问AI平台 / Qwen presets | Wave 3 #39 |
| `99f9dd2c` | #7263 honor proxy when `model_provider` omitted | Wave 2 #39 |
| `2d54e261` | #7255 MiniMax M3 defaults | Wave 3 #39 |

No additional merged provider / preset / bugfix commit exists on official `main` after `8272707d`.

## Skip-schema (permanent while this fork is SCHEMA 18)

| Item | SHA | Why |
| --- | --- | --- |
| #7383 MiniMax Code harness | `06082e18` | Official `SCHEMA_VERSION` **19**, `migrate_v18_to_v19`, new app tables / MCP / skills flags. Taking any slice that needs that migration is disallowed. MiniMax as a tenth managed app cannot land without a schema bump. |
| Official 3.20.3 version bump | `1a725016` | Ships the SCHEMA 19 tree as “3.20.3”. Fork must not take official version numbers. |
| Official 3.20.3 release notes | `d695a2d7` | Documents MiniMax / SCHEMA 19. Fork keeps its own 3.20.x notes. |
| Unmerged #7496 DevEco Code | (not on `main`) | PR text: **SCHEMA 19 → 20**, `enabled_deveco` on `mcp_servers` / `skills`. Same class as MiniMax. |

Official `main` currently has `pub(crate) const SCHEMA_VERSION: i32 = 19` in `src-tauri/src/database/mod.rs`. A reverse-diff or merge of that tree would brick SCHEMA 18 databases and drop fork Pi takeover.

## Conflict-risk / product-scope skip (merged, not picked)

Cherry-pick of #7331 was **dry-run on this branch**: `git cherry-pick --no-commit 42ac174d` applied with **zero conflicts**. It is not a merge conflict. It is skipped for product-scope.

| Item | SHA | Bucket | Why not this wave |
| --- | --- | --- | --- |
| #7331 Linux Claude Desktop 3P | `42ac174d` | Product-scope skip (applies cleanly) | Adds Linux XDG / Flatpak host `~/.config` paths + manuals. **No schema.** Does not touch Pi. This fork ships **Windows x64 Portable + MSI** (macOS best-effort) and does **not** ship Linux packages. Windows 3P already works; WSL users use Windows Claude Desktop, not Linux 3P. Revisit only if the fork starts shipping a Linux build. |
| #7522 Kimi Code plan sponsor CTAs | `f2d0b2a6` | Skip (marketing) | README-only dual-region sponsor links. Not a provider/preset/bugfix. |
| Atlas Cloud de-sponsor | `f21e0944` | Conflict-risk + product-scope | Removes `partnerPromotionKey` / banner across **all** preset files including **Pi**. Wave 3 already refreshed Atlas glm-5.2 catalogs while **keeping** the sponsor slot. Applying this would rewrite Pi preset metadata and is a reverse-diff of that decision. |
| Official 3.20.2 notes | `f3b18df1` | Skip (docs) | Official release notes; fork has its own. |

## Unmerged official PRs (not on `main` — do not cherry-pick yet)

Previous waves only pick commits that have already merged to official `main`. These are still open / `mergeable_state: blocked` as of this audit. Listed so Wave F can re-check.

### Would be SAFE *candidates* after official merge (SCHEMA 18, user-facing)

Re-evaluate on the fork at merge time (file drift, especially proxy / OpenCode).

| Official PR | Title | Why it might help | Why not now |
| --- | --- | --- | --- |
| [#7543](https://github.com/farion1231/cc-switch/pull/7543) | OpenCode: fall back to model IDs for blank display names | Blank `name` in live OpenCode config | Unmerged; single-file write-path change must not collide with fork OpenCode batch-add (#7515) |
| [#7541](https://github.com/farion1231/cc-switch/pull/7541) | OpenCode V2 session usage tables | Usage import after OpenCode V2 (`session_v2`) | Unmerged; reads OpenCode’s SQLite, not CC Switch schema — still wait for official `main` |
| [#7531](https://github.com/farion1231/cc-switch/pull/7531) | Preserve `max` effort for GPT-5.6 / GPT-6 Astra | Claude `/effort max` currently mapped to `xhigh` for models that now have a real `max` tier | Unmerged; patches `transform.rs` which this fork already hardened (4xx/5xx, truncated SSE, 1214 remap). **Conflict-risk** until diffed against fork proxy |
| [#7520](https://github.com/farion1231/cc-switch/pull/7520) | Settings shortcut opens General tab | Small UX bugfix | Unmerged |
| [#7510](https://github.com/farion1231/cc-switch/pull/7510) | Show resolved skills storage dir | Settings hint accuracy | Unmerged |
| [#7509](https://github.com/farion1231/cc-switch/pull/7509) | Synthesize outputs for orphan Codex tool calls | Proxy replay bugfix | Unmerged; proxy = conflict-risk vs fork 1214 / fail-closed |
| [#7505](https://github.com/farion1231/cc-switch/pull/7505) | Confirm bulk skill app toggles | Skills UX | Unmerged |

### Conflict-risk even after merge

| Official PR | Title | Why |
| --- | --- | --- |
| [#7514](https://github.com/farion1231/cc-switch/pull/7514) / [#7502](https://github.com/farion1231/cc-switch/pull/7502) | Claude Desktop custom request headers | Two overlapping PRs; Desktop 3P + proxy overrides. Easy to clobber fork proxy / Pi listen |
| [#7535](https://github.com/farion1231/cc-switch/pull/7535) | Lightweight mode on silent startup | Touches startup / window lifecycle; fork already salvaged SIGTERM / tray Live-restore |
| [#7498](https://github.com/farion1231/cc-switch/pull/7498) / [#7486](https://github.com/farion1231/cc-switch/pull/7486) / [#7481](https://github.com/farion1231/cc-switch/pull/7481) / [#7476](https://github.com/farion1231/cc-switch/pull/7476) | Proxy history / OpenCode Go / image normalize | Hot path next to fork 1214 remap and takeover |
| [#7492](https://github.com/farion1231/cc-switch/pull/7492) / [#7479](https://github.com/farion1231/cc-switch/pull/7479) | Codex auth snapshot / OAuth 归属 | Next to `CodexLiveAuthSwitchGuard` and `proxy_takeover_pi` |
| [#1168](https://github.com/farion1231/cc-switch/pull/1168) | WSL + Windows dual-env sync | Would fight this fork’s WSL UNC / `wsl.localhost` work |

### Skip-schema (unmerged harnesses)

| Official PR | Title | Why |
| --- | --- | --- |
| [#7496](https://github.com/farion1231/cc-switch/pull/7496) | DevEco Code harness | SCHEMA 19 → 20 |
| Anything that extends MiniMax Code (#7383) | — | Already SCHEMA 19 on official `main` |

## What this fork will not do

- Merge official `main` (SCHEMA 19 + MiniMax + no Pi takeover).
- Take official 3.20.3 / later version numbers.
- Cherry-pick unmerged contributor PRs “early”.
- Drop Atlas sponsor slots or rebalance README CTAs.
- Add Linux Claude Desktop 3P until a Linux package is an actual ship target.

## Recheck trigger (Wave F)

Re-run this audit when **any** of these is true:

1. Official `main` SHA ≠ `8272707d`.
2. One of #7543 / #7541 / #7531 (or similar SCHEMA-18 bugfixes) **merges** to official `main`.
3. This fork decides to ship a Linux build (then re-open #7331).

Until then, further “Wave N cherry-pick” runs should expect another docs-only result.
