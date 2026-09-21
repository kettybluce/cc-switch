import { describe, expect, it } from "vitest";
import { hermesProviderPresets } from "@/config/hermesProviderPresets";
import { openclawProviderPresets } from "@/config/openclawProviderPresets";
import { opencodeProviderPresets } from "@/config/opencodeProviderPresets";

/**
 * Offline catalog smoke for the three additive apps. These presets are the
 * UI CRUD starting point; they must not require network and must not encode
 * a local-proxy listen (SCHEMA 18 has no proxy_config row for these apps).
 */
describe("OpenCode / Hermes / OpenClaw preset catalogs (offline smoke)", () => {
  it("keeps unique preset names in each catalog", () => {
    for (const [label, names] of [
      ["OpenCode", opencodeProviderPresets.map((preset) => preset.name)],
      ["Hermes", hermesProviderPresets.map((preset) => preset.name)],
      ["OpenClaw", openclawProviderPresets.map((preset) => preset.name)],
    ] as const) {
      expect(names.length, `${label} catalog`).toBeGreaterThan(0);
      expect(new Set(names).size, `${label} duplicate names`).toBe(names.length);
    }
  });

  it("OpenCode non-OMO presets expose npm + https options.baseURL without listen 15721", () => {
    const regular = opencodeProviderPresets.filter(
      (preset) => preset.category !== "omo" && preset.category !== "omo-slim",
    );
    expect(regular.length).toBeGreaterThan(0);

    for (const preset of regular) {
      expect(preset.settingsConfig.npm.length).toBeGreaterThan(0);
      const baseURL = preset.settingsConfig.options?.baseURL;
      if (typeof baseURL === "string" && baseURL.length > 0) {
        expect(baseURL).toMatch(/^https?:\/\//);
        expect(baseURL).not.toContain("15721");
      }
      const apiKey = preset.settingsConfig.options?.apiKey;
      if (typeof apiKey === "string") {
        expect(apiKey).toBe("");
      }
    }
  });

  it("OpenClaw presets expose api + models and never point at Claude listen", () => {
    for (const preset of openclawProviderPresets) {
      expect(preset.settingsConfig.api).toBeTruthy();
      const models = preset.settingsConfig.models ?? [];
      expect(models.length).toBeGreaterThan(0);
      const modelIds = models.map((model) => model.id);
      expect(new Set(modelIds).size).toBe(modelIds.length);
      if (preset.settingsConfig.baseUrl) {
        expect(preset.settingsConfig.baseUrl).toMatch(/^https?:\/\//);
        expect(preset.settingsConfig.baseUrl).not.toContain("15721");
      }
      if (preset.settingsConfig.apiKey !== undefined) {
        expect(preset.settingsConfig.apiKey).toBe("");
      }
    }
  });

  it("OMO templates stay exclusive-mode stubs (empty npm, not live-projected here)", () => {
    for (const name of ["Oh My OpenCode", "Oh My OpenCode Slim"]) {
      const preset = opencodeProviderPresets.find((item) => item.name === name);
      expect(preset, name).toBeDefined();
      expect(preset!.settingsConfig.npm).toBe("");
      expect(preset!.isCustomTemplate).toBe(true);
    }
  });
});
