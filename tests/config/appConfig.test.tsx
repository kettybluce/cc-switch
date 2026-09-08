import { describe, expect, it } from "vitest";
import { isAdditiveAppId, isProxyAppId, isTakeoverAppId, PROXY_APP_IDS, TAKEOVER_APP_IDS } from "@/config/appConfig";

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

  it("includes Pi in takeover apps alongside Claude and Codex", () => {
    expect(TAKEOVER_APP_IDS).toEqual([
      "claude",
      "codex",
      "gemini",
      "grokbuild",
      "pi",
    ]);
    expect(isTakeoverAppId("pi")).toBe(true);
    expect(isTakeoverAppId("claude")).toBe(true);
    expect(isTakeoverAppId("opencode")).toBe(false);
  });
});
