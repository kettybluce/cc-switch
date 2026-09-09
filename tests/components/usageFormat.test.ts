import { describe, expect, it } from "vitest";
import {
  formatRequestTiming,
  formatTokensShort,
  getLocaleFromLanguage,
  UNKNOWN_REQUEST_TIMING,
} from "@/components/usage/format";

describe("usage format helpers", () => {
  it("formats Traditional Chinese token units with Traditional characters", () => {
    expect(formatTokensShort(12_345, "zh-TW")).toBe("1.2 萬");
    expect(formatTokensShort(123_456_789, "zh-Hant", 2)).toBe("1.23 億");
  });

  it("resolves Traditional Chinese locale aliases", () => {
    expect(getLocaleFromLanguage("zh_TW")).toBe("zh-TW");
    expect(getLocaleFromLanguage("zh-HK")).toBe("zh-TW");
  });
});

describe("formatRequestTiming", () => {
  it("shows an em dash for Pi/Claude session rows with stored latency 0", () => {
    expect(formatRequestTiming(0, null, "pi_session")).toBe(
      UNKNOWN_REQUEST_TIMING,
    );
    expect(formatRequestTiming(0, undefined, "session_log")).toBe(
      UNKNOWN_REQUEST_TIMING,
    );
    expect(formatRequestTiming(0, 0, "pi_session")).toBe(
      UNKNOWN_REQUEST_TIMING,
    );
  });

  it("does not render a fake 0.0s TTFT for session rows", () => {
    expect(formatRequestTiming(0, 0, "codex_session")).toBe(
      UNKNOWN_REQUEST_TIMING,
    );
    expect(formatRequestTiming(1500, 0, "grok_session")).toBe("1.5s");
  });

  it("keeps real proxy and session timings that are actually present", () => {
    expect(formatRequestTiming(1234, 200, "proxy")).toBe("1.2s/0.2s");
    expect(formatRequestTiming(0, null, "proxy")).toBe("0.0s");
    expect(formatRequestTiming(1500, null, "grok_session")).toBe("1.5s");
  });
});
