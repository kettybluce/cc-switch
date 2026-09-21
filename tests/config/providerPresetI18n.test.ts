import { describe, expect, it } from "vitest";
import { providerPresets } from "@/config/claudeProviderPresets";
import { claudeDesktopProviderPresets } from "@/config/claudeDesktopProviderPresets";
import {
  generateThirdPartyConfig,
  codexProviderPresets,
} from "@/config/codexProviderPresets";
import { opencodeProviderPresets } from "@/config/opencodeProviderPresets";
import { piProviderPresets } from "@/config/piProviderPresets";
import { hermesProviderPresets } from "@/config/hermesProviderPresets";
import { openclawProviderPresets } from "@/config/openclawProviderPresets";
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

const locales = [
  ["en", flattenStrings(en as TranslationTree)],
  ["zh", flattenStrings(zh as TranslationTree)],
  ["ja", flattenStrings(ja as TranslationTree)],
  ["zh-TW", flattenStrings(zhTW as TranslationTree)],
] as const;

const appPresets = [
  ["claude", providerPresets],
  ["claude-desktop", claudeDesktopProviderPresets],
  ["codex", codexProviderPresets],
  ["opencode", opencodeProviderPresets],
  ["pi", piProviderPresets],
  ["hermes", hermesProviderPresets],
  ["openclaw", openclawProviderPresets],
] as const;

type NamedPreset = {
  name: string;
  nameKey?: string;
  partnerPromotionKey?: string;
  isPartner?: boolean;
};

function collectNameKeys(presets: readonly NamedPreset[]): string[] {
  return [
    ...new Set(
      presets.flatMap((preset) => (preset.nameKey ? [preset.nameKey] : [])),
    ),
  ];
}

function collectPromoKeys(presets: readonly NamedPreset[]): string[] {
  return [
    ...new Set(
      presets.flatMap((preset) =>
        preset.partnerPromotionKey ? [preset.partnerPromotionKey] : [],
      ),
    ),
  ];
}

/**
 * Live `~/.pi/agent/models.json` node keys already shipped by this fork.
 * ALIGN = APPEND only: new official twins may be added, existing keys must
 * never be renamed (that would orphan or rewrite live models.json nodes).
 */
const LIVE_PI_PROVIDER_KEYS = [
  "cc-switch-kimi",
  "cc-switch-kimi-global",
  "cc-switch-kimi-for-coding",
  "cc-switch-kimi-for-coding-global",
  "cc-switch-packy-code",
  "cc-switch-zeta-api",
  "cc-switch-apinebula",
  "cc-switch-aicode-mirror",
  "cc-switch-fenno-ai",
  "cc-switch-run-api",
  "cc-switch-shengsuanyun",
  "cc-switch-aigo-code",
  "cc-switch-qiniu",
  "cc-switch-aicoding",
  "cc-switch-sub-router",
  "cc-switch-apikey-fun",
  "cc-switch-9527-code",
  "cc-switch-code0",
  "cc-switch-teamo-router",
  "cc-switch-ppio",
  "cc-switch-claude-cn",
  "cc-switch-agentplan",
  "cc-switch-byte-plus",
  "cc-switch-dou-bao-seed",
  "cc-switch-a6-api",
  "cc-switch-atlas-cloud",
  "cc-switch-ccsub",
  "cc-switch-sssai-code",
  "cc-switch-sole-api",
  "cc-switch-micu",
  "cc-switch-right-code",
  "cc-switch-etok-ai",
  "cc-switch-cubence",
  "cc-switch-crazy-router",
  "cc-switch-dmxapi",
  "cc-switch-sudo-code-chat",
  "cc-switch-sudo-code-us",
  "cc-switch-amux",
  "cc-switch-deep-seek",
  "cc-switch-zhipu-glm",
  "cc-switch-zhipu-glm-en",
  "cc-switch-qianwenai",
  "cc-switch-qianwenai-token-plan",
  "cc-switch-qwencloud",
  "cc-switch-qwencloud-coding",
  "cc-switch-qwencloud-token-plan",
  "cc-switch-step-fun",
  "cc-switch-step-fun-en",
  "cc-switch-step-fun-step-plan",
  "cc-switch-model-scope",
  "cc-switch-kat-coder",
  "cc-switch-longcat",
  "cc-switch-mini-max",
  "cc-switch-mini-max-en",
  "cc-switch-bai-ling",
  "cc-switch-xiaomi-mi-mo",
  "cc-switch-xiaomi-mi-mo-token-plan-china",
  "cc-switch-open-code-go",
  "cc-switch-ai-hub-mix",
  "cc-switch-cherry-in",
  "cc-switch-open-router",
  "cc-switch-the-router",
  "cc-switch-novita-ai",
  "cc-switch-nvidia",
  "cc-switch-pipellm",
  "cc-switch-aicode-with",
  "cc-switch-e-flow-code",
  "cc-switch-aws-bedrock",
  "cc-switch-tencent-token-plan",
  "cc-switch-tencent-token-plan-intl",
  "cc-switch-tencent-token-plan-enterprise-pro",
  "cc-switch-tencent-token-plan-enterprise-pro-intl",
  "cc-switch-tencent-token-plan-enterprise-lite",
  "cc-switch-tencent-token-plan-enterprise-lite-intl",
  "cc-switch-tencent-tokenhub",
  "cc-switch-tencent-tokenhub-intl",
] as const;

