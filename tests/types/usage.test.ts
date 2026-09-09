import {
  appHasUsageDashboard,
  getCacheWriteAvailability,
  isSessionUsageSource,
} from "@/types/usage";

describe("getCacheWriteAvailability", () => {
  it("distinguishes cache-write support across fixed protocols", () => {
    expect(getCacheWriteAvailability(["claude"])).toBe("ok");
    expect(getCacheWriteAvailability(["pi"])).toBe("partial");
    expect(getCacheWriteAvailability(["codex", "gemini"])).toBe("na");
    expect(getCacheWriteAvailability(["claude", "codex"])).toBe("partial");
    expect(getCacheWriteAvailability([])).toBe("ok");
  });
});

describe("appHasUsageDashboard", () => {
  it("includes Pi and Claude without a takeover flag", () => {
    expect(appHasUsageDashboard("pi")).toBe(true);
    expect(appHasUsageDashboard("claude")).toBe(true);
    expect(appHasUsageDashboard("openclaw")).toBe(false);
  });
});

describe("isSessionUsageSource", () => {
  it("recognizes Claude session_log and * _session importers", () => {
    expect(isSessionUsageSource("pi_session")).toBe(true);
    expect(isSessionUsageSource("session_log")).toBe(true);
    expect(isSessionUsageSource("codex_session")).toBe(true);
    expect(isSessionUsageSource("proxy")).toBe(false);
    expect(isSessionUsageSource(undefined)).toBe(false);
  });
});

describe("getCacheWriteAvailability", () => {
  it("distinguishes cache-write support across fixed protocols", () => {
    expect(getCacheWriteAvailability(["claude"])).toBe("ok");
    expect(getCacheWriteAvailability(["pi"])).toBe("partial");
    expect(getCacheWriteAvailability(["codex", "gemini"])).toBe("na");
    expect(getCacheWriteAvailability(["claude", "codex"])).toBe("partial");
    expect(getCacheWriteAvailability([])).toBe("ok");
  });
});
