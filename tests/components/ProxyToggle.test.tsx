import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProxyToggle } from "@/components/proxy/ProxyToggle";

const useProxyStatusMock = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: useProxyStatusMock,
}));

describe("ProxyToggle", () => {
  beforeEach(() => {
    useProxyStatusMock.mockReset();
  });

  it("waits for initial proxy status before allowing takeover", () => {
    const proxyState = {
      isRunning: false,
      takeoverStatus: undefined,
      setTakeoverForApp: vi.fn(),
      isPending: false,
      isInitialStatusPending: true,
      status: undefined,
    };
    useProxyStatusMock.mockImplementation(() => proxyState);
    const { rerender } = render(<ProxyToggle activeApp="claude" />);

    expect(screen.getByRole("switch")).toBeDisabled();

    proxyState.isInitialStatusPending = false;
    rerender(<ProxyToggle activeApp="claude" />);

    expect(screen.getByRole("switch")).toBeEnabled();
  });

  it("renders a Pi takeover switch using the Claude UX", () => {
    useProxyStatusMock.mockImplementation(() => ({
      isRunning: true,
      takeoverStatus: { pi: true },
      setTakeoverForApp: vi.fn(),
      isPending: false,
      isInitialStatusPending: false,
      status: { address: "127.0.0.1", port: 15721 },
    }));
    render(<ProxyToggle activeApp="pi" />);
    expect(screen.getByRole("switch")).toBeChecked();
  });
});
