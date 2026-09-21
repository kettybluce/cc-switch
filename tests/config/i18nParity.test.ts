import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

type ParityReport = {
  ok: boolean;
  filesScanned: number;
  literalKeys: number;
  localeKeyCounts: Record<string, number>;
  missingInLocales: string[];
  orphans: string[];
  schema19Keys: string[];
};

function runParity(args: string[] = []): {
  stdout: string;
  report?: ParityReport;
} {
  const script = join(process.cwd(), "scripts/check-i18n-parity.mjs");
  const stdout = execFileSync(process.execPath, [script, ...args], {
    encoding: "utf8",
  });
  return {
    stdout,
    report: args.includes("--json")
      ? (JSON.parse(stdout) as ParityReport)
      : undefined,
  };
}

describe("i18n four-locale key parity script", () => {
  it("exits 0 with matching zh/en/zh-TW/ja keys and no target-app drift", () => {
    const { stdout, report } = runParity(["--json"]);
    expect(report).toBeDefined();
    const counts = Object.values(report!.localeKeyCounts);
    expect(counts.every((count) => count === counts[0])).toBe(true);
    expect(counts[0]).toBeGreaterThan(2000);
    expect(report!.missingInLocales).toEqual([]);
    expect(report!.orphans).toEqual([]);
    expect(report!.schema19Keys).toEqual([]);
    expect(report!.ok).toBe(true);
    expect(report!.filesScanned).toBeGreaterThan(100);
    expect(report!.literalKeys).toBeGreaterThan(500);
    expect(stdout).toContain('"ok": true');
  });

  it("prints a human-readable OK summary", () => {
    const { stdout } = runParity();
    expect(stdout).toMatch(/OK/);
    expect(stdout).toMatch(/zh \/ zh-TW \/ en \/ ja/);
  });
});
