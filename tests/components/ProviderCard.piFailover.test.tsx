import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ProviderCard } from "@/components/providers/ProviderCard";
import type { Provider } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";

vi.mock("@/lib/query/failover", () => ({
  useProviderHealth: () => ({
    data: {
      consecutive_failures: 3,
      is_healthy: false,
    },
  }),
}));

vi.mock("@/lib/query/queries", () => ({
  useUsageQuery: () => ({ data: undefined }),
}));

function piProvider(): Provider {
  return {
    id: "pi-1",
    name: "Pi Custom",
    category: "custom",
    settingsConfig: {
      baseUrl: "https://api.example.com/v1",
      apiKey: "sk-test",
      api: "openai-completions",
      models: [{ id: "model-a", name: "Model A" }],
    },
  };
}

describe("ProviderCard Pi failover labeling", () => {
  it("does not show failover takeover UI when stale failover props are supplied", () => {
    const queryClient = createTestQueryClient();

    render(
      <QueryClientProvider client={queryClient}>
        <ProviderCard
          provider={piProvider()}
          appId="pi"
          isCurrent={false}
          isInConfig
          isProxyRunning
          isProxyTakeover
          isAutoFailoverEnabled
          isInFailoverQueue
          failoverPriority={1}
          onToggleFailover={vi.fn()}
          onSwitch={vi.fn()}
          onEdit={vi.fn()}
          onDelete={vi.fn()}
          onConfigureUsage={vi.fn()}
          onOpenWebsite={vi.fn()}
          onDuplicate={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(screen.queryByText("P1")).not.toBeInTheDocument();
    expect(
      screen.queryByTitle("failover.priority.tooltip"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "failover.addQueue" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "failover.inQueue" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "移除" })).toBeInTheDocument();
  });
});