const OFFICIAL_APPEND_PRESET_NAMES = [
  "Kimi Global",
  "Kimi For Coding Global",
] as const;

describe("provider preset i18n and live keys", () => {
  it.each(locales)(
    "defines every preset nameKey in %s",
    (_name, translations) => {
      const missing = appPresets.flatMap(([app, presets]) =>
        collectNameKeys(presets as NamedPreset[]).flatMap((key) =>
          translations.has(key) ? [] : [`${app}:${key}`],
        ),
      );

      expect(missing).toEqual([]);
    },
  );

  it.each(locales)(
    "defines every partnerPromotionKey in %s",
    (_name, translations) => {
      const missing = appPresets.flatMap(([app, presets]) =>
        collectPromoKeys(presets as NamedPreset[]).flatMap((key) => {
          const path = `providerForm.partnerPromotion.${key}`;
          const value = translations.get(path);
          return typeof value === "string" && value.trim().length > 0
            ? []
            : [`${app}:${path}`];
        }),
      );

      expect(missing).toEqual([]);
    },
  );

  it("keeps Pi providerKeys unique, prefixed, and APPEND-only for live models.json", () => {
    const keys = piProviderPresets.map((preset) => preset.providerKey);

    expect(new Set(keys).size).toBe(keys.length);
    expect(keys.every((key) => key.startsWith("cc-switch-"))).toBe(true);
    expect(keys.length).toBeGreaterThanOrEqual(LIVE_PI_PROVIDER_KEYS.length);

    const missing = LIVE_PI_PROVIDER_KEYS.filter((key) => !keys.includes(key));
    expect(missing).toEqual([]);
  });

  it("never rewrites the Volcengine Doubao live models.json key", () => {
    const piPreset = piProviderPresets.find(
      (preset) => preset.name === "Volcengine Doubao",
    );

    expect(piPreset?.providerKey).toBe("cc-switch-dou-bao-seed");
    expect(piPreset?.nameKey).toBe("providerForm.presets.doubaoseed");
    expect(piPreset?.settingsConfig.name).toBe("Volcengine Doubao");
  });

  it("ships official SCHEMA-18-safe Kimi Global twins without colliding live keys", () => {
    for (const [app, presets] of appPresets) {
      if (app === "hermes" || app === "openclaw" || app === "claude-desktop") {
        const names = (presets as NamedPreset[]).map((preset) => preset.name);
        if (!names.includes("Kimi")) continue;
      }

      const names = new Set(
        (presets as NamedPreset[]).map((preset) => preset.name),
      );
      for (const name of OFFICIAL_APPEND_PRESET_NAMES) {
        expect(names.has(name), `${app} missing ${name}`).toBe(true);
      }
    }

    const kimi = piProviderPresets.find((preset) => preset.name === "Kimi");
    const kimiGlobal = piProviderPresets.find(
      (preset) => preset.name === "Kimi Global",
    );
    const kimiCoding = piProviderPresets.find(
      (preset) => preset.name === "Kimi For Coding",
    );
    const kimiCodingGlobal = piProviderPresets.find(
      (preset) => preset.name === "Kimi For Coding Global",
    );

    expect(kimi?.providerKey).toBe("cc-switch-kimi");
    expect(kimiGlobal?.providerKey).toBe("cc-switch-kimi-global");
    expect(kimiCoding?.providerKey).toBe("cc-switch-kimi-for-coding");
    expect(kimiCodingGlobal?.providerKey).toBe(
      "cc-switch-kimi-for-coding-global",
    );
  });

  it("keeps Atlas Cloud sponsor copy on the fork without renaming its Pi live key", () => {
    const atlasPresets = [
      providerPresets.find((preset) => preset.name === "AtlasCloud"),
      codexProviderPresets.find((preset) => preset.name === "AtlasCloud"),
      opencodeProviderPresets.find((preset) => preset.name === "AtlasCloud"),
      piProviderPresets.find((preset) => preset.name === "AtlasCloud"),
    ];

    for (const preset of atlasPresets) {
      expect(preset).toBeDefined();
      expect(preset?.isPartner).toBe(true);
      expect(preset?.partnerPromotionKey).toBe("atlascloud");
    }

    expect(
      piProviderPresets.find((preset) => preset.name === "AtlasCloud")
        ?.providerKey,
    ).toBe("cc-switch-atlas-cloud");
  });

  it("keeps the default Cursor pool Codex model on third-party templates", () => {
    expect(generateThirdPartyConfig("custom", "https://example.invalid")).toMatch(
      /^model = "gpt-5.6-sol"$/m,
    );
  });
});
