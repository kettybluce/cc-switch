import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

type TranslationTree = Record<string, unknown>;

function flattenStrings(
  value: unknown,
  path: string[] = [],
  result = new Map<string, string>(),
): Map<string, string> {
  if (typeof value === "string") {
    result.set(path.join("."), value);
  } else if (typeof value === "object" && value !== null) {
    for (const [key, child] of Object.entries(value)) {
      flattenStrings(child, [...path, key], result);
    }
  }
  return result;
}

function interpolationVariables(value: string): string[] {
  return Array.from(
    value.matchAll(/\{\{\s*([^}]+?)\s*\}\}/g),
    ([, name]) => name,
  ).sort();
}

const reference = flattenStrings(en);
const localeTrees = [
  ["en", en],
  ["zh", zh],
  ["ja", ja],
  ["zh-TW", zhTW],
] as const;
const locales = localeTrees.filter(([name]) => name !== "en");

const piKeysOutsideNamespace = new Set([
  "apps.pi",
  "confirm.piDefaultProviderWarning",
  "deeplink.api",
  "notifications.piDefaultProviderSet",
  "notifications.piDefaultProviderSetFailed",
  "sessionManager.piDiscoveryUnavailable",
  "sessionManager.piRelativeSessionDir",
  "settings.browsePlaceholderPi",
  "settings.piConfigDir",
  "settings.piConfigDirDescription",
]);
const piReference = new Map(
  [...reference].filter(
    ([key]) => key.startsWith("pi.") || piKeysOutsideNamespace.has(key),
  ),
);
const claudeReference = new Map(
  [...reference].filter(
    ([key]) =>
      key.startsWith("claudeConfig.") ||
      key.startsWith("claudeCode.") ||
      key.startsWith("claudeDesktop.") ||
      key.startsWith("provider.") ||
      key.startsWith("providerForm."),
  ),
);
const codexReference = new Map(
  [...reference].filter(
    ([key]) =>
      key.startsWith("codex.") ||
      key.startsWith("xaiOauth.") ||
      key.startsWith("managedAuth.") ||
      key === "apps.codex" ||
      key === "usage.appFilter.codex",
  ),
);
const opencodeReference = new Map(
  [...reference].filter(
    ([key]) =>
      key.startsWith("opencode.") ||
      key.startsWith("omo.") ||
      key === "apps.opencode" ||
      key === "usage.appFilter.opencode",
  ),
);
const requiredParityKeys = [
  "settings.oneClickInstall",
  "settings.oneClickInstallHint",
  "providerForm.partnerPromotion.atlascloud",
  "providerForm.presets.doubaoseed",
  "apps.claude",
  "apps.codex",
  "apps.opencode",
  "apps.pi",
  "usage.appFilter.claude",
  "usage.appFilter.codex",
  "usage.appFilter.opencode",
  "usage.appFilter.pi",
] as const;
const schema19MiniMaxKeys = [
  "apps.mcode",
  "mcp.unifiedPanel.apps.mcode",
  "usage.appFilter.mcode",
] as const;
const piProductReferences = new Map(
  [...reference].filter(([, value]) => /\bPi\b/.test(value)),
);

function missingKeys(
  expected: Map<string, string>,
  translations: Map<string, string>,
): string[] {
  return [...expected.keys()].filter((key) => !translations.has(key));
}

function mismatchedInterpolation(
  expected: Map<string, string>,
  translations: Map<string, string>,
): string[] {
  return [...expected].flatMap(([key, value]) => {
    const actual = translations.get(key);
    return actual !== undefined &&
      interpolationVariables(actual).join("\0") !==
        interpolationVariables(value).join("\0")
      ? [key]
      : [];
  });
}

describe("locale coverage", () => {
  it("keeps zh/en/zh-TW/ja key sets identical", () => {
    const flattened = localeTrees.map(
      ([name, tree]) =>
        [name, flattenStrings(tree as TranslationTree)] as const,
    );
    const union = new Set(flattened.flatMap(([, map]) => [...map.keys()]));

    for (const [name, map] of flattened) {
      const missing = [...union].filter((key) => !map.has(key));
      expect(missing, `${name} missing keys`).toEqual([]);
    }
  });

  it.each(localeTrees)(
    "defines every required Pi/Claude/Codex/OpenCode parity key in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      const missing = requiredParityKeys.filter((key) => {
        const value = translations.get(key);
        return typeof value !== "string" || value.trim().length === 0;
      });

      expect(missing).toEqual([]);
    },
  );

  it.each(localeTrees)(
    "does not ship SCHEMA 19 MiniMax Code keys in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      const present = schema19MiniMaxKeys.filter((key) =>
        translations.has(key),
      );

      expect(present).toEqual([]);
    },
  );

  it.each(locales)("covers every Pi translation key in %s", (_name, tree) => {
    const translations = flattenStrings(tree as TranslationTree);
    expect(missingKeys(piReference, translations)).toEqual([]);
  });

  it.each(locales)(
    "covers every Claude translation key in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(missingKeys(claudeReference, translations)).toEqual([]);
    },
  );

  it.each(locales)(
    "covers every Codex translation key in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(missingKeys(codexReference, translations)).toEqual([]);
    },
  );

  it.each(locales)(
    "covers every OpenCode translation key in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(missingKeys(opencodeReference, translations)).toEqual([]);
    },
  );

  it.each(locales)(
    "preserves every Pi interpolation variable in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(mismatchedInterpolation(piReference, translations)).toEqual([]);
    },
  );

  it.each(locales)(
    "preserves every Claude interpolation variable in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(mismatchedInterpolation(claudeReference, translations)).toEqual(
        [],
      );
    },
  );

  it.each(locales)(
    "preserves every Codex interpolation variable in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(mismatchedInterpolation(codexReference, translations)).toEqual([]);
    },
  );

  it.each(locales)(
    "preserves every OpenCode interpolation variable in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      expect(mismatchedInterpolation(opencodeReference, translations)).toEqual(
        [],
      );
    },
  );

  it.each(locales)(
    "preserves explicit Pi product mentions in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      const missingMentions = [...piProductReferences.keys()].filter((key) => {
        const actual = translations.get(key);
        return actual === undefined || !/\bPi\b/.test(actual);
      });

      expect(missingMentions).toEqual([]);
    },
  );

  it("tells users that deleting Pi's default reassigns or clears it", () => {
    expect(en.confirm.piDefaultProviderWarning).toMatch(/reassign/i);
    expect(en.confirm.piDefaultProviderWarning).toMatch(/auth\.json/i);
    expect(zh.confirm.piDefaultProviderWarning).toMatch(/改到另一个/);
    expect(zh.confirm.piDefaultProviderWarning).toMatch(/清除默认/);
    expect(zh.confirm.piDefaultProviderWarning).toMatch(/auth\.json/);
    expect(ja.confirm.piDefaultProviderWarning).toMatch(/付け替える|クリア/);
    expect(zhTW.confirm.piDefaultProviderWarning).toMatch(/改到|清除預設/);
  });
});
