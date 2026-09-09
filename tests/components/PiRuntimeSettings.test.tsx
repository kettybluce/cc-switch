import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { PiRuntimeSettings } from "@/components/settings/PiRuntimeSettings";
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
const syncSessionsMock = vi.fn();
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
  useSyncPiWslSessions: () => ({ mutate: syncSessionsMock, isPending: false }),
  useTestPiProxy: () => ({ mutate: testProxyMock, isPending: false }),
}));

const setTakeoverForAppMock = vi.fn();
let takeoverEnabled = false;

vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: () => ({
    takeoverStatus: { pi: takeoverEnabled },
    setTakeoverForApp: setTakeoverForAppMock,
    isPending: false,
    isInitialStatusPending: false,
  }),
}));

const probe: WslPiProbe = {
  distro: "Ubuntu-22.04",
  home: "/home/tfdx8045",
  agentDir: "/home/tfdx8045/.pi/agent",
  piPath: "/home/tfdx8045/.nvm/versions/node/v22.0.0/bin/pi",
  piVersion: "0.4.1",
  hasModels: true,
  hasSessions: true,
  sessionCount: 137,
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

describe("PiRuntimeSettings", () => {
  beforeEach(() => {
    setRuntimeMock.mockReset();
    syncSessionsMock.mockReset();
    testProxyMock.mockReset();
    refetchMock.mockReset();
    setTakeoverForAppMock.mockReset();
    takeoverEnabled = false;
    status = localStatus;
    distros = [probe];
    proxyPlan = undefined;
  });

  it("renders the proxy panel even on a platform that cannot host WSL", () => {
    status = { ...localStatus, wslAvailable: false };
    render(<PiRuntimeSettings />);
    expect(screen.getByText("settings.piRuntime.title")).toBeInTheDocument();
    expect(screen.getByText("settings.piRuntime.takeover")).toBeInTheDocument();
    expect(
      screen.queryByText("settings.piRuntime.location"),
    ).not.toBeInTheDocument();
  });

  it("offers the runtime choice when WSL is available", () => {
    render(<PiRuntimeSettings />);

    expect(screen.getByText("settings.piRuntime.title")).toBeInTheDocument();
    expect(screen.getByText("settings.piRuntime.local")).toBeInTheDocument();
  });

  it("does not ask for a distribution while Pi runs locally", () => {
    render(<PiRuntimeSettings />);

    expect(
      screen.queryByText("settings.piRuntime.distro"),
    ).not.toBeInTheDocument();
  });

  it("shows the detected Pi installation for the WSL runtime", () => {
    status = wslStatus;
    render(<PiRuntimeSettings />);

    expect(screen.getByText("settings.piRuntime.distro")).toBeInTheDocument();
    expect(screen.getByText("0.4.1")).toBeInTheDocument();
    expect(screen.getByText(probe.agentDir)).toBeInTheDocument();
    expect(
      screen.getByText(
        "\\\\wsl.localhost\\Ubuntu-22.04\\home\\tfdx8045\\.pi",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("settings.piRuntime.sessionCount:137"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("settings.piRuntime.natListenHint"),
    ).not.toBeInTheDocument();
  });

  it("does not suggest 0.0.0.0 when mirrored localhost already answered", () => {
    status = wslStatus;
    proxyPlan = {
      enabled: true,
      projected: true,
      gateway: {
        reachable: true,
        host: "127.0.0.1",
        strategy: "mirroredLoopback",
      },
      origin: "http://127.0.0.1:15721",
      listenAddress: "127.0.0.1",
      environment: {},
    };
    render(<PiRuntimeSettings />);

    expect(
      screen.queryByText("settings.piRuntime.natListenHint"),
    ).not.toBeInTheDocument();
  });

  it("shows the NAT listen hint only after the localhost probe fails", () => {
    status = wslStatus;
    proxyPlan = {
      enabled: true,
      projected: false,
      gateway: { reachable: false, error: "no route" },
      listenAddress: "127.0.0.1",
      environment: {},
    };
    render(<PiRuntimeSettings />);

    expect(
      screen.getByText("settings.piRuntime.natListenHint"),
    ).toBeInTheDocument();
  });

  it("hides the WSL NAT listen hint once the proxy already binds 0.0.0.0", () => {
    status = wslStatus;
    proxyPlan = {
      enabled: true,
      projected: true,
      gateway: { reachable: true },
      origin: "http://172.30.208.1:15721",
      listenAddress: "0.0.0.0",
      environment: {},
    };
    render(<PiRuntimeSettings />);

    expect(
      screen.queryByText("settings.piRuntime.natListenHint"),
    ).not.toBeInTheDocument();
  });

  it("surfaces a runtime error without hiding the rest of the panel", () => {
    status = {
      ...wslStatus,
      error: { code: "PI_NOT_FOUND", message: "Pi is not on PATH in WSL" },
    };
    render(<PiRuntimeSettings />);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Pi is not on PATH in WSL",
    );
    expect(screen.getByText("settings.piRuntime.title")).toBeInTheDocument();
  });

  it("refreshes WSL sessions on demand", async () => {
    status = wslStatus;
    render(<PiRuntimeSettings />);

    fireEvent.click(screen.getByText("settings.piRuntime.syncSessions"));

    await waitFor(() => expect(syncSessionsMock).toHaveBeenCalledTimes(1));
  });

  it("shows a Claude-style takeover switch that starts the proxy on enable", async () => {
    render(<PiRuntimeSettings />);
    expect(screen.getByText("settings.piRuntime.takeover")).toBeInTheDocument();
    expect(
      screen.getByText("settings.piRuntime.takeoverDescription"),
    ).toBeInTheDocument();
    const toggle = screen.getByRole("switch");
    expect(toggle).toBeInTheDocument();
    expect(toggle).not.toBeChecked();
    expect(
      screen.getByText("settings.piRuntime.proxyStopped"),
    ).toBeInTheDocument();
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(setTakeoverForAppMock).toHaveBeenCalledWith({
        appType: "pi",
        enabled: true,
      }),
    );
    expect(
      screen.getByText("settings.piRuntime.testProxy"),
    ).toBeInTheDocument();
  });

  it("shows takeover-on copy when live models.json is projected", () => {
    takeoverEnabled = true;
    proxyPlan = {
      enabled: true,
      projected: true,
      gateway: { reachable: true },
      origin: "http://127.0.0.1:15721",
      listenAddress: "127.0.0.1",
      environment: {},
    };
    render(<PiRuntimeSettings />);
    expect(screen.getByRole("switch")).toBeChecked();
    expect(
      screen.getByText("settings.piRuntime.takeoverOn"),
    ).toBeInTheDocument();
  });

  it("tests the proxy on demand", async () => {
    status = wslStatus;
    render(<PiRuntimeSettings />);

    fireEvent.click(screen.getByText("settings.piRuntime.testProxy"));

    await waitFor(() => expect(testProxyMock).toHaveBeenCalledTimes(1));
  });
});
