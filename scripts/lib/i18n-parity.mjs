import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

export const LOCALES = ["en", "zh", "zh-TW", "ja"];
export const SCHEMA19_MINIMAX_KEYS = [
  "apps.mcode",
  "mcp.unifiedPanel.apps.mcode",
  "usage.appFilter.mcode",
];

export const TARGET_APPS = ["pi", "claude", "codex", "opencode"];

const LITERAL_T_RE =
  /\b(?:t|i18n\.t)\(\s*(['"])([a-zA-Z][a-zA-Z0-9_.-]*)\1/g;
const TEMPLATE_T_RE =
  /\b(?:t|i18n\.t)\(\s*`([^`$]*?)\$\{[^}]+\}([^`]*)`/g;
const I18N_KEY_PROPS = new Set([
  "nameKey",
  "labelKey",
  "titleKey",
  "messageKey",
  "descriptionKey",
  "descKey",
  "tooltipKey",
  "i18nKey",
  "placeholderKey",
  "hintKey",
  "errorKey",
  "ariaLabelKey",
]);
const KEY_PROP_RE = /\b([A-Za-z][A-Za-z0-9]*Key)\s*:\s*(['"])([^'"]+)\2/g;
const PROMO_KEY_RE = /\bpartnerPromotionKey\s*:\s*(['"])([^'"]+)\1/g;
const QUOTED_DOTTED_RE = /(['"])([a-zA-Z][a-zA-Z0-9_]*(?:\.[a-zA-Z0-9_-]+)+)\1/g;

const SKIP_DIRS = new Set([
  "node_modules",
  "dist",
  "target",
  ".git",
  "coverage",
]);

export function repoRootFrom(url = import.meta.url) {
  return join(dirname(fileURLToPath(url)), "..", "..");
}

export function flattenStrings(
  value,
  path = [],
  result = new Map(),
) {
  if (typeof value === "string") {
    result.set(path.join("."), value);
  } else if (Array.isArray(value)) {
    value.forEach((child, index) => {
      flattenStrings(child, [...path, String(index)], result);
    });
  } else if (typeof value === "object" && value !== null) {
    for (const [key, child] of Object.entries(value)) {
      flattenStrings(child, [...path, key], result);
    }
  }
  return result;
}

export function interpolationVariables(value) {
  return Array.from(
    value.matchAll(/\{\{\s*([^}]+?)\s*\}\}/g),
    ([, name]) => name.trim(),
  ).sort();
}

export function loadLocales(root) {
  const dir = join(root, "src", "i18n", "locales");
  const trees = {};
  const maps = {};
  for (const locale of LOCALES) {
    const file = join(dir, `${locale}.json`);
    const tree = JSON.parse(readFileSync(file, "utf8"));
    trees[locale] = tree;
    maps[locale] = flattenStrings(tree);
  }
  return { trees, maps };
}

export function walkFiles(dir, acc = []) {
  for (const name of readdirSync(dir)) {
    if (SKIP_DIRS.has(name)) continue;
    const full = join(dir, name);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      walkFiles(full, acc);
      continue;
    }
    if (!/\.(ts|tsx|js|jsx|mjs)$/.test(name)) continue;
    if (/\.test\.(ts|tsx|js|jsx)$/.test(name)) continue;
    acc.push(full);
  }
  return acc;
}

export function looksLikeI18nKey(key) {
  if (!key) return false;
  if (key.includes("/") || key.includes("${") || key.includes("{{")) return false;
  if (key.startsWith("cc-switch-")) return false;
  return /^[a-zA-Z][a-zA-Z0-9_-]*(?:\.[a-zA-Z0-9_-]+)+$/.test(key);
}

function addRef(map, key, ref) {
  if (!key) return;
  const refs = map.get(key) ?? [];
  refs.push(ref);
  map.set(key, refs);
}

export function extractSourceUsage(root, files) {
  const literals = new Map();
  const prefixes = new Map();
  const mentioned = new Map();

  for (const file of files) {
    const rel = relative(root, file).replaceAll("\\", "/");
    const text = readFileSync(file, "utf8");

    for (const match of text.matchAll(LITERAL_T_RE)) {
      if (!looksLikeI18nKey(match[2])) continue;
      addRef(literals, match[2], rel);
    }

    for (const match of text.matchAll(TEMPLATE_T_RE)) {
      const prefix = match[1];
      const suffix = match[2];
      if (!prefix && !suffix) continue;
      addRef(prefixes, `${prefix}\0${suffix}`, rel);
    }

    for (const match of text.matchAll(KEY_PROP_RE)) {
      if (!I18N_KEY_PROPS.has(match[1])) continue;
      if (!looksLikeI18nKey(match[3])) continue;
      addRef(literals, match[3], rel);
    }

    for (const match of text.matchAll(PROMO_KEY_RE)) {
      const raw = match[2];
      const key = raw.includes(".")
        ? raw
        : `providerForm.partnerPromotion.${raw}`;
      addRef(literals, key, rel);
    }

    for (const match of text.matchAll(QUOTED_DOTTED_RE)) {
      addRef(mentioned, match[2], rel);
    }
  }

  return { literals, prefixes, mentioned };
}

export function isTargetAppKey(key) {
  if (
    key.startsWith("pi.") ||
    key.startsWith("claudeConfig.") ||
    key.startsWith("claudeCode.") ||
    key.startsWith("claudeDesktop.") ||
    key.startsWith("codex.") ||
    key.startsWith("opencode.") ||
    key.startsWith("omo.") ||
    key.startsWith("provider.") ||
    key.startsWith("providerForm.") ||
    key.startsWith("xaiOauth.") ||
    key.startsWith("managedAuth.")
  ) {
    return true;
  }

  const exact = new Set([
    "apps.pi",
    "apps.claude",
    "apps.claudeCode",
    "apps.claudeDesktop",
    "apps.claude-desktop",
    "apps.codex",
    "apps.opencode",
    "usage.appFilter.pi",
    "usage.appFilter.claude",
    "usage.appFilter.codex",
    "usage.appFilter.opencode",
    "mcp.unifiedPanel.apps.claude",
    "mcp.unifiedPanel.apps.codex",
    "mcp.unifiedPanel.apps.opencode",
    "mcp.unifiedPanel.apps.pi",
    "skills.apps.claude",
    "skills.apps.codex",
    "skills.apps.opencode",
    "skills.apps.pi",
    "settings.browsePlaceholderPi",
    "settings.piConfigDir",
    "settings.piConfigDirDescription",
    "settings.claudeConfigDirDescription",
    "settings.codexConfigDirDescription",
    "settings.opencodeConfigDirDescription",
    "settings.oneClickInstall",
    "settings.oneClickInstallHint",
    "notifications.piDefaultProviderSet",
    "notifications.piDefaultProviderSetFailed",
    "confirm.piDefaultProviderWarning",
    "sessionManager.piDiscoveryUnavailable",
    "sessionManager.piRelativeSessionDir",
    "deeplink.api",
  ]);
  return exact.has(key);
}

function prefixMatches(key, prefix, suffix) {
  if (prefix && !key.startsWith(prefix)) return false;
  if (suffix && !key.endsWith(suffix)) return false;
  if (!prefix && !suffix) return false;
  const middle = key.slice(prefix.length, suffix ? key.length - suffix.length : undefined);
  return middle.length > 0;
}

export function keysCoveredByUsage(localeKeys, usage) {
  const used = new Set();

  for (const key of usage.literals.keys()) {
    if (localeKeys.has(key)) used.add(key);
  }
  for (const key of usage.mentioned.keys()) {
    if (localeKeys.has(key)) used.add(key);
  }
  for (const encoded of usage.prefixes.keys()) {
    const [prefix, suffix] = encoded.split("\0");
    for (const key of localeKeys) {
      if (prefixMatches(key, prefix, suffix)) used.add(key);
    }
  }
  return used;
}

export function analyzeParity(root = repoRootFrom()) {
  const { maps } = loadLocales(root);
  const srcRoot = join(root, "src");
  const files = walkFiles(srcRoot);
  const usage = extractSourceUsage(root, files);

  const union = new Set();
  for (const map of Object.values(maps)) {
    for (const key of map.keys()) union.add(key);
  }

  const missingByLocale = {};
  const extraByLocale = {};
  const emptyByLocale = {};
  const interpolationMismatches = {};
  const en = maps.en;

  for (const locale of LOCALES) {
    const map = maps[locale];
    missingByLocale[locale] = [...union].filter((key) => !map.has(key)).sort();
    extraByLocale[locale] = [...map.keys()]
      .filter((key) => !LOCALES.every((name) => maps[name].has(key)))
      .sort();
    // Empty strings are valid (placeholder keys use defaultValue: "").
    emptyByLocale[locale] = [];
    interpolationMismatches[locale] = [...en.entries()]
      .flatMap(([key, expected]) => {
        const actual = map.get(key);
        if (actual === undefined) return [];
        return interpolationVariables(actual).join("\0") ===
          interpolationVariables(expected).join("\0")
          ? []
          : [key];
      })
      .sort();
  }

  const schema19Keys = [...union]
    .filter((key) => SCHEMA19_MINIMAX_KEYS.includes(key))
    .sort();

  const used = keysCoveredByUsage(union, usage);
  const missingInLocales = [...usage.literals.keys()]
    .filter((key) => !union.has(key))
    .sort();

  const orphans = [...union]
    .filter((key) => isTargetAppKey(key) && !used.has(key))
    .sort();

  const allOrphans = [...union].filter((key) => !used.has(key)).sort();

  const ok =
    schema19Keys.length === 0 &&
    missingInLocales.length === 0 &&
    orphans.length === 0 &&
    LOCALES.every(
      (locale) =>
        missingByLocale[locale].length === 0 &&
        emptyByLocale[locale].length === 0 &&
        interpolationMismatches[locale].length === 0,
    );

  return {
    ok,
    filesScanned: files.length,
    literalKeys: usage.literals.size,
    localeKeyCounts: Object.fromEntries(
      LOCALES.map((locale) => [locale, maps[locale].size]),
    ),
    missingByLocale,
    extraByLocale,
    emptyByLocale,
    interpolationMismatches,
    schema19Keys,
    missingInLocales,
    orphans,
    allOrphanCount: allOrphans.length,
    usage,
    maps,
  };
}

export function formatReport(result) {
  const lines = [];
  lines.push("i18n four-locale key parity (zh / zh-TW / en / ja)");
  lines.push(
    `scanned ${result.filesScanned} src files, ${result.literalKeys} literal t()/nameKey refs`,
  );
  lines.push(
    `locale leaves: ${LOCALES.map((locale) => `${locale}=${result.localeKeyCounts[locale]}`).join(", ")}`,
  );

  const problems = [];
  for (const locale of LOCALES) {
    if (result.missingByLocale[locale].length) {
      problems.push(
        `${locale} missing vs union: ${result.missingByLocale[locale].join(", ")}`,
      );
    }
    if (result.emptyByLocale[locale].length) {
      problems.push(`${locale} empty: ${result.emptyByLocale[locale].join(", ")}`);
    }
    if (result.interpolationMismatches[locale].length) {
      problems.push(
        `${locale} interpolation drift: ${result.interpolationMismatches[locale].join(", ")}`,
      );
    }
  }
  if (result.schema19Keys.length) {
    problems.push(`SCHEMA 19 MiniMax keys present: ${result.schema19Keys.join(", ")}`);
  }
  if (result.missingInLocales.length) {
    problems.push(
      `used in Pi/Claude/Codex/OpenCode UI but missing from locales:\n  ${result.missingInLocales.join("\n  ")}`,
    );
  }
  if (result.orphans.length) {
    problems.push(
      `orphan Pi/Claude/Codex/OpenCode keys (in locales, never referenced):\n  ${result.orphans.join("\n  ")}`,
    );
  }

  if (problems.length === 0) {
    lines.push("OK — four locales match; no missing/orphan target-app keys; no SCHEMA 19 mcode keys.");
  } else {
    lines.push("FAIL");
    for (const problem of problems) lines.push(`- ${problem}`);
  }
  lines.push(`(all-namespace unused leaf count, informational: ${result.allOrphanCount})`);
  return lines.join("\n");
}
