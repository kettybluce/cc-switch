import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  ClaudeRuntimeSettings,
  CodexRuntimeSettings,
} from "@/components/settings/CliRuntimeSettings";
import type { PiProxyPlan, PiRuntimeStatus, WslPiProbe } from "@/lib/api/pi";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) =>
      options
        ? `${key}:${Object.values(options)
            .filter((value) => typeof value !== "function")
            .join(",")}`
        : key,
  }),
}));

const setRuntimeMock = vi.fn();
const testProxyMock = vi.fn();
const refetchMock = vi.fn();

let status: PiRuntimeStatus | undefined;
let distros: WslPiProbe[] = [];
let proxyPlan: PiProxyPlan | undefined;

vi.mock("@/lib/query/pi", () => ({
  usePiRuntimeStatus: () => ({
    data: status,
    isLoading: status === undefined,
    isFetching: false,
    refetch: refetchMock,
  }),
  usePiWslDistros: (enabled: boolean) => ({
    data: enabled ? distros : undefined,
    isFetching: false,
  }),
  usePiProxyPlan: () => ({ data: proxyPlan }),
  useSetPiRuntime: () => ({ mutate: setRuntimeMock, isPending: false }),
  useTestPiProxy: () => ({ mutate: testProxyMock, isPending: false }),
}));

const setTakeoverForAppMock = vi.fn();
let takeover = { claude: false, codex: false, pi: false };

vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: () => ({
    takeoverStatus: takeover,
    setTakeoverForApp: setTakeoverForAppMock,
    isPending: false,
    isInitialStatusPending: false,
  }),
}));

const probe: WslPiProbe = {
  distro: "Ubuntu-22.04",
  home: "/home/tfdx8045",
  agentDir: "/home/tfdx8045/.pi/agent",
  piPath: "/usr/bin/pi",
  piVersion: "0.4.1",
  hasModels: true,
  hasSessions: true,
  sessionCount: 1,
  capabilities: { models: true, sessions: true, usage: true, duration: true },
};

const localStatus: PiRuntimeStatus = {
  settings: {
    kind: "local",
    flags: {
      wsl: true,
      wslProxy: true,
      session: true,
      sessionIncremental: true,
      usage: true,
    },
  },
  target: { kind: "local" },
  wslAvailable: true,
};

const wslStatus: PiRuntimeStatus = {
  settings: { ...localStatus.settings, kind: "wsl", distro: "Ubuntu-22.04" },
  target: {
    kind: "wsl",
    distro: "Ubuntu-22.04",
    home: probe.home,
    agentDir: probe.agentDir,
  },
  probe,
  wslAvailable: true,
};

describe("CliRuntimeSettings", () => {
  beforeEach(() => {
    setRuntimeMock.mockReset();
    testProxyMock.mockReset();
    refetchMock.mockReset();
    setTakeoverForAppMock.mockReset();
    takeover = { claude: false, codex: false, pi: false };
    status = localStatus;
    distros = [probe];
    proxyPlan = undefined;
  });

  it("shows Claude next to Pi with a shared WSL runtime and UNC ban", () => {
    render(<ClaudeRuntimeSettings />);
    expect(
      screen.getByText("settings.cliRuntime.claude.title"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("settings.cliRuntime.sharedRuntime"),
    ).toBeInTheDocument();
    expect(screen.getByText("settings.cliRuntime.uncBan")).toBeInTheDocument();
    expect(screen.getByText("~/.claude/settings.json")).toBeInTheDocument();
  });

  it("shows Codex config via wsl.exe when the shared runtime is WSL", () => {
    status = wslStatus;
    render(<CodexRuntimeSettings />);
    expect(
      screen.getByText("settings.cliRuntime.codex.title"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("wsl:Ubuntu-22.04:/home/tfdx8045/.codex/config.toml"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("wsl:Ubuntu-22.04:/home/tfdx8045/.codex/auth.json"),
    ).toBeInTheDocument();
    expect(screen.getByText("Ubuntu-22.04")).toBeInTheDocument();
  });

  it("toggles Claude takeover independently of Pi", async () => {
    render(<ClaudeRuntimeSettings />);
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(setTakeoverForAppMock).toHaveBeenCalledWith({
        appType: "claude",
        enabled: true,
      }),
    );
  });

  it("toggles Codex takeover and can test the shared proxy", async () => {
    render(<CodexRuntimeSettings />);
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(setTakeoverForAppMock).toHaveBeenCalledWith({
        appType: "codex",
        enabled: true,
      }),
    );
    fireEvent.click(screen.getByText("settings.piRuntime.testProxy"));
    await waitFor(() => expect(testProxyMock).toHaveBeenCalledTimes(1));
  });

  it("offers the shared Windows vs WSL location picker for Claude", () => {
    render(<ClaudeRuntimeSettings />);
    expect(
      screen.getByText("settings.cliRuntime.claude.location"),
    ).toBeInTheDocument();
    expect(screen.getByText("settings.piRuntime.local")).toBeInTheDocument();
  });
});
