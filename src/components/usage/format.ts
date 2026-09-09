import { isSessionUsageSource } from "@/types/usage";

/** Shown when session JSONL has no latency/TTFT (do not render a fake 0.0s). */
export const UNKNOWN_REQUEST_TIMING = "—";

export function parseFiniteNumber(value: unknown): number | null {
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : null;
  }

  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    return Number.isFinite(parsed) ? parsed : null;
  }

  return null;
}

export function fmtInt(
  value: unknown,
  locale?: string,
  fallback: string = "--",
): string {
  const num = parseFiniteNumber(value);
  if (num == null) return fallback;
  return new Intl.NumberFormat(locale).format(Math.trunc(num));
}

export function fmtUsd(
  value: unknown,
  digits: number,
  fallback: string = "--",
): string {
  const num = parseFiniteNumber(value);
  if (num == null) return fallback;
  return `$${num.toFixed(digits)}`;
}

function knownPositiveMs(value: unknown): number | null {
  const num = parseFiniteNumber(value);
  if (num == null || num <= 0) return null;
  return num;
}

function formatSeconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

function resolveTimingMs(
  value: unknown,
  dataSource?: string | null,
): number | null {
  return isSessionUsageSource(dataSource)
    ? knownPositiveMs(value)
    : parseFiniteNumber(value);
}

/** Latency for the request-log 用时 column. Session JSONL has none → em dash. */
export function formatRequestLatency(
  latencyMs: unknown,
  dataSource?: string | null,
  unknownLabel: string = UNKNOWN_REQUEST_TIMING,
): string {
  const latency = resolveTimingMs(latencyMs, dataSource);
  return latency == null ? unknownLabel : formatSeconds(latency);
}

/**
 * TTFT fragment without a leading slash, or null when the field is absent /
 * unknown. Session `0`/`null` must not render as `/0.0s`.
 */
export function formatRequestFirstToken(
  firstTokenMs: unknown,
  dataSource?: string | null,
): string | null {
  if (firstTokenMs == null) return null;
  const ttft = resolveTimingMs(firstTokenMs, dataSource);
  return ttft == null ? null : formatSeconds(ttft);
}

/**
 * Format the request-log 用时/首字 cell.
 *
 * Session imports (Pi / Claude / Codex / …) have no timing fields in JSONL.
 * Stored `latency_ms=0` (SCHEMA 18 NOT NULL) must display as an em dash, not
 * `0.0s`. Do not invent latency from timestamps.
 */
export function formatRequestTiming(
  latencyMs: unknown,
  firstTokenMs?: unknown | null,
  dataSource?: string | null,
  unknownLabel: string = UNKNOWN_REQUEST_TIMING,
): string {
  const latencyLabel = formatRequestLatency(
    latencyMs,
    dataSource,
    unknownLabel,
  );
  const ttftLabel = formatRequestFirstToken(firstTokenMs, dataSource);
  return ttftLabel == null ? latencyLabel : `${latencyLabel}/${ttftLabel}`;
}

function normalizeLanguageTag(language: string): string {
  return language.toLowerCase().replace(/_/g, "-");
}

function isTraditionalChineseLanguage(normalizedLanguage: string): boolean {
  return (
    normalizedLanguage === "zh-tw" ||
    normalizedLanguage.startsWith("zh-hant") ||
    normalizedLanguage.startsWith("zh-hk") ||
    normalizedLanguage.startsWith("zh-mo")
  );
}

export function getLocaleFromLanguage(language: string): string {
  if (!language) return "en-US";
  const normalized = normalizeLanguageTag(language);
  if (normalized === "zh") return "zh-CN";
  if (isTraditionalChineseLanguage(normalized)) {
    return "zh-TW";
  }
  if (normalized.startsWith("zh")) return "zh-CN";
  if (normalized.startsWith("ja")) return "ja-JP";
  return "en-US";
}

interface I18nLike {
  resolvedLanguage?: string;
  language?: string;
}

export function getResolvedLang(i18n: I18nLike): string {
  return i18n.resolvedLanguage || i18n.language || "en";
}

/**
 * Token 数量的紧凑显示。
 *
 * Why: 中日文用户期待 "亿/万" 量纲；英文用户期待 K/M/B。共用同一份格式化
 * 逻辑避免 Hero 卡和分应用卡显示不一致。`compactDecimals=2` 用于 Hero
 * 大数副标（更精确），默认 1 位用于卡片副字段。
 */
export function formatTokensShort(
  value: number,
  lang: string,
  compactDecimals: 1 | 2 = 1,
): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  const decimals = compactDecimals;
  const normalizedLang = normalizeLanguageTag(lang);
  if (isTraditionalChineseLanguage(normalizedLang)) {
    if (value >= 1e8) return `${(value / 1e8).toFixed(2)} 億`;
    if (value >= 1e4) return `${(value / 1e4).toFixed(decimals)} 萬`;
    return value.toLocaleString("zh-TW");
  }
  if (normalizedLang.startsWith("zh") || normalizedLang.startsWith("ja")) {
    if (value >= 1e8) return `${(value / 1e8).toFixed(2)} 亿`;
    if (value >= 1e4) return `${(value / 1e4).toFixed(decimals)} 万`;
    return value.toLocaleString();
  }
  if (value >= 1e9) return `${(value / 1e9).toFixed(2)}B`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(decimals)}K`;
  return value.toLocaleString();
}
