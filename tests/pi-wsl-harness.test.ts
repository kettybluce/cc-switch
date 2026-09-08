import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const FIXTURES = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fixtures/pi-wsl",
);

const CWD = "/home/tfdx8045/code/agent";
const CWD_GROUP = "--home-tfdx8045-code-agent--";

/** Pi encodes a launch cwd as a sessions/ directory name. */
function encodeSessionCwd(cwd: string): string {
  const withoutRoot = cwd.trim().replace(/^\/+/, "");
  return `--${withoutRoot.replaceAll("/", "-")}--`;
}

function decodeSessionCwd(encoded: string): string {
  const inner = encoded.replace(/^--/, "").replace(/--$/, "");
  return `/${inner.replaceAll("-", "/")}`;
}

function parseJsonl(filePath: string): Record<string, unknown>[] {
  return readFileSync(filePath, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Record<string, unknown>);
}

describe("Pi WSL Linux harness fixtures", () => {
  it("encodes and decodes the documented cwd group", () => {
    expect(encodeSessionCwd(CWD)).toBe(CWD_GROUP);
    expect(decodeSessionCwd(CWD_GROUP)).toBe(CWD);
    expect(encodeSessionCwd(CWD)).toBe(
      path.basename(path.join(FIXTURES, "sessions", CWD_GROUP)),
    );
  });

  it("keeps session JSONL under the encoded cwd group, not a flat glob", () => {
    const sessionsRoot = path.join(FIXTURES, "sessions");
    const topLevelJsonl = readdirSync(sessionsRoot).filter((name) =>
      name.endsWith(".jsonl"),
    );
    expect(topLevelJsonl).toEqual([]);
    expect(
      readdirSync(sessionsRoot, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name),
    ).toEqual([CWD_GROUP]);
  });

  it("parses session + model_change + zero-cost usage lines", () => {
    const file = path.join(
      FIXTURES,
      "sessions",
      CWD_GROUP,
      "2026-03-14T10-32-00_abc.jsonl",
    );
    const lines = parseJsonl(file);
    expect(lines.map((line) => line.type)).toEqual([
      "session",
      "model_change",
      "message",
      "message",
    ]);

    expect(lines[0]).toMatchObject({
      type: "session",
      id: "sess-parent-abc123",
      cwd: CWD,
    });
    expect(lines[1]).toMatchObject({
      type: "model_change",
      provider: "cc-switch-harness",
      model: "gpt-4.1-mini",
    });

    const assistant = lines[3]?.message as {
      role: string;
      usage: { input: number; cost: { total: number } };
    };
    expect(assistant.role).toBe("assistant");
    expect(assistant.usage.input).toBe(1_000_000);
    expect(assistant.usage.cost.total).toBe(0);
  });

  it("includes a nested tasks/*.jsonl session directory", () => {
    const taskFile = path.join(
      FIXTURES,
      "sessions",
      CWD_GROUP,
      "2026-03-14T10-32-00_abc",
      "tasks",
      "task-1.jsonl",
    );
    const lines = parseJsonl(taskFile);
    expect(lines[0]).toMatchObject({
      type: "session",
      id: "sess-task-1",
      parentSession: "sess-parent-abc123",
      cwd: CWD,
    });
    const usage = (lines[2]?.message as { usage: { cost: { total: number } } })
      .usage;
    expect(usage.cost.total).toBe(0);
  });

  it("ships identical / diverge / only-agent models.json sync cases", () => {
    const read = (rel: string) =>
      JSON.parse(readFileSync(path.join(FIXTURES, rel), "utf8")) as {
        providers: Record<string, { baseUrl?: string }>;
      };

    const identicalAgent = read("cases/identical/agent/models.json");
    const identicalTop = read("cases/identical/models.json");
    expect(identicalAgent).toEqual(identicalTop);
    expect(identicalAgent.providers["cc-switch-harness"]?.baseUrl).toBe(
      "https://api.example.com/v1",
    );

    const divergeAgent = read("cases/diverge/agent/models.json");
    const divergeTop = read("cases/diverge/models.json");
    expect(divergeAgent).not.toEqual(divergeTop);
    expect(divergeTop.providers["stale-proxy"]?.baseUrl).toContain(
      "/pi/stale-proxy/",
    );

    const onlyAgent = read("cases/only-agent/agent/models.json");
    expect(onlyAgent.providers["cc-switch-harness"]).toBeTruthy();
  });
});
