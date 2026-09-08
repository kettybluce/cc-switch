import { describe, expect, it, vi } from "vitest";

const checkMock = vi.fn();

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: (...args: unknown[]) => checkMock(...args),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("3.20.2"),
}));

describe("fork updater", () => {
  it("does not consult upstream latest.json when the fork updater is disabled", async () => {
    const { FORK_DISABLE_APP_UPDATER } = await import("@/config/fork");
    expect(FORK_DISABLE_APP_UPDATER).toBe(true);

    const { checkForUpdate } = await import("@/lib/updater");
    await expect(checkForUpdate()).resolves.toEqual({ status: "up-to-date" });
    expect(checkMock).not.toHaveBeenCalled();
  });
});
