#!/usr/bin/env node
/**
 * Four-locale i18n key parity for CC Switch (zh / zh-TW / en / ja).
 *
 * Default check (Pi / Claude / Codex / OpenCode):
 *   - identical leaf-key sets across the four locales
 *   - interpolation variables match English
 *   - no SCHEMA 19 MiniMax Code (`mcode`) keys
 *   - every literal t()/nameKey/partnerPromotionKey exists in locales
 *   - no unused keys in those app namespaces
 *
 * Usage:
 *   node scripts/check-i18n-parity.mjs
 *   node scripts/check-i18n-parity.mjs --json
 */

import { analyzeParity, formatReport } from "./lib/i18n-parity.mjs";

const json = process.argv.includes("--json");
const result = analyzeParity();

if (json) {
  const {
    usage: _usage,
    maps: _maps,
    ...serializable
  } = result;
  console.log(JSON.stringify(serializable, null, 2));
} else {
  console.log(formatReport(result));
}

process.exit(result.ok ? 0 : 1);
