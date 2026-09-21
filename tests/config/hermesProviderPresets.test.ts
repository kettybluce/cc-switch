import { describe, expect, it } from "vitest";
import {
  HERMES_DEFAULT_API_MODE,
  hermesApiModes,
  hermesProviderPresets,
} from "@/config/hermesProviderPresets";

const VALID_API_MODES = new Set(hermesApiModes.map((mode) => mode.value));

describe("Hermes provider presets (offline smoke)", () => {
  it("owns a non-empty catalog with unique names", () => {
    const names = hermesProviderPresets.map((preset) => preset.name);
    expect(hermesProviderPresets.length).toBeGreaterThan(0);
    expect(new Set(names).size).toBe(names.length);
  });

  it("ships static settings only — no secrets, no live gateway URLs to fetch", () => {
    expect(HERMES_DEFAULT_API_MODE).toBe("chat_completions");

    for (const preset of hermesProviderPresets) {
      expect(preset.name.length).toBeGreaterThan(0);
      expect(preset.websiteUrl).toMatch(/^https:\/\//);
      expect(preset.settingsConfig.name.length).toBeGreaterThan(0);
      if (preset.settingsConfig.api_key !== undefined) {
        expect(preset.settingsConfig.api_key).toBe("");
      }
      if (preset.settingsConfig.api_mode) {
        expect(VALID_API_MODES.has(preset.settingsConfig.api_mode)).toBe(true);
      }
      if (preset.settingsConfig.base_url) {
        expect(preset.settingsConfig.base_url).toMatch(/^https?:\/\//);
        expect(preset.settingsConfig.base_url).not.toContain("15721");
      }
      if (preset.settingsConfig.models) {
        const ids = preset.settingsConfig.models.map((model) => model.id);
        expect(ids.every((id) => id.length > 0)).toBe(true);
        expect(new Set(ids).size).toBe(ids.length);
      }
    }
  });
});
