import { describe, expect, it } from "vitest";
import {
  formatOutputTokensPerSecond,
  formatRequestFirstToken,
  formatRequestLatency,
  formatRequestTiming,
  formatTokensShort,
  getOutputTokensPerSecond,
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

  it("calculates streaming TPS from generation duration after first token", () => {
    expect(
      getOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 10_000,
        firstTokenMs: 4_000,
      }),
    ).toBe(20);
  });

  it("prefers explicit durationMs for output TPS", () => {
    expect(
      getOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 10_000,
        firstTokenMs: 4_000,
        durationMs: 3_000,
      }),
    ).toBe(40);
  });

  it("falls back to full latency when first token timing is missing", () => {
    expect(
      getOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 10_000,
      }),
    ).toBe(12);
  });

  it("does not show TPS without positive tokens or duration", () => {
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 0,
        latencyMs: 10_000,
      }),
    ).toBeNull();
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 4_000,
        firstTokenMs: 4_000,
      }),
    ).toBeNull();
  });

  it("does not invent TPS from session JSONL rows with stored latency 0", () => {
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 0,
      }),
    ).toBeNull();
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 120,
        latencyMs: 0,
        firstTokenMs: 0,
      }),
    ).toBeNull();
  });

  it("formats TPS with integer or single-decimal precision", () => {
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 121,
        latencyMs: 10_000,
      }),
    ).toBe("12");
    expect(
      formatOutputTokensPerSecond({
        outputTokens: 1,
        latencyMs: 4_000,
      }),
    ).toBe("0.3");
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

  it("maps SCHEMA 18 Claude/Codex/Pi session imports (latency 0, no TTFT) to a dash", () => {
    for (const source of [
      "session_log",
      "codex_session",
      "pi_session",
    ] as const) {
      expect(formatRequestTiming(0, null, source)).toBe(UNKNOWN_REQUEST_TIMING);
      expect(formatRequestTiming(0, undefined, source)).toBe(
        UNKNOWN_REQUEST_TIMING,
      );
      expect(formatRequestTiming(0, 0, source)).toBe(UNKNOWN_REQUEST_TIMING);
    }
  });

  it("keeps real proxy and session timings that are actually present", () => {
    expect(formatRequestTiming(1234, 200, "proxy")).toBe("1.2s/0.2s");
    expect(formatRequestTiming(0, null, "proxy")).toBe("0.0s");
    expect(formatRequestTiming(1500, null, "grok_session")).toBe("1.5s");
  });

  it("omits a TTFT fragment when session JSONL stored 0 or null", () => {
    expect(formatRequestFirstToken(null, "pi_session")).toBeNull();
    expect(formatRequestFirstToken(0, "session_log")).toBeNull();
    expect(formatRequestFirstToken(0, "codex_session")).toBeNull();
    expect(formatRequestLatency(0, "pi_session")).toBe(UNKNOWN_REQUEST_TIMING);
    expect(formatRequestLatency(0, "session_log")).toBe(UNKNOWN_REQUEST_TIMING);
    expect(formatRequestLatency(0, "codex_session")).toBe(
      UNKNOWN_REQUEST_TIMING,
    );
  });
});
