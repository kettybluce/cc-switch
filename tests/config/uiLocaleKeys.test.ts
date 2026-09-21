import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
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

const SRC_ROOT = join(process.cwd(), "src");
const TARGET_PATH = /(providers|prompts|opencode|omo|codex|claude|\/pi)/i;
const KEY_RE = /\b(?:t|i18n\.t)\(\s*(['"])([^`'"]+)\1/g;

function walk(dir: string, acc: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      walk(full, acc);
      continue;
    }
    if (!/\.(ts|tsx)$/.test(name)) continue;
    if (/\.test\.(ts|tsx)$/.test(name)) continue;
    acc.push(full);
  }
  return acc;
}

function extractKeys(files: string[]): Map<string, string[]> {
  const used = new Map<string, string[]>();
  for (const file of files) {
    const rel = relative(process.cwd(), file);
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(KEY_RE)) {
      const key = match[2];
      if (!key || key.includes("${")) continue;
      const refs = used.get(key) ?? [];
      refs.push(rel);
      used.set(key, refs);
    }
  }
  return used;
}

const locales = [
  ["en", flattenStrings(en as TranslationTree)],
  ["zh", flattenStrings(zh as TranslationTree)],
  ["ja", flattenStrings(ja as TranslationTree)],
  ["zh-TW", flattenStrings(zhTW as TranslationTree)],
] as const;

describe("Pi/Claude/Codex/OpenCode UI locale keys", () => {
  const files = walk(SRC_ROOT).filter((file) => TARGET_PATH.test(file));
  const used = extractKeys(files);

  it("scans at least the provider/prompt surfaces", () => {
    expect(files.length).toBeGreaterThan(20);
    expect(used.size).toBeGreaterThan(50);
  });

  it.each(locales)(
    "has every scanned UI key in %s",
    (_name, translations) => {
      const missing = [...used.keys()].filter((key) => !translations.has(key));
      expect(missing).toEqual([]);
    },
  );
});
