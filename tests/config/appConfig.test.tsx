import { describe, expect, it } from "vitest";
import {
  isAdditiveAppId,
  isProxyAppId,
  isTakeoverAppId,
} from "@/config/appConfig";

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
});

describe("appConfig proxy takeover", () => {
  it("keeps Pi out of the failover proxy data plane", () => {
    expect(isProxyAppId("pi")).toBe(false);
    expect(isProxyAppId("claude")).toBe(true);
  });

  it("lets Pi reuse Claude-style takeover without failover", () => {
    expect(isTakeoverAppId("pi")).toBe(true);
    expect(isTakeoverAppId("claude")).toBe(true);
    expect(isTakeoverAppId("hermes")).toBe(false);
  });
});
