import { describe, expect, it } from "vitest";
import { isAdditiveAppId, isProxyAppId, PROXY_APP_IDS } from "@/config/appConfig";

describe("appConfig provider lifecycle", () => {
  it.each(["opencode", "openclaw", "hermes", "pi"])(
    "classifies %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(true);
    },
  );

  it.each(["claude", "claude-desktop", "codex", "gemini", "grokbuild"])(
    "does not classify %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(false);
    },
  );

  it("keeps Pi out of the local-gateway failover apps", () => {
    expect(PROXY_APP_IDS).toEqual(["claude", "codex", "gemini", "grokbuild"]);
    expect(isProxyAppId("pi")).toBe(false);
    expect(isProxyAppId("claude")).toBe(true);
  });
});
